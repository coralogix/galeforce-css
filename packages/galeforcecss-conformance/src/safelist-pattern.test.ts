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

// Pattern-form safelist integration test. RegExp safelist entries
// can't ride through JSON fixtures (regex isn't serialisable), so this
// lives as a TS test next to the plugin tests.

import { describe, it } from 'vitest'
import { compileWithTailwind3 } from '@coralogix/galeforcecss-oracle'
import { compile } from '@coralogix/galeforcecss'
import { processRawConfigAsync } from '@coralogix/galeforcecss-config-loader'
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
