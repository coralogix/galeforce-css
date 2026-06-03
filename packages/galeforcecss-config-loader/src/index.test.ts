import { mkdtempSync, writeFileSync, rmSync, mkdirSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { findConfigPath, loadConfig } from './index.js'
import { _resetResolveTailwindCache, resolveTailwind } from './resolve-tailwind.js'

let toClean: string[] = []
afterEach(() => {
  for (const dir of toClean) {
    rmSync(dir, { recursive: true, force: true })
  }
  toClean = []
})

function tempProject(files: Record<string, string>): string {
  const dir = mkdtempSync(join(tmpdir(), 'galeforcecss-cfg-'))
  toClean.push(dir)
  for (const [name, content] of Object.entries(files)) {
    writeFileSync(join(dir, name), content)
  }
  return dir
}

describe('findConfigPath', () => {
  it('returns null when no config file is present', () => {
    const dir = tempProject({})
    expect(findConfigPath(dir)).toBeNull()
  })

  it('prefers .js over later extensions when both are present', () => {
    const dir = tempProject({
      'tailwind.config.js': 'module.exports = {}',
      'tailwind.config.ts': 'export default {}',
    })
    expect(findConfigPath(dir)).toBe(join(dir, 'tailwind.config.js'))
  })
})

describe('loadConfig', () => {
  it('returns the empty-default shape when no config exists', async () => {
    const dir = tempProject({})
    const loaded = await loadConfig({ cwd: dir })
    expect(loaded.path).toBeNull()
    expect(loaded.raw).toEqual({})
    expect(loaded.pluginNames).toEqual([])
    // resolveConfig fills in the default theme — sm screen lives there.
    expect(loaded.resolved.theme).toBeDefined()
    const theme = loaded.resolved.theme as Record<string, unknown>
    expect(theme.screens).toMatchObject({ sm: '640px', md: '768px' })
  })

  it('merges theme.extend over default theme', async () => {
    const dir = tempProject({
      'tailwind.config.js': `module.exports = { theme: { extend: { screens: { '3xl': '1800px' } } } }`,
    })
    const loaded = await loadConfig({ cwd: dir })
    expect(loaded.path).toBe(join(dir, 'tailwind.config.js'))
    const theme = loaded.resolved.theme as Record<string, unknown>
    const screens = theme.screens as Record<string, string>
    expect(screens.sm).toBe('640px')
    expect(screens['3xl']).toBe('1800px')
  })

  it('supports .mjs config via jiti', async () => {
    const dir = tempProject({
      'tailwind.config.mjs': `export default { prefix: 'tw-' }`,
    })
    const loaded = await loadConfig({ cwd: dir })
    expect(loaded.resolved.prefix).toBe('tw-')
  })

  it('strips plugins and surfaces their names', async () => {
    const dir = tempProject({
      'tailwind.config.js': `
        module.exports = {
          plugins: [
            function myPlugin() {},
            function () {},
          ]
        }
      `,
    })
    const loaded = await loadConfig({ cwd: dir })
    expect(loaded.pluginNames).toEqual(['myPlugin', '(anonymous)'])
    // The resolved config should not contain a `plugins` array of functions
    // (they were stripped before resolveConfig). Tailwind's resolver may
    // populate an empty plugins array, which is fine — we just need the
    // result to be JSON-safe.
    expect(JSON.stringify(loaded.resolved)).toBeTypeOf('string')
  })

  it('produces a JSON-safe resolved config', async () => {
    const dir = tempProject({
      'tailwind.config.js': `module.exports = {}`,
    })
    const loaded = await loadConfig({ cwd: dir })
    // Round-trip via JSON; if anything in `resolved` is non-serialisable,
    // this throws.
    expect(() => JSON.parse(JSON.stringify(loaded.resolved))).not.toThrow()
  })

  it('runs plugins declared in a preset (not just the leaf config)', async () => {
    // Mirrors the workspace pattern where a shared base
    // `tailwind.config.ts` registers utility-emitting plugins and
    // per-app configs extend via `presets: [base]`. The plugin
    // output must still be captured — `raw.plugins` is empty at
    // the leaf, the plugins live inside the preset.
    //
    // We use the function-plugin form (vs `tailwindcss/plugin(fn)`)
    // because the temp dir has no node_modules — `require('tailwindcss/plugin')`
    // would fail. The function form is one of the shapes Tailwind
    // accepts: any callable is treated as a plugin's handler.
    const dir = tempProject({
      'base.config.js': `
        module.exports = {
          plugins: [
            function basePlugin({ addUtilities }) {
              addUtilities({ '.border-t-solid': { 'border-top-style': 'solid' } });
            },
          ],
        };
      `,
      'tailwind.config.js': `
        const base = require('./base.config.js');
        module.exports = { presets: [base] };
      `,
    })
    const loaded = await loadConfig({ cwd: dir })
    expect(loaded.pluginOutput.utilities.length).toBeGreaterThan(0)
    const selectors = loaded.pluginOutput.utilities.map((u) => u.selector)
    expect(selectors).toContain('.border-t-solid')
    expect(loaded.pluginNames.length).toBeGreaterThan(0)
  })
})

describe('resolveTailwind', () => {
  afterEach(() => _resetResolveTailwindCache())

  it('falls back to the bundled copy when searchFrom is null', () => {
    const mod = resolveTailwind(null)
    expect(mod.source).toBe('bundled')
    expect(mod.version).toMatch(/^\d+\.\d+\.\d+/)
    expect(typeof mod.resolveConfig).toBe('function')
    expect(typeof mod.tailwindcss).toBe('function')
  })

  it('falls back to bundled when the user dir has no tailwindcss', () => {
    const dir = tempProject({})
    const mod = resolveTailwind(dir)
    expect(mod.source).toBe('bundled')
  })

  it('uses the user`s installed tailwindcss when present', () => {
    // Synthesize a fake project layout with a `tailwindcss` install
    // in its `node_modules`. The fake `resolveConfig` and main
    // module are tagged so we can prove the resolver picked them up
    // instead of the bundled copy.
    const dir = mkdtempSync(join(tmpdir(), 'galeforcecss-twres-'))
    toClean.push(dir)
    const pkgDir = join(dir, 'node_modules', 'tailwindcss')
    mkdirSync(pkgDir, { recursive: true })
    writeFileSync(
      join(pkgDir, 'package.json'),
      JSON.stringify({
        name: 'tailwindcss',
        version: '3.3.99-fake',
        main: 'index.js',
      }),
    )
    writeFileSync(
      join(pkgDir, 'index.js'),
      `function tw() { return { __tag: 'fake-main' } } module.exports = tw`,
    )
    writeFileSync(
      join(pkgDir, 'resolveConfig.js'),
      `module.exports = function fakeResolve(c) { return Object.assign({ __tag: 'fake-resolve' }, c) }`,
    )
    const mod = resolveTailwind(dir)
    expect(mod.source).toBe('user')
    expect(mod.version).toBe('3.3.99-fake')
    const resolved = mod.resolveConfig({ prefix: 'tw-' }) as Record<string, unknown>
    expect(resolved.__tag).toBe('fake-resolve')
    expect(resolved.prefix).toBe('tw-')
  })

  it('caches results per searchFrom', () => {
    const dir = tempProject({})
    const a = resolveTailwind(dir)
    const b = resolveTailwind(dir)
    expect(a).toBe(b)
  })
})
