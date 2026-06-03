// Walks every fixture under conformance/fixtures/ and writes the oracle's
// raw output to conformance/snapshots/<fixture-name>.css. The conformance
// harness itself doesn't need these — it diffs live against the oracle —
// but they're useful as a *review surface* when the Tailwind pin moves:
//
//   1. Bump tailwindcss in package.json + the submodule.
//   2. Run `pnpm fixtures:rebuild`.
//   3. `git diff conformance/snapshots/` shows exactly what the upstream
//      compiler now emits differently. Decide which diffs are real
//      upstream changes (port them) vs. Galeforce regressions (fix them).
//
// Run with: `pnpm fixtures:rebuild` (script is added to root package.json).

import { createHash } from 'node:crypto'
import { mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { dirname, join, relative, resolve } from 'node:path'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'
// Resolve the workspace packages by source-file path so `tsx` can run this
// from the repo root without needing a root-level dep on the oracle.
import { compileWithTailwind3 } from '../packages/galeforcecss-oracle/src/index.js'
import { loadFixture } from '../packages/galeforcecss-conformance/src/fixture.js'

const here = dirname(fileURLToPath(import.meta.url))
const repoRoot = resolve(here, '..')
const FIXTURES_ROOT = resolve(repoRoot, 'conformance/fixtures')
const SNAPSHOTS_ROOT = resolve(repoRoot, 'conformance/snapshots')
const require_ = createRequire(import.meta.url)
const TAILWIND_VERSION: string = (
  require_('tailwindcss/package.json') as { version: string }
).version
const NODE_VERSION = process.version
const GENERATED_AT = new Date().toISOString()

function hash(input: string): string {
  return createHash('sha256').update(input).digest('hex').slice(0, 16)
}

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

async function main(): Promise<void> {
  // Wipe + recreate so deleted fixtures don't leave stale snapshots behind.
  await rm(SNAPSHOTS_ROOT, { recursive: true, force: true })
  await mkdir(SNAPSHOTS_ROOT, { recursive: true })

  const fixturePaths = (await findFixtures(FIXTURES_ROOT)).sort()
  const manifest: Record<
    string,
    { fixtureHash: string; configHash: string; outputBytes: number }
  > = {}
  let count = 0
  for (const path of fixturePaths) {
    const fx = await loadFixture(path)
    const inputCss = fx.inputCss ?? '@tailwind utilities;\n'
    const config = fx.config ?? { corePlugins: { preflight: false } }
    const { css } = await compileWithTailwind3({
      candidates: fx.candidates,
      inputCss,
      config,
    })
    const fixtureBytes = await readFile(path)
    const fixtureKey = relative(FIXTURES_ROOT, path)
    manifest[fixtureKey] = {
      fixtureHash: hash(fixtureBytes.toString('utf8')),
      configHash: hash(JSON.stringify(config)),
      outputBytes: Buffer.byteLength(css, 'utf8'),
    }
    const relPath = fixtureKey.replace(/\.json$/, '.css')
    const outPath = join(SNAPSHOTS_ROOT, relPath)
    await mkdir(dirname(outPath), { recursive: true })
    await writeFile(outPath, css, 'utf8')
    count += 1
  }

  // Manifest at the root captures version provenance + per-fixture
  // hashes. Snapshots stay byte-identical between rebuilds when nothing
  // changes; the manifest changes on every rebuild because of the
  // `generated` timestamp, which is fine — it's a single file.
  const manifestPath = join(SNAPSHOTS_ROOT, '_manifest.json')
  await writeFile(
    manifestPath,
    JSON.stringify(
      {
        tailwind: TAILWIND_VERSION,
        node: NODE_VERSION,
        generated: GENERATED_AT,
        fixtureCount: count,
        fixtures: manifest,
      },
      null,
      2,
    ) + '\n',
    'utf8',
  )
  console.log(`Wrote ${count} oracle snapshots to ${relative(repoRoot, SNAPSHOTS_ROOT)}/`)
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
