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

// Benchmark Galeforce vs Tailwind 3 oracle on a representative project.
//
// Measures three dimensions independently because they're optimised
// differently:
//
//   1. **Scan**  — discover + tokenize content. For Galeforce this is the
//      `galeforcecss scan` subcommand. For Tailwind it happens internally
//      (we approximate with the synthesized-content pathway via the
//      oracle's full compile, since there's no scan-only handle).
//   2. **Compile (cold)** — start a fresh process, hand it candidates +
//      input CSS, get CSS back. Measures startup + compile.
//   3. **Compile (warm)** — same compile, but with the process already
//      running (`compile-stream` for Galeforce; reuse the imported
//      tailwindcss for the oracle). Measures pure compute.
//
// Each phase runs N iterations and reports mean / min / p95.
//
// Usage: pnpm --filter @coralogix/galeforcecss-conformance bench
//        pnpm --filter @coralogix/galeforcecss-conformance bench -- --project /abs/path

import { spawnSync, spawn } from 'node:child_process'
import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { resolve, dirname, join } from 'node:path'
import { tmpdir } from 'node:os'
import { fileURLToPath } from 'node:url'
import { performance } from 'node:perf_hooks'
import { compile, createCompileStream } from '@coralogix/galeforcecss'
import { compileWithTailwind3 } from '@coralogix/galeforcecss-oracle'

const here = dirname(fileURLToPath(import.meta.url))
const repoRoot = resolve(here, '../../..')
function readProjectArg(): string {
  const idx = process.argv.indexOf('--project')
  if (idx === -1) return resolve(repoRoot, 'smoke')
  const next = process.argv[idx + 1]
  if (!next) throw new Error('--project requires a path argument')
  return resolve(next)
}
const projectRoot = readProjectArg()
const iterations = (() => {
  const idx = process.argv.indexOf('--iterations')
  if (idx === -1) return 5
  const n = parseInt(process.argv[idx + 1] ?? '', 10)
  if (!Number.isFinite(n) || n < 1) throw new Error('--iterations needs a positive integer')
  return n
})()

function findBinary(): string {
  for (const c of [
    resolve(repoRoot, 'target/release/galeforcecss'),
    resolve(repoRoot, 'target/debug/galeforcecss'),
  ])
    if (existsSync(c)) return c
  throw new Error('galeforcecss binary not found — `cargo build -p galeforce-cli --release` first')
}

const inputCss = '@tailwind base;\n@tailwind components;\n@tailwind utilities;\n'
const config = { darkMode: 'class' as const }

function stats(samples: number[]): { mean: number; min: number; p95: number } {
  const sorted = [...samples].sort((a, b) => a - b)
  const sum = sorted.reduce((a, b) => a + b, 0)
  const p95Idx = Math.min(sorted.length - 1, Math.ceil(sorted.length * 0.95) - 1)
  return { mean: sum / sorted.length, min: sorted[0]!, p95: sorted[p95Idx]! }
}

function fmt(ms: number): string {
  if (ms < 1) return `${(ms * 1000).toFixed(0)}µs`
  if (ms < 1000) return `${ms.toFixed(1)}ms`
  return `${(ms / 1000).toFixed(2)}s`
}

function row(label: string, samples: number[], extra = ''): void {
  const s = stats(samples)
  const mean = fmt(s.mean).padStart(8)
  const min = fmt(s.min).padStart(8)
  const p95 = fmt(s.p95).padStart(8)
  console.log(`  ${label.padEnd(28)}  mean=${mean}  min=${min}  p95=${p95}  ${extra}`)
}

async function timeAsync(fn: () => Promise<void>, n: number): Promise<number[]> {
  const samples: number[] = []
  for (let i = 0; i < n; i++) {
    const t = performance.now()
    await fn()
    samples.push(performance.now() - t)
  }
  return samples
}

function timeSync(fn: () => void, n: number): number[] {
  const samples: number[] = []
  for (let i = 0; i < n; i++) {
    const t = performance.now()
    fn()
    samples.push(performance.now() - t)
  }
  return samples
}

