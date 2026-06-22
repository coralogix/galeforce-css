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

import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { build, type InlineConfig } from 'vite'
import { afterEach, describe, expect, it } from 'vitest'
import galeforcecss from './index.js'

let tmpDirs: string[] = []
afterEach(() => {
  for (const d of tmpDirs) rmSync(d, { recursive: true, force: true })
  tmpDirs = []
})

function project(files: Record<string, string>): string {
  const dir = mkdtempSync(join(tmpdir(), 'vite-galeforce-'))
  tmpDirs.push(dir)
  for (const [name, content] of Object.entries(files)) {
    writeFileSync(join(dir, name), content)
  }
  return dir
}

describe('vite-plugin-galeforcecss', () => {
  it('virtual:galeforcecss.css resolves to compiled CSS during build', async () => {
    const dir = project({
      'main.ts': `import 'virtual:galeforcecss.css'\nexport {}`,
      'styles.css': `body { color: red }\n@tailwind utilities;\n.custom { padding: 1rem }`,
    })
    // Use lib mode so we don't hit Vite's index.html flow (which has tmpdir
    // path issues). The CSS asset still gets emitted alongside the JS bundle.
    const config: InlineConfig = {
      root: dir,
      logLevel: 'silent',
      build: {
        write: false,
        lib: {
          entry: join(dir, 'main.ts'),
          formats: ['es'],
          fileName: 'main',
        },
      },
      plugins: [galeforcecss({ input: 'styles.css' })],
    }
    const out = await build(config)
    const result = Array.isArray(out) ? out[0] : out
    if (!result) throw new Error('no build output')
    if (!('output' in result)) throw new Error('expected RollupOutput')

    // Find the emitted CSS asset.
    const cssAsset = result.output.find(
      (a): a is Extract<typeof a, { type: 'asset' }> =>
        a.type === 'asset' && a.fileName.endsWith('.css'),
    )
    expect(cssAsset).toBeDefined()
    const css = String(cssAsset!.source)
    expect(css).toContain('body')
    expect(css).toContain('.custom')
    // The placeholder is gone — input CSS made it through the directive
    // processor, which is the test we actually care about (the plugin
    // is wired to the real compiler, not the stub).
    expect(css).not.toMatch(/pre-alpha placeholder/)
  })

  it('drop-in: transforms @tailwind directives in CSS imported from JS', async () => {
    // No `input` option, no `virtual:` import — just `import './styles.css'`
    // from the entry. The plugin must intercept the CSS file's content and
    // expand `@tailwind` / `@apply` inline. This mirrors what the Tailwind v3
    // PostCSS plugin does.
    const dir = project({
      'main.ts': `import './styles.css'\nexport const app = '<div class=\"flex p-4 hidden\">x</div>'`,
      'styles.css': `@tailwind utilities;\n.outer { @apply p-4; }\n`,
      'tailwind.config.js': `module.exports = { content: ['./main.ts'] }`,
    })
    const config: InlineConfig = {
      root: dir,
      logLevel: 'silent',
      build: {
        write: false,
        lib: { entry: join(dir, 'main.ts'), formats: ['es'], fileName: 'main' },
      },
      plugins: [galeforcecss({ content: ['./main.ts'] })],
    }
    const out = await build(config)
    const result = Array.isArray(out) ? out[0] : out
    if (!result) throw new Error('no build output')
    if (!('output' in result)) throw new Error('expected RollupOutput')
    const cssAsset = result.output.find(
      (a): a is Extract<typeof a, { type: 'asset' }> =>
        a.type === 'asset' && a.fileName.endsWith('.css'),
    )
    expect(cssAsset).toBeDefined()
    const css = String(cssAsset!.source)
    // Utilities from candidates were emitted in place of `@tailwind utilities`.
    expect(css).toContain('.flex')
    expect(css).toContain('.p-4')
    expect(css).toContain('.hidden')
    // `@apply p-4` resolved to a real `padding: 1rem` declaration.
    expect(css).toContain('.outer')
    expect(css).toContain('padding')
    // No raw `@apply` left behind.
    expect(css).not.toContain('@apply')
  })

  it('drop-in: falls back to tailwind.config.content for scan globs', async () => {
    // No plugin `content` option — should read from the loaded config.
    const dir = project({
      'main.ts': `import './styles.css'\nexport const app = '<div class=\"flex\">x</div>'`,
      'styles.css': `@tailwind utilities;\n`,
      'tailwind.config.js': `module.exports = { content: ['./main.ts'] }`,
    })
    const config: InlineConfig = {
      root: dir,
      logLevel: 'silent',
      build: {
        write: false,
        lib: { entry: join(dir, 'main.ts'), formats: ['es'], fileName: 'main' },
      },
      plugins: [galeforcecss()],
    }
    const out = await build(config)
    const result = Array.isArray(out) ? out[0] : out
    if (!result || !('output' in result)) throw new Error('expected RollupOutput')
    const cssAsset = result.output.find(
      (a): a is Extract<typeof a, { type: 'asset' }> =>
        a.type === 'asset' && a.fileName.endsWith('.css'),
    )
    expect(cssAsset).toBeDefined()
    const css = String(cssAsset!.source)
    expect(css).toContain('.flex')
  })

  it('bypasses user postcss.config.js (does not double-emit utilities)', async () => {
    // Drop a postcss.config.js into the project root containing
    // `tailwindcss` — Vite would normally auto-discover it via
    // postcss-load-config and run tailwindcss on top of galeforce, which
    // would double-emit utility classes (one `.flex` from galeforce, one
    // from Tailwind). Our plugin sets `css.postcss` explicitly, which
    // makes Vite treat it as authoritative and SKIP auto-discovery.
    // This is the core "OXC path doesn't touch postcss.config.js"
    // contract: a legacy postcss.config.js stays in place for the
    // Angular CLI build path, but the Vite path runs only galeforce.
    const dir = project({
      'main.ts': `import './styles.css'\nexport const app = '<div class=\"flex\">x</div>'`,
      'styles.css': `@tailwind utilities;\n`,
      'tailwind.config.js': `module.exports = { content: ['./main.ts'] }`,
      // This file would emit `.flex` if Vite auto-discovered it.
      'postcss.config.js': `module.exports = { plugins: { tailwindcss: { config: './tailwind.config.js' } } }`,
    })
    const config: InlineConfig = {
      root: dir,
      logLevel: 'silent',
      build: {
        write: false,
        lib: { entry: join(dir, 'main.ts'), formats: ['es'], fileName: 'main' },
      },
      plugins: [galeforcecss()],
    }
    const out = await build(config)
    const result = Array.isArray(out) ? out[0] : out
    if (!result || !('output' in result)) throw new Error('expected RollupOutput')
    const cssAsset = result.output.find(
      (a): a is Extract<typeof a, { type: 'asset' }> =>
        a.type === 'asset' && a.fileName.endsWith('.css'),
    )
    expect(cssAsset).toBeDefined()
    const css = String(cssAsset!.source)
    expect(css).toContain('.flex')
    // Each utility appears exactly once — auto-discovered tailwindcss
    // did NOT also run.
    const flexCount = (css.match(/\.flex\s*\{/g) ?? []).length
    expect(flexCount).toBe(1)
  })

  it('runs user-supplied postcssPlugins alongside galeforce', async () => {
    // Users who want autoprefixer (or any other postcss plugin) on the
    // Vite path pass them explicitly via the `postcssPlugins` option.
    // They run AFTER galeforce in the plugin chain.
    const autoprefixer = (await import('autoprefixer')).default
    const dir = project({
      'main.ts': `import './styles.css'\nexport const app = '<div class=\"flex\">x</div>'`,
      'styles.css': `@tailwind utilities;\n.legacy { user-select: none; }\n`,
      'tailwind.config.js': `module.exports = { content: ['./main.ts'] }`,
    })
    const config: InlineConfig = {
      root: dir,
      logLevel: 'silent',
      build: {
        write: false,
        lib: { entry: join(dir, 'main.ts'), formats: ['es'], fileName: 'main' },
      },
      plugins: [
        galeforcecss({
          postcssPlugins: [autoprefixer({ overrideBrowserslist: ['last 5 chrome versions', 'safari >= 14'] })],
        }),
      ],
    }
    const out = await build(config)
    const result = Array.isArray(out) ? out[0] : out
    if (!result || !('output' in result)) throw new Error('expected RollupOutput')
    const cssAsset = result.output.find(
      (a): a is Extract<typeof a, { type: 'asset' }> =>
        a.type === 'asset' && a.fileName.endsWith('.css'),
    )
    expect(cssAsset).toBeDefined()
    const css = String(cssAsset!.source)
    expect(css).toContain('.flex')
    expect(css).toMatch(/-webkit-user-select\s*:\s*none/)
  })

  it('scans content roots described by extglob patterns', async () => {
    // Real Tailwind projects routinely use extglob patterns in
    // `tailwind.config.ts > content` to exclude `*.stories.ts` /
    // `*.spec.ts` while including everything else, e.g.:
    //
    //   './apps/web-app/src/**/!(*.stories|*.spec).{ts,html}'
    //
    // The Rust scanner's `WalkBuilder` treats roots as literal
    // directory paths and can't expand globs itself. Without
    // glob-expansion on the JS side, the candidate set is empty
    // and utilities like `tw-flex` / `tw-items-center` never make
    // it into the output — even when the user writes them in their
    // HTML. We MUST expand the globs before handing files to Rust.
    const dir = project({
      'main.ts': `import 'virtual:galeforcecss.css'\nexport {}`,
      'styles.css': `@tailwind utilities;\n`,
      'app.component.html':
        '<div class="flex items-center justify-center hidden inset-0"></div>',
      'app.component.spec.ts':
        'const SPEC_ONLY_CLASS = "rotate-180"; export {}',
    })
    const config: InlineConfig = {
      root: dir,
      logLevel: 'warn',
      build: {
        write: false,
        lib: {
          entry: join(dir, 'main.ts'),
          formats: ['es'],
          fileName: 'main',
        },
      },
      plugins: [
        galeforcecss({
          input: 'styles.css',
          // The exact shape Tailwind v3 projects use — extglob with an
          // alternation excluding spec/stories.
          content: ['./**/!(*.stories|*.spec).{ts,html}'],
        }),
      ],
    }
    const out = await build(config)
    const result = Array.isArray(out) ? out[0] : out
    if (!result || !('output' in result)) throw new Error('expected RollupOutput')
    const cssAsset = result.output.find(
      (a): a is Extract<typeof a, { type: 'asset' }> =>
        a.type === 'asset' && a.fileName.endsWith('.css'),
    )
    expect(cssAsset).toBeDefined()
    const css = String(cssAsset!.source)
    expect(css, 'must scan extglob-matched .html files').toContain('.flex')
    expect(css, 'must scan extglob-matched .html files').toContain('.items-center')
    expect(css, 'must scan extglob-matched .html files').toContain('.justify-center')
    expect(css, 'must scan extglob-matched .html files').toContain('.hidden')
    expect(css, 'must scan extglob-matched .html files').toContain('.inset-0')
    expect(css, 'must NOT scan spec.ts files excluded by extglob').not.toContain('.rotate-180')
  })

  it('scans content roots and emits utilities for tokens it finds', async () => {
    const dir = project({
      'main.ts': `import 'virtual:galeforcecss.css'\nexport const app = '<div class=\"flex p-4\">' + '<span class=\"hidden\">x</span>' + '</div>'`,
      'styles.css': `@tailwind utilities;\n`,
    })
    const config: InlineConfig = {
      root: dir,
      logLevel: 'silent',
      build: {
        write: false,
        lib: {
          entry: join(dir, 'main.ts'),
          formats: ['es'],
          fileName: 'main',
        },
      },
      plugins: [galeforcecss({ input: 'styles.css' })],
    }
    const out = await build(config)
    const result = Array.isArray(out) ? out[0] : out
    if (!result) throw new Error('no build output')
    if (!('output' in result)) throw new Error('expected RollupOutput')
    const cssAsset = result.output.find(
      (a): a is Extract<typeof a, { type: 'asset' }> =>
        a.type === 'asset' && a.fileName.endsWith('.css'),
    )
    expect(cssAsset).toBeDefined()
    const css = String(cssAsset!.source)
    expect(css).toContain('.flex')
    expect(css).toContain('.p-4')
    expect(css).toContain('.hidden')
  })

  // ── Native CSS nesting in NON-Tailwind files ──────────────────────────────
  //
  // Real-world surfaced this:
  //   - The workspace's legacy `postcss.config.js` loads
  //     `tailwindcss/nesting` BEFORE tailwind, so every CSS file the build
  //     touches gets nesting flattened first.
  //   - This plugin overrides `css.postcss` to be authoritative (only
  //     galeforce) — the legacy postcss.config.js is bypassed.
  //   - The galeforce PostCSS plugin's `OnceExit` early-returns when no
  //     Tailwind directive is present (the cheap-skip path).
  //   - Result: a CSS file containing only native nesting + a `:host(...)`
  //     selector (common in Angular's emulated view encapsulation) reaches
  //     the consuming framework's CSS handler still nested, and the
  //     framework's selector rewriter can't unwrap the `&:hover` form.
  //     Hover rules silently never match, components that depend on hover
  //     state to toggle `display` stay invisible, end-to-end tests fail.
  //
  // Fix contract: when `nesting: true` and the CSS contains an `&`
  // combinator at top-level, route the file through the Rust compiler
  // for nesting flattening even if there are no Tailwind directives.
  // No new postcss dependency — leverage the Rust binary that's already
  // wired up.

  it('flattens native CSS nesting in NON-Tailwind files when nesting:true', async () => {
    const dir = project({
      'main.ts': `import './styles.css'\nexport {}`,
      'styles.css': `
.btn {
  color: red;
  &:hover {
    color: blue;
  }
}
.card {
  & .body {
    padding: 1rem;
  }
}
`,
    })
    const config: InlineConfig = {
      root: dir,
      logLevel: 'silent',
      build: {
        write: false,
        // Vite's own cssMinify uses lightningcss which flattens nesting
        // as a side-effect. Turn it off so the assertions actually
        // measure what the plugin produced.
        cssMinify: false,
        lib: {
          entry: join(dir, 'main.ts'),
          formats: ['es'],
          fileName: 'main',
        },
      },
      plugins: [galeforcecss({ nesting: true })],
    }
    const out = await build(config)
    const result = Array.isArray(out) ? out[0] : out
    if (!result || !('output' in result)) throw new Error('expected RollupOutput')
    const cssAsset = result.output.find(
      (a): a is Extract<typeof a, { type: 'asset' }> =>
        a.type === 'asset' && a.fileName.endsWith('.css'),
    )
    expect(cssAsset).toBeDefined()
    const css = String(cssAsset!.source)
    // Flat form must appear.
    expect(css).toMatch(/\.btn:hover/)
    expect(css).toMatch(/\.card\s+\.body/)
    // Nested form must be gone.
    expect(css).not.toMatch(/&:hover/)
    expect(css).not.toMatch(/&\s+\.body/)
  })

  it('passes nested CSS through unchanged when nesting:false (default)', async () => {
    // Default behavior — projects that don't author nested CSS pay zero
    // cost. We don't try to be clever and flatten on demand; the user
    // opts in explicitly with `nesting: true`. cssMinify off so vite's
    // lightningcss doesn't flatten on our behalf.
    const dir = project({
      'main.ts': `import './styles.css'\nexport {}`,
      'styles.css': `.btn { color: red; &:hover { color: blue; } }`,
    })
    const config: InlineConfig = {
      root: dir,
      logLevel: 'silent',
      build: {
        write: false,
        cssMinify: false,
        lib: {
          entry: join(dir, 'main.ts'),
          formats: ['es'],
          fileName: 'main',
        },
      },
      plugins: [galeforcecss({})],
    }
    const out = await build(config)
    const result = Array.isArray(out) ? out[0] : out
    if (!result || !('output' in result)) throw new Error('expected RollupOutput')
    const cssAsset = result.output.find(
      (a): a is Extract<typeof a, { type: 'asset' }> =>
        a.type === 'asset' && a.fileName.endsWith('.css'),
    )
    expect(cssAsset).toBeDefined()
    const css = String(cssAsset!.source)
    // Without nesting:true, the plugin's flatten step doesn't run.
    // Vite without lightningcss preserves the nesting verbatim.
    expect(css).toMatch(/&:hover/)
  })

  it('still skips non-Tailwind files that have no nesting (fast path preserved)', async () => {
    // The whole reason for the early-return is performance: most CSS
    // files in a real Angular/Vue/React project are plain component
    // styles with no Tailwind and no nesting. They must continue to
    // skip the Rust IPC. We can't observe "Rust wasn't invoked" from
    // build output directly, but we CAN assert the output is byte-for-
    // byte the input (modulo Vite's own CSS handling) — proving no
    // flatten/expand pass ran.
    const dir = project({
      'main.ts': `import './styles.css'\nexport {}`,
      'styles.css': `.btn { color: red; }\n.card { padding: 1rem; }\n`,
    })
    const config: InlineConfig = {
      root: dir,
      logLevel: 'silent',
      build: {
        write: false,
        cssMinify: false,
        lib: {
          entry: join(dir, 'main.ts'),
          formats: ['es'],
          fileName: 'main',
        },
      },
      plugins: [galeforcecss({ nesting: true })],
    }
    const out = await build(config)
    const result = Array.isArray(out) ? out[0] : out
    if (!result || !('output' in result)) throw new Error('expected RollupOutput')
    const cssAsset = result.output.find(
      (a): a is Extract<typeof a, { type: 'asset' }> =>
        a.type === 'asset' && a.fileName.endsWith('.css'),
    )
    expect(cssAsset).toBeDefined()
    const css = String(cssAsset!.source)
    expect(css).toContain('.btn')
    expect(css).toContain('color: red')
    expect(css).toContain('.card')
    expect(css).toContain('padding: 1rem')
  })
})
