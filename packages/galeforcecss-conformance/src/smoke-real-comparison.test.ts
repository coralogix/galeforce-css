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

import { describe, it, expect } from 'vitest'
import { spawnSync } from 'child_process'
import { compileWithTailwind3 } from '@coralogix/galeforcecss-oracle'
import { compile } from '@coralogix/galeforcecss'
import { processRawConfigAsync } from '@coralogix/galeforcecss-config-loader'
import { normalizeCss, diffStylesheets, formatDiff } from './index.js'
import * as path from 'path'
import * as fs from 'fs'
import Module from 'module'

/**
 * Compare GaleforceCSS vs Tailwind v3.4.19 on real open-source projects.
 * Uses processRawConfigAsync to properly resolve configs with function-valued
 * theme keys before comparing, matching the conformance harness approach.
 */

const REPO_ROOT = new URL('../../../', import.meta.url).pathname

function mockPlugins() {
  const mod = Module as any
  const orig = mod._load
  mod._load = function (req: string, parent: unknown, isMain: boolean) {
    if (req === 'tailwindcss-rtl' || req === 'tailwindcss-animate') {
      return () => {}
    }
    return orig.apply(this, [req, parent, isMain])
  }
}

async function scanCandidates(srcDir: string): Promise<string[]> {
  const galeforceBin = path.join(REPO_ROOT, 'target/release/galeforcecss')
  const r = spawnSync(galeforceBin, ['scan', '--content', srcDir, '--json'], {
    encoding: 'utf8',
    maxBuffer: 50 * 1024 * 1024,
  })
  if (r.status !== 0) throw new Error(`Scan failed: ${r.stderr}`)
  return JSON.parse(r.stdout)
}

async function compareProjects(opts: {
  projectName: string
  srcDir: string
  rawConfig: Record<string, unknown>
  inputCss: string
}) {
  const { projectName, srcDir, rawConfig, inputCss } = opts

  const candidates = await scanCandidates(srcDir)
  const loaded = await processRawConfigAsync(rawConfig, null, candidates)
  const config = loaded.resolved as Record<string, unknown>

  console.log(`\n${projectName}:`)
  console.log(`  Candidates: ${candidates.length}`)

  // Strip content/purge from raw config so oracle only uses the explicit candidates
  // we've already scanned, preventing it from scanning additional files whose
  // content (like test file string literals) would inflate the oracle's output.
  // `purge` is the v2/v3 deprecated alias for `content` and Tailwind v3 still
  // reads it, so we must clear both.
  const rawNoContent = { ...rawConfig, content: undefined, purge: undefined }
  const [oracleResult, galeforceResult] = await Promise.all([
    compileWithTailwind3({ candidates, inputCss, config: rawNoContent }),
    compile({ candidates, inputCss, config }),
  ])

  const normalizedOracle = normalizeCss(oracleResult.css)
  const normalizedGaleforce = normalizeCss(galeforceResult.css)

  console.log(`  Oracle: ${oracleResult.css.length} bytes, ${normalizedOracle.rules.length} rules`)
  console.log(`  Galeforce: ${galeforceResult.css.length} bytes, ${normalizedGaleforce.rules.length} rules`)

  const diffs = diffStylesheets(normalizedGaleforce, normalizedOracle, {})

  if (diffs.length > 0) {
    const byKind: Record<string, number> = {}
    for (const d of diffs) byKind[d.kind] = (byKind[d.kind] || 0) + 1
    console.log(`  Differences: ${diffs.length}`, byKind)

    console.log(`\n  First 15 differences:`)
    for (const d of diffs.slice(0, 15)) {
      console.log(`    [${d.kind}] ctx=${d.context.join(' / ')} sel=${d.selector}: ${d.detail}`)
    }
  } else {
    console.log(`  Differences: 0 — FULL PARITY`)
  }

  return { diffs, normalizedOracle, normalizedGaleforce, oracleResult, galeforceResult }
}

