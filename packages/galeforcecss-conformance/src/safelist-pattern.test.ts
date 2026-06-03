// Pattern-form safelist integration test. RegExp safelist entries
// can't ride through JSON fixtures (regex isn't serialisable), so this
// lives as a TS test next to the plugin tests.

import { describe, it } from 'vitest'
import { compileWithTailwind3 } from '@cx/galeforcecss-oracle'
import { compile } from '@cx/galeforcecss'
import { processRawConfigAsync } from '@cx/galeforcecss-config-loader'
import { normalizeCss, diffStylesheets, formatDiff } from './index.js'

async function runBoth(opts: {
  raw: Record<string, unknown>
  candidates?: string[]
}): Promise<{ oracle: string; galeforce: string }> {
  const baseConfig = { corePlugins: { preflight: false } }
  const candidates = opts.candidates ?? []
  const oracleResult = await compileWithTailwind3({
    candidates,
    inputCss: '@tailwind utilities;',
    config: { ...opts.raw, ...baseConfig },
  })
  const loaded = await processRawConfigAsync(opts.raw)
  const galeforceResult = await compile({
    candidates,
    inputCss: '@tailwind utilities;',
    config: { ...(loaded.resolved as Record<string, unknown>), ...baseConfig },
  })
  return { oracle: oracleResult.css, galeforce: galeforceResult.css }
}

function assertNoDiff(oracle: string, galeforce: string): void {
  const o = normalizeCss(oracle)
  const b = normalizeCss(galeforce)
  const diff = diffStylesheets(b, o, {})
  if (diff.length > 0) throw new Error(`diff:\n${formatDiff(diff)}`)
}

describe('pattern-form safelist', () => {
  it('flat regex pattern lifts matched classes', async () => {
    const { oracle, galeforce } = await runBoth({
      raw: {
        safelist: [{ pattern: /^bg-(red|blue)-(500|600)$/ }],
      },
    })
    assertNoDiff(oracle, galeforce)
  })

  it('pattern with variants array expands across variants', async () => {
    const { oracle, galeforce } = await runBoth({
      raw: {
        safelist: [{ pattern: /^p-(2|4)$/, variants: ['hover', 'md'] }],
      },
    })
    assertNoDiff(oracle, galeforce)
  })

  it('mix of flat string + pattern entries', async () => {
    const { oracle, galeforce } = await runBoth({
      raw: {
        safelist: ['flex', { pattern: /^m-(1|2)$/ }],
      },
      candidates: ['block'],
    })
    assertNoDiff(oracle, galeforce)
  })
})
