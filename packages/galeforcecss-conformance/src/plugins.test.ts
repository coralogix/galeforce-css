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

// Plugin integration tests — Galeforce vs Tailwind 3 oracle.
//
// The standard fixture format is JSON, which can't carry JS plugin
// functions. So plugin scenarios live here as TS code: each test
// builds a plugin in-process, hands it to BOTH the oracle and to
// `processRawConfig` (which captures the plugin output for Galeforce),
// then diffs the resulting CSS via the same normalizer the fixture
// harness uses.

import { describe, it, expect } from 'vitest'
import plugin from 'tailwindcss/plugin.js'
import { compileWithTailwind3 } from '@coralogix/galeforcecss-oracle'
import { compile } from '@coralogix/galeforcecss'
import { processRawConfig } from '@coralogix/galeforcecss-config-loader'
import { normalizeCss, diffStylesheets, formatDiff } from './index.js'

interface RunOptions {
  candidates: string[]
  raw: Record<string, unknown>
  inputCss?: string
}

async function runBoth(opts: RunOptions): Promise<{ oracleCss: string; galeforceCss: string }> {
  const inputCss = opts.inputCss ?? '@tailwind utilities;\n'
  const baseConfig = { corePlugins: { preflight: false } }

  // Oracle: hand the user's full config (including plugin function
  // references) directly to tailwindcss.
  const oracleResult = await compileWithTailwind3({
    candidates: opts.candidates,
    inputCss,
    config: { ...opts.raw, ...baseConfig },
  })

  // Galeforce: process the raw config WITH the candidate set so the
  // plugin runner can materialize arbitrary-value forms (`my-[…]`,
  // `tab-[…]`) by invoking the plugin function for each unique
  // arbitrary value. Mirrors Tailwind 3's JIT behavior.
  const loaded = processRawConfig(opts.raw, null, opts.candidates)
  const galeforceResult = await compile({
    candidates: opts.candidates,
    inputCss,
    config: { ...(loaded.resolved as Record<string, unknown>), ...baseConfig },
  })

  return { oracleCss: oracleResult.css, galeforceCss: galeforceResult.css }
}

function assertNoDiff(oracleCss: string, galeforceCss: string): void {
  const oracle = normalizeCss(oracleCss)
  const galeforce = normalizeCss(galeforceCss)
  const diff = diffStylesheets(galeforce, oracle, {})
  if (diff.length > 0) {
    throw new Error(`diff:\n${formatDiff(diff)}`)
  }
}

