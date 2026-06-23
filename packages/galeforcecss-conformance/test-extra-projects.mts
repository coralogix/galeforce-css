import { spawnSync } from 'child_process'
import { compileWithTailwind3 } from '@coralogix/galeforcecss-oracle'
import { compile } from '@coralogix/galeforcecss'
import { processRawConfigAsync } from '@coralogix/galeforcecss-config-loader'
import { normalizeCss, diffStylesheets } from './src/index.js'
import * as path from 'path'
import * as fs from 'fs'
import { createRequire } from 'module'
import Module from 'module'

const require = createRequire(import.meta.url)
const REPO_ROOT = new URL('../../', import.meta.url).pathname
const galeforceBin = path.join(REPO_ROOT, 'target/release/galeforcecss')

function mockPlugins(extras: string[] = []) {
  const mod = Module as any
  const orig = mod._load
  mod._load = function (req: string, parent: unknown, isMain: boolean) {
    if (req === 'tailwindcss-rtl' || req === 'tailwindcss-animate' || extras.includes(req)) {
      return () => {}
    }
    return orig.apply(this, [req, parent, isMain])
  }
}

async function scanCandidates(srcDir: string): Promise<string[]> {
  const r = spawnSync(galeforceBin, ['scan', '--content', srcDir, '--json'], {
    encoding: 'utf8', maxBuffer: 50 * 1024 * 1024,
  })
  if (r.status !== 0) throw new Error(`Scan failed: ${r.stderr}`)
  return JSON.parse(r.stdout)
}

async function compareProject(opts: {
  name: string
  projectRoot: string
  srcDir: string
  inputCss: string
  rawConfigLoader: () => Record<string, unknown>
}) {
  const { name, projectRoot, srcDir, inputCss, rawConfigLoader } = opts
  if (!fs.existsSync(projectRoot)) {
    console.log(`${name}: not found at smoke-real/, skipping`)
    return
  }
  let rawConfig: Record<string, unknown>
  try { rawConfig = rawConfigLoader() }
  catch (e: any) { console.log(`${name}: could not load config: ${e.message}`); return }

  const candidates = await scanCandidates(srcDir)
  const loaded = await processRawConfigAsync(rawConfig, null, candidates)
  const config = loaded.resolved as Record<string, unknown>
  const rawNoContent = { ...rawConfig, content: undefined, purge: undefined }

  const [oracle, galeforce] = await Promise.all([
    compileWithTailwind3({ candidates, inputCss, config: rawNoContent }),
    compile({ candidates, inputCss, config } as any),
  ])

  const normO = normalizeCss(oracle.css)
  const normB = normalizeCss(galeforce.css)
  const diffs = diffStylesheets(normB, normO, {})

  console.log(`\n${name}:`)
  console.log(`  Candidates: ${candidates.length}`)
  console.log(`  Oracle: ${oracle.css.length} bytes, ${normO.rules.length} rules`)
  console.log(`  Galeforce: ${galeforce.css.length} bytes, ${normB.rules.length} rules`)
  if (diffs.length === 0) {
    console.log(`  Differences: 0 — FULL PARITY`)
  } else {
    const byKind: Record<string, number> = {}
    for (const d of diffs) byKind[d.kind] = (byKind[d.kind] || 0) + 1
    console.log(`  Differences: ${diffs.length}`, byKind)
    for (const d of diffs.slice(0, 25)) {
      console.log(`    [${d.kind}] ctx=${d.context.join(' / ')} sel=${d.selector}: ${d.detail}`)
    }
  }
}

// soft-ui-dashboard: uses max-width screens + custom plugin with element-selector addComponents
mockPlugins()
await compareProject({
  name: 'Soft UI Dashboard',
  projectRoot: path.join(REPO_ROOT, 'smoke-real/soft-ui-dashboard'),
  srcDir: path.join(REPO_ROOT, 'smoke-real/soft-ui-dashboard/build'),
  inputCss: '@tailwind base;\n@tailwind components;\n@tailwind utilities;\n',
  rawConfigLoader: () => require(path.join(REPO_ROOT, 'smoke-real/soft-ui-dashboard/tailwind.config.js')),
})

// flowbite: uses flowbite plugin.withOptions, mock flowbite-typography
mockPlugins(['flowbite-typography'])
await compareProject({
  name: 'Flowbite',
  projectRoot: path.join(REPO_ROOT, 'smoke-real/flowbite'),
  srcDir: path.join(REPO_ROOT, 'smoke-real/flowbite/src'),
  inputCss: '@tailwind base;\n@tailwind components;\n@tailwind utilities;\n',
  rawConfigLoader: () => require(path.join(REPO_ROOT, 'smoke-real/flowbite/tailwind.config.js')),
})
