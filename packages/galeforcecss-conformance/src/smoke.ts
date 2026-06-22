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

// Smoke test: point Galeforce and the Tailwind v3 oracle at a representative
// project, diff their output, and report. Reveals divergences that the
// curated fixtures don't surface — real codebases mix utilities in ways
// nobody writes in a hand-curated test.
//
// Usage: pnpm --filter @coralogix/galeforcecss-conformance smoke
//        pnpm --filter @coralogix/galeforcecss-conformance smoke -- --verbose
//        pnpm --filter @coralogix/galeforcecss-conformance smoke -- --project /abs/path/to/project

import { spawnSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { compile } from '@coralogix/galeforcecss'
import { compileWithTailwind3 } from '@coralogix/galeforcecss-oracle'
import { normalizeCss, diffStylesheets } from './index.js'

const here = dirname(fileURLToPath(import.meta.url))
// here is packages/galeforcecss-conformance/src; repo root is three up.
const repoRoot = resolve(here, '../../..')
const verbose = process.argv.includes('--verbose')
// `--project <path>` overrides the default ./smoke synthetic project.
function readProjectArg(): string {
  const idx = process.argv.indexOf('--project')
  if (idx === -1) return resolve(repoRoot, 'smoke')
  const next = process.argv[idx + 1]
  if (!next) throw new Error('--project requires a path argument')
  return resolve(next)
}
const projectRoot = readProjectArg()

function findBinary(): string {
  const candidates = [
    resolve(repoRoot, 'target/release/galeforcecss'),
    resolve(repoRoot, 'target/debug/galeforcecss'),
  ]
  for (const c of candidates) if (existsSync(c)) return c
  throw new Error('galeforcecss binary not found — `cargo build -p galeforce-cli --release` first')
}

// Use the CLI's `scan` subcommand so the JS side and the Rust compiler
// see exactly the same candidate set. Avoids tokenizer drift between a
// JS reimplementation and the Rust truth.
function scanCandidates(): string[] {
  const bin = findBinary()
  const r = spawnSync(bin, ['scan', '--content', projectRoot, '--json'], {
    encoding: 'utf8',
  })
  if (r.status !== 0) {
    throw new Error(`scan failed: ${r.stderr || '(no stderr)'}`)
  }
  return JSON.parse(r.stdout) as string[]
}

async function main() {
  console.log(`smoke-test: scanning ${projectRoot}`)
  const candidates = scanCandidates()
  console.log(`  found ${candidates.length} candidates`)

  // Use the natural full-mode input so preflight and base block both
  // matter. `darkMode: 'class'` matches the components' `dark:` prefix
  // expectations.
  const inputCss = '@tailwind base;\n@tailwind components;\n@tailwind utilities;\n'
  const config = { darkMode: 'class' as const }

  console.log('  compiling with Tailwind 3 oracle...')
  const oracleOut = await compileWithTailwind3({ candidates, inputCss, config })
  console.log(`    ${oracleOut.css.length} bytes`)

  console.log('  compiling with Galeforce...')
  const galeforceOut = await compile({ candidates, inputCss, config })
  console.log(`    ${galeforceOut.css.length} bytes, ${galeforceOut.diagnostics.length} diagnostics`)
  if (galeforceOut.diagnostics.length > 0) {
    const byCode = new Map<string, number>()
    for (const d of galeforceOut.diagnostics) {
      byCode.set(d.code, (byCode.get(d.code) ?? 0) + 1)
    }
    for (const [code, count] of [...byCode.entries()].sort((a, b) => b[1] - a[1])) {
      console.log(`      ${count.toString().padStart(4)}  ${code}`)
    }
  }

  const oracleNorm = normalizeCss(oracleOut.css)
  const galeforceNorm = normalizeCss(galeforceOut.css)

  const diff = diffStylesheets(galeforceNorm, oracleNorm, {})
  console.log()
  console.log(`oracle rules: ${oracleNorm.rules.length}`)
  console.log(`galeforce rules: ${galeforceNorm.rules.length}`)
  console.log(`diff entries: ${diff.length}`)

  if (diff.length === 0) {
    console.log('PASS — outputs match (modulo PostCSS normalization).')
    return
  }

  // Bucket diff entries by kind so the summary is digestible.
  const byKind = new Map<string, number>()
  for (const entry of diff) {
    byKind.set(entry.kind, (byKind.get(entry.kind) ?? 0) + 1)
  }
  console.log()
  console.log('--- divergence summary by kind ---')
  for (const [kind, count] of [...byKind.entries()].sort((a, b) => b[1] - a[1])) {
    console.log(`  ${count.toString().padStart(4)}  ${kind}`)
  }

  const formatEntry = (e: typeof diff[number]) => {
    const ctx = e.context.length > 0 ? `${e.context.join(' / ')} :: ` : ''
    return `  [${e.kind}] ${ctx}${e.selector}  ${e.detail}`
  }

  if (verbose) {
    console.log()
    console.log('--- full diff ---')
    for (const entry of diff.slice(0, 500)) {
      console.log(formatEntry(entry))
    }
    if (diff.length > 500) {
      console.log(`  … and ${diff.length - 500} more`)
    }
  } else {
    console.log()
    console.log('--- first 30 diff entries ---')
    for (const entry of diff.slice(0, 30)) {
      console.log(formatEntry(entry))
    }
    if (diff.length > 30) {
      console.log(`  (re-run with --verbose for the full list)`)
    }
  }
  process.exitCode = 1
}

main().catch((err) => {
  console.error('smoke-test failed:', err)
  process.exitCode = 1
})