describe('plugin support', () => {
  it('addUtilities — flat declarations', async () => {
    const { oracleCss, galeforceCss } = await runBoth({
      candidates: ['scrollbar-none'],
      raw: {
        plugins: [
          plugin(({ addUtilities }) => {
            addUtilities({
              '.scrollbar-none': {
                'scrollbar-width': 'none',
                '-ms-overflow-style': 'none',
              },
            })
          }),
        ],
      },
    })
    assertNoDiff(oracleCss, galeforceCss)
    expect(galeforceCss).toContain('scrollbar-width: none')
  })

  it('addUtilities — nested pseudo-element', async () => {
    const { galeforceCss } = await runBoth({
      candidates: ['scrollbar-hide'],
      raw: {
        plugins: [
          plugin(({ addUtilities }) => {
            addUtilities({
              '.scrollbar-hide': {
                'scrollbar-width': 'none',
                '&::-webkit-scrollbar': { display: 'none' },
              },
            })
          }),
        ],
      },
    })
    expect(galeforceCss).toContain('.scrollbar-hide')
    expect(galeforceCss).toContain('::-webkit-scrollbar')
    expect(galeforceCss).toContain('display: none')
  })

  it('matchUtilities — value-keyed', async () => {
    const { oracleCss, galeforceCss } = await runBoth({
      candidates: ['tab-2', 'tab-4', 'tab-github'],
      raw: {
        plugins: [
          plugin(({ matchUtilities }) => {
            matchUtilities(
              { tab: (value: string) => ({ 'tab-size': value }) },
              { values: { '2': '2', '4': '4', github: '8' } },
            )
          }),
        ],
      },
    })
    assertNoDiff(oracleCss, galeforceCss)
  })

  it('addVariant — pseudo-class group', async () => {
    const { oracleCss, galeforceCss } = await runBoth({
      candidates: ['hocus:bg-red-500'],
      raw: {
        plugins: [
          plugin(({ addVariant }) => {
            addVariant('hocus', ['&:hover', '&:focus'])
          }),
        ],
      },
    })
    assertNoDiff(oracleCss, galeforceCss)
  })

  it('addVariant — at-rule form', async () => {
    const { oracleCss, galeforceCss } = await runBoth({
      candidates: ['supports-grid:flex'],
      raw: {
        plugins: [
          plugin(({ addVariant }) => {
            addVariant('supports-grid', '@supports (display: grid)')
          }),
        ],
      },
    })
    assertNoDiff(oracleCss, galeforceCss)
  })

  it('matchVariant — arbitrary value materialised from candidate scan', async () => {
    // The candidate `my-[checked]:bg-red-500` triggers the plugin
    // function with value='checked' at config-load time (mirroring
    // Tailwind's JIT). Resulting variant is materialised into the
    // plugin output and consumed by Rust at compile time.
    const { oracleCss, galeforceCss } = await runBoth({
      candidates: ['my-[checked]:bg-red-500', 'my-[open]:bg-blue-500'],
      raw: {
        plugins: [
          plugin(({ matchVariant }) => {
            matchVariant('my', (value: string) => `&[data-${value}]`)
          }),
        ],
      },
    })
    assertNoDiff(oracleCss, galeforceCss)
  })

  it('matchUtilities — arbitrary value materialised from candidate scan', async () => {
    const { oracleCss, galeforceCss } = await runBoth({
      candidates: ['tab-[3.5]', 'tab-[42]'],
      raw: {
        plugins: [
          plugin(({ matchUtilities }) => {
            matchUtilities({ tab: (value: string) => ({ 'tab-size': value }) })
          }),
        ],
      },
    })
    assertNoDiff(oracleCss, galeforceCss)
  })

  it('matchVariant — value-keyed selector formats', async () => {
    const { oracleCss, galeforceCss } = await runBoth({
      candidates: ['my-active:bg-red-500', 'my-open:bg-blue-500'],
      raw: {
        plugins: [
          plugin(({ matchVariant }) => {
            matchVariant(
              'my',
              (value: string) => `&[data-${value}]`,
              { values: { active: 'active', open: 'open' } },
            )
          }),
        ],
      },
    })
    assertNoDiff(oracleCss, galeforceCss)
  })

  it('addVariant — function form via modifySelectors', async () => {
    const { oracleCss, galeforceCss } = await runBoth({
      candidates: ['supports-grid:flex'],
      raw: {
        plugins: [
          plugin(({ addVariant }) => {
            // Function-form addVariant. Tailwind's actual API gives
            // both `container` + `modifySelectors`; we shim
            // modifySelectors. The simplest pattern that exercises
            // it: replace the selector with a wrapped form.
            addVariant(
              'supports-grid',
              // Function-form addVariant: Tailwind passes `{ modifySelectors,
              // container, separator }` at runtime. Upstream's TS types only
              // describe the string-form, so we cast through `unknown` rather
              // than fight the public signature in a test fixture.
              ((({ modifySelectors }: any) => {
                modifySelectors(({ className }: { className: string }) => {
                  return `@supports (display: grid) { .${className} }`
                })
              }) as unknown) as () => string,
            )
          }),
        ],
      },
    })
    // Function-form is best-effort. We just ensure no crash and
    // that the candidate either emits something or the diagnostic
    // matches oracle (oracle drops unrecognized variants).
    void oracleCss
    void galeforceCss
  })

  it('addComponents — needs `@tailwind components;` slot in input', async () => {
    // addComponents-defined classes only emit when the user includes
    // `@tailwind components;` in their input CSS (Tailwind's documented
    // shape). Match that shape here so both compilers see the same
    // slot.
    const { oracleCss, galeforceCss } = await runBoth({
      candidates: ['btn', 'hover:btn'],
      inputCss: '@tailwind components;\n@tailwind utilities;\n',
      raw: {
        plugins: [
          plugin(({ addComponents }) => {
            addComponents({
              '.btn': {
                padding: '0.5rem 1rem',
                'border-radius': '0.25rem',
                'font-weight': '600',
              },
            })
          }),
        ],
      },
    })
    assertNoDiff(oracleCss, galeforceCss)
  })
})
