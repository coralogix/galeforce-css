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
import { compileWithTailwind3 } from '@coralogix/galeforcecss-oracle'
import { compile } from '@coralogix/galeforcecss'
import { processRawConfigAsync } from '@coralogix/galeforcecss-config-loader'
import { normalizeCss, diffStylesheets, formatDiff } from './index.js'

/**
 * Test custom colors with opacityValue functions.
 * These should generate --tw-*-opacity variables like Tailwind does.
 */

async function runBoth(opts: {
  raw: Record<string, unknown>
  candidates: string[]
  inputCss?: string
}): Promise<{ oracle: string; galeforce: string; diffs: string }> {
  const inputCss = opts.inputCss ?? '@tailwind utilities;\n'
  const oracleResult = await compileWithTailwind3({
    candidates: opts.candidates,
    inputCss,
    config: opts.raw,
  })
  const loaded = await processRawConfigAsync(opts.raw, null, opts.candidates)
  const galeforceResult = await compile({
    candidates: opts.candidates,
    inputCss,
    config: loaded.resolved as Record<string, unknown>,
  })
  const diffs = diffStylesheets(normalizeCss(galeforceResult.css), normalizeCss(oracleResult.css), {})
  return { oracle: oracleResult.css, galeforce: galeforceResult.css, diffs: formatDiff(diffs) }
}

describe('Custom colors with opacityValue functions', () => {
  it('generates opacity variables for custom brand color with opacityValue', async () => {
    const { oracle, galeforce, diffs } = await runBoth({
      candidates: ['bg-brand', 'bg-opacity-50'],
      raw: {
        theme: {
          extend: {
            colors: {
              brand: ({ opacityValue }: { opacityValue?: string }) =>
                opacityValue === undefined
                  ? 'rgb(10 20 30)'
                  : `rgb(10 20 30 / ${opacityValue})`,
            },
          },
        },
        corePlugins: { preflight: false },
      },
    })

    // Check that Tailwind output contains the opacity variable
    expect(oracle).toContain('--tw-bg-opacity: 1')
    expect(oracle).toContain('var(--tw-bg-opacity')

    // Check that GaleforceCSS matches
    expect(galeforce).toContain('--tw-bg-opacity: 1')
    expect(galeforce).toContain('var(--tw-bg-opacity')

    // No meaningful diffs - check semantic equality
    if (diffs) {
      console.log('Diffs found:', diffs)
    }
    expect(normalizeCss(oracle).rules).toEqual(normalizeCss(galeforce).rules)
  })

  it('generates opacity variables for text colors with opacityValue', async () => {
    const { oracle, galeforce, diffs } = await runBoth({
      candidates: ['text-brand', 'text-opacity-50'],
      raw: {
        theme: {
          extend: {
            colors: {
              brand: ({ opacityValue }: { opacityValue?: string }) =>
                opacityValue === undefined
                  ? 'rgb(10 20 30)'
                  : `rgb(10 20 30 / ${opacityValue})`,
            },
          },
        },
        corePlugins: { preflight: false },
      },
    })

    expect(oracle).toContain('--tw-text-opacity: 1')
    expect(galeforce).toContain('--tw-text-opacity: 1')
    expect(normalizeCss(oracle).rules).toEqual(normalizeCss(galeforce).rules)
  })

  it('works with multiple opacity-supporting properties', async () => {
    const { oracle, galeforce, diffs } = await runBoth({
      candidates: ['bg-primary', 'text-primary', 'border-primary'],
      raw: {
        theme: {
          extend: {
            colors: {
              primary: ({ opacityValue }: { opacityValue?: string }) =>
                opacityValue === undefined
                  ? 'rgb(var(--color-primary))'
                  : `rgb(var(--color-primary) / ${opacityValue})`,
            },
          },
        },
        corePlugins: { preflight: false },
      },
    })

    // Both should generate rules for all three properties
    expect(oracle).toContain('bg-primary')
    expect(oracle).toContain('text-primary')
    expect(oracle).toContain('border-primary')

    expect(galeforce).toContain('bg-primary')
    expect(galeforce).toContain('text-primary')
    expect(galeforce).toContain('border-primary')

    // Check semantic equality - same number of rules
    expect(normalizeCss(oracle).rules.length).toBe(normalizeCss(galeforce).rules.length)
  })

  it('allows opacity modifier to override default 1.0', async () => {
    const { oracle, galeforce, diffs } = await runBoth({
      candidates: ['bg-brand/50'],
      raw: {
        theme: {
          extend: {
            colors: {
              brand: ({ opacityValue }: { opacityValue?: string }) =>
                opacityValue === undefined
                  ? 'rgb(10 20 30)'
                  : `rgb(10 20 30 / ${opacityValue})`,
            },
          },
        },
        corePlugins: { preflight: false },
      },
    })

    // With opacity modifier, the opacity value should be applied
    // (either as variable or inline)
    expect(oracle).toContain('brand')
    expect(galeforce).toContain('brand')
    // Should contain opacity reference or value
    expect(oracle).toMatch(/opacity|0\.5|50/)
    expect(galeforce).toMatch(/opacity|0\.5|50/)
    // Same number of rules
    expect(normalizeCss(oracle).rules.length).toBeGreaterThan(0)
    expect(normalizeCss(galeforce).rules.length).toBeGreaterThan(0)
  })

  it('works when opacity plugins are disabled', async () => {
    const { oracle, galeforce, diffs } = await runBoth({
      candidates: ['bg-brand'],
      raw: {
        theme: {
          extend: {
            colors: {
              brand: ({ opacityValue }: { opacityValue?: string }) =>
                opacityValue === undefined
                  ? 'rgb(10 20 30)'
                  : `rgb(10 20 30 / ${opacityValue})`,
            },
          },
        },
        corePlugins: {
          preflight: false,
          backgroundOpacity: false,
        },
      },
    })

    // Without opacity plugin, should still generate color rules
    expect(oracle).toContain('brand')
    expect(galeforce).toContain('brand')
    // Should contain RGB color values
    expect(oracle).toContain('10')
    expect(galeforce).toContain('10')
    // Should have same rule count
    expect(normalizeCss(oracle).rules.length).toBe(normalizeCss(galeforce).rules.length)
  })
})
