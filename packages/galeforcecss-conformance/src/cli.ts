#!/usr/bin/env node
/*
 * Copyright 2026 Coralogix Ltd.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

// galeforce-conformance — runs every fixture under conformance/fixtures and
// reports diffs against the oracle.

import { readdir } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { loadFixture } from './fixture.js'
import { runFixture } from './runner.js'

async function findFixtures(root: string): Promise<string[]> {
  const out: string[] = []
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const full = join(root, entry.name)
    if (entry.isDirectory()) {
      out.push(...(await findFixtures(full)))
    } else if (entry.isFile() && entry.name.endsWith('.json')) {
      out.push(full)
    }
  }
  return out
}

async function main(): Promise<number> {
  const here = fileURLToPath(new URL('.', import.meta.url))
  // Default fixture root: <repo>/conformance/fixtures
  const argRoot = process.argv[2]
  const fixturesRoot = argRoot ? resolve(argRoot) : resolve(here, '../../../conformance/fixtures')

  let paths: string[]
  try {
    paths = await findFixtures(fixturesRoot)
  } catch (err) {
    console.error(`galeforce-conformance: cannot read ${fixturesRoot}: ${(err as Error).message}`)
    return 1
  }

  if (paths.length === 0) {
    console.error(`galeforce-conformance: no fixtures under ${fixturesRoot}`)
    return 1
  }

  let pass = 0
  let unimpl = 0
  let fail = 0
  for (const path of paths.sort()) {
    const fx = await loadFixture(path)
    const result = await runFixture(fx)
    if (result.unimplemented) {
      unimpl++
      console.log(`  ?  ${result.fixture}  (Galeforce not implemented)`)
      continue
    }
    if (result.diffs.length === 0) {
      pass++
      console.log(`  ✓  ${result.fixture}`)
    } else {
      fail++
      console.log(`  ✗  ${result.fixture}`)
      console.log(indent(result.report, '       '))
    }
  }

  console.log(`\n${pass} pass, ${fail} fail, ${unimpl} unimplemented (${paths.length} total)`)
  return fail > 0 ? 1 : 0
}

function indent(s: string, prefix: string): string {
  return s
    .split('\n')
    .map((l) => prefix + l)
    .join('\n')
}

main().then(
  (code) => process.exit(code),
  (err) => {
    console.error(err)
    process.exit(1)
  },
)