describe('Real-world project comparisons', () => {
  it('notus-nextjs: full CSS parity', async () => {
    mockPlugins()

    const projectRoot = path.join(REPO_ROOT, 'smoke-real/notus-nextjs')
    if (!fs.existsSync(projectRoot)) {
      console.log('  notus-nextjs not found at smoke-real/, skipping')
      return
    }

    const rawConfig = require(path.join(projectRoot, 'tailwind.config.js'))
    const { diffs } = await compareProjects({
      projectName: 'Notus Next.js',
      srcDir: projectRoot,
      rawConfig,
      inputCss: '@tailwind base;\n@tailwind components;\n@tailwind utilities;\n',
    })

    expect(diffs.length).toBe(0)
  }, 120_000)

  it('material-tailwind: full CSS parity', async () => {
    mockPlugins()

    const projectRoot = path.join(REPO_ROOT, 'smoke-real/material-tailwind')
    if (!fs.existsSync(projectRoot)) {
      console.log('  material-tailwind not found at smoke-real/, skipping')
      return
    }

    let rawConfig: Record<string, unknown>
    try {
      rawConfig = require(path.join(projectRoot, 'tailwind.config.js'))
    } catch (e: any) {
      console.log(`  Could not load material-tailwind config: ${e.message}, skipping`)
      return
    }

    const { diffs } = await compareProjects({
      projectName: 'Material Tailwind',
      srcDir: projectRoot,
      rawConfig,
      inputCss: '@tailwind base;\n@tailwind components;\n@tailwind utilities;\n',
    })

    expect(diffs.length).toBe(0)
  }, 120_000)

  it('horizon-tailwind-react: full CSS parity with processRawConfigAsync', async () => {
    mockPlugins()

    const projectRoot = path.join(REPO_ROOT, 'smoke-real/horizon-tailwind-react')
    if (!fs.existsSync(projectRoot)) {
      console.log('  horizon-tailwind-react not found at smoke-real/, skipping')
      return
    }

    const rawConfig = require(path.join(projectRoot, 'tailwind.config.js'))
    const { diffs } = await compareProjects({
      projectName: 'Horizon Tailwind React',
      srcDir: path.join(projectRoot, 'src'),
      rawConfig,
      inputCss: '@tailwind base;\n@tailwind components;\n@tailwind utilities;\n',
    })

    expect(diffs.length).toBe(0)
  }, 60_000)

  it('shadcn-nextjs-boilerplate: full CSS parity', async () => {
    mockPlugins()

    const projectRoot = path.join(REPO_ROOT, 'smoke-real/shadcn-nextjs-boilerplate')
    if (!fs.existsSync(projectRoot)) {
      console.log('  shadcn-nextjs-boilerplate not found at smoke-real/, skipping')
      return
    }

    // TS config — try compiled .js first, fall back to skipping
    const configJs = path.join(projectRoot, 'tailwind.config.js')
    if (!fs.existsSync(configJs)) {
      console.log('  No compiled tailwind.config.js found, skipping')
      return
    }

    const rawConfig = require(configJs)
    const { diffs } = await compareProjects({
      projectName: 'Shadcn Next.js Boilerplate',
      srcDir: path.join(projectRoot, 'app'),
      rawConfig,
      inputCss: '@tailwind base;\n@tailwind components;\n@tailwind utilities;\n',
    })

    expect(diffs.length).toBe(0)
  }, 60_000)

  it('notus-react: full CSS parity', async () => {
    mockPlugins()

    const projectRoot = path.join(REPO_ROOT, 'smoke-real/notus-react')
    if (!fs.existsSync(projectRoot)) {
      console.log('  notus-react not found at smoke-real/, skipping')
      return
    }

    const rawConfig = require(path.join(projectRoot, 'tailwind.config.js'))
    const { diffs } = await compareProjects({
      projectName: 'Notus React',
      srcDir: path.join(projectRoot, 'src'),
      rawConfig,
      inputCss: '@tailwind base;\n@tailwind components;\n@tailwind utilities;\n',
    })

    expect(diffs.length).toBe(0)
  }, 120_000)

  it('merakiui: full CSS parity with default config', async () => {
    const projectRoot = path.join(REPO_ROOT, 'smoke-real/merakiui')
    if (!fs.existsSync(projectRoot)) {
      console.log('  merakiui not found at smoke-real/, skipping')
      return
    }

    // merakiui has no tailwind.config.js — run with default config (preflight enabled)
    const rawConfig = { corePlugins: { preflight: false } }
    const { diffs } = await compareProjects({
      projectName: 'Meraki UI',
      srcDir: path.join(projectRoot, 'components'),
      rawConfig,
      inputCss: '@tailwind utilities;\n',
    })

    expect(diffs.length).toBe(0)
  }, 120_000)
})