async function main(): Promise<void> {
  const bin = findBinary()
  console.log(`bench: project=${projectRoot}`)
  console.log(`bench: iterations=${iterations}`)
  console.log()

  // ---- Phase 1: scan ----
  // Both runs read the candidate set from `galeforcecss scan`. Tailwind's
  // own scanner is internal to the compile pass; we don't expose it
  // standalone, so we report Galeforce's scan and assume the oracle's is
  // proportional (it folds into Compile-cold below).
  const scanSamples = timeSync(() => {
    const r = spawnSync(bin, ['scan', '--content', projectRoot, '--json'], {
      encoding: 'utf8',
    })
    if (r.status !== 0) throw new Error(`scan failed: ${r.stderr}`)
  }, iterations)
  row('Galeforce scan (cold)', scanSamples)

  // Capture the candidate set ONCE so the compile benchmarks below
  // measure compile-only time, not scan + compile.
  const r = spawnSync(bin, ['scan', '--content', projectRoot, '--json'], {
    encoding: 'utf8',
  })
  if (r.status !== 0) throw new Error(`scan failed: ${r.stderr}`)
  const candidates = JSON.parse(r.stdout) as string[]
  console.log(`  ${`(${candidates.length} candidates)`.padEnd(28)}`)
  console.log()

  // ---- Phase 2: compile (cold) ----
  // `compile()` from the public package spawns a fresh `galeforcecss
  // compile-json` per call. So this measures spawn + parse + compile
  // + serialize + read.
  const galeforceColdSamples = await timeAsync(async () => {
    await compile({ candidates, inputCss, config })
  }, iterations)
  row('Galeforce compile (cold)', galeforceColdSamples)

  const oracleColdSamples = await timeAsync(async () => {
    await compileWithTailwind3({ candidates, inputCss, config })
  }, iterations)
  row('Oracle compile (warm import)', oracleColdSamples, '(tailwindcss import is reused)')
  console.log()

  // ---- Phase 3: compile (warm) ----
  // Long-running stream amortises the spawn cost; this measures the
  // pure compile per call.
  const stream = createCompileStream()
  // Warmup: first call pays the lazy-init cost.
  await stream.compile({ candidates, inputCss, config })
  const galeforceWarmSamples = await timeAsync(async () => {
    await stream.compile({ candidates, inputCss, config })
  }, iterations)
  await stream.close()
  row('Galeforce compile (stream)', galeforceWarmSamples, '(spawn amortised)')

  // Oracle "warm" — import is at module top; just call again.
  await compileWithTailwind3({ candidates, inputCss, config }) // warmup
  const oracleWarmSamples = await timeAsync(async () => {
    await compileWithTailwind3({ candidates, inputCss, config })
  }, iterations)
  row('Oracle compile (warm)', oracleWarmSamples)
  console.log()

  // ---- Phase 4: CLI head-to-head ----
  // Both Galeforce and Tailwind 3 have a `build` style CLI. This is the
  // user-visible cost: every keystroke save in a non-watch flow pays
  // for `cli startup + scan + compile + write`. No keep-alive process
  // — apples-to-apples for `npx tailwindcss build` vs
  // `galeforcecss build`.
  console.log('--- CLI head-to-head ---')
  const tw3CliPath = resolve(
    repoRoot,
    'node_modules/.pnpm/tailwindcss@3.4.19_tsx@4.21.0/node_modules/tailwindcss/lib/cli.js',
  )
  if (!existsSync(tw3CliPath)) {
    console.log(`  (skipped — tailwindcss@3.4.19 CLI not found at ${tw3CliPath})`)
  } else {
    const cliWorkdir = mkdtempSync(join(tmpdir(), 'galeforce-bench-'))
    const inputPath = join(cliWorkdir, 'input.css')
    writeFileSync(inputPath, inputCss)
    const tw3OutPath = join(cliWorkdir, 'tw3-out.css')
    const galeforceOutPath = join(cliWorkdir, 'galeforce-out.css')

    // Tailwind 3 CLI: `node tailwindcss/lib/cli.js -i input.css
    // -o out.css --content "<projectRoot>/**/*.{html,ts,tsx,js,jsx}"`
    // Tailwind's CLI rejects directory args; it wants globs.
    const contentGlob = `${projectRoot}/**/*.{html,ts,tsx,js,jsx,vue,svelte,mdx}`
    const tw3Samples = timeSync(() => {
      const r = spawnSync(
        'node',
        [tw3CliPath, '-i', inputPath, '-o', tw3OutPath, '--content', contentGlob],
        { encoding: 'utf8' },
      )
      if (r.status !== 0) throw new Error(`tw3 cli failed: ${r.stderr}`)
    }, iterations)
    row('Tailwind 3 CLI (build)', tw3Samples, '(node + tailwindcss/lib/cli.js)')

    // Galeforce CLI: `galeforcecss build -i input.css -o out.css --content <projectRoot>`
    const galeforceSamples = timeSync(() => {
      const r = spawnSync(
        bin,
        [
          'build',
          '-i',
          inputPath,
          '-o',
          galeforceOutPath,
          '--content',
          projectRoot,
        ],
        { encoding: 'utf8' },
      )
      if (r.status !== 0) throw new Error(`galeforcecss build failed: ${r.stderr}`)
    }, iterations)
    row('Galeforce CLI (build)', galeforceSamples, '(galeforcecss build)')

    rmSync(cliWorkdir, { recursive: true, force: true })

    const tw3 = stats(tw3Samples).mean
    const br = stats(galeforceSamples).mean
    console.log()
    console.log(
      `summary: CLI build  tw3 / galeforce = ${(tw3 / br).toFixed(2)}x  ` +
        `(${fmt(tw3)} / ${fmt(br)})`,
    )
  }

  // ---- Final summary ----
  const galeforceWarm = stats(galeforceWarmSamples).mean
  const oracleWarm = stats(oracleWarmSamples).mean
  const ratio = oracleWarm / galeforceWarm
  console.log(
    `summary: warm-compile ratio  oracle / galeforce = ${ratio.toFixed(2)}x  ` +
      `(${fmt(oracleWarm)} / ${fmt(galeforceWarm)})`,
  )
}

main().catch((err) => {
  console.error('bench failed:', err)
  process.exitCode = 1
})
// silence unused-import warning when the module is only used as a script
void spawn
