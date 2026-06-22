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

import { readdir } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { loadFixture } from './fixture.js'
import { runFixture } from './runner.js'

const here = fileURLToPath(new URL('.', import.meta.url))
const FIXTURES_ROOT = resolve(here, '../../../conformance/fixtures')

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

describe('conformance fixtures', async () => {
  const paths = (await findFixtures(FIXTURES_ROOT)).sort()

  it('discovers at least one fixture', () => {
    expect(paths.length).toBeGreaterThan(0)
  })

  for (const path of paths) {
    it(`fixture: ${path.replace(FIXTURES_ROOT + '/', '')}`, async () => {
      const fx = await loadFixture(path)
      const result = await runFixture(fx)

      // Phase A acceptance: oracle compiles successfully and Galeforce is either
      // (a) zero-diff against it or (b) explicitly marked unimplemented.
      // Once `compileWithGaleforce` returns real CSS, the `unimplemented` branch
      // disappears and these assertions become a real conformance gate.
      expect(result.oracleCss.length).toBeGreaterThan(0)
      if (!result.unimplemented) {
        expect.soft(result.diffs).toEqual([])
        if (result.diffs.length > 0) {
          throw new Error(`conformance diff for ${result.fixture}:\n${result.report}`)
        }
      }
    })
  }
})

describe('Galeforce compiler bridge', () => {
  it('produces CSS for static/flex via the galeforce-cli binary', async () => {
    const fx = await loadFixture(resolve(FIXTURES_ROOT, 'static/flex.json'))
    const result = await runFixture(fx)
    // Bridge is wired; output should be real CSS (no placeholder marker).
    expect(result.unimplemented).toBe(false)
    expect(result.galeforceCss).toBeTypeOf('string')
    expect(result.galeforceCss).toMatch(/\.flex\s*\{[^}]*display:\s*flex/)
  })
})
