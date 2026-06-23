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

// Preflight `theme()` override integration test.
//
// Tailwind's preflight CSS embeds four `theme()` calls
// (`borderColor.DEFAULT`, `fontFamily.sans`, `fontFamily.mono`, and a
// placeholder `colors.gray.400`). We vendor the post-`theme()`-
// resolution form for the default config; when a user overrides any
// of those theme paths, the JS-side loader pre-resolves the base
// block via Tailwind itself and ships the result under
// `config.__resolvedBase`. The Rust compiler prefers that when
// present.

import { describe, it } from 'vitest'
import { compileWithTailwind3 } from '@coralogix/galeforcecss-oracle'
import { compile } from '@coralogix/galeforcecss'
import { processRawConfigAsync } from '@coralogix/galeforcecss-config-loader'
import { normalizeCss, diffStylesheets, formatDiff } from './index.js'

async function runBoth(opts: {
  raw: Record<string, unknown>
  inputCss?: string
}): Promise<{ oracle: string; galeforce: string }> {
  const inputCss = opts.inputCss ?? '@tailwind base;\n'
  const oracleResult = await compileWithTailwind3({
    candidates: [],
    inputCss,
    config: opts.raw,
  })
  const loaded = await processRawConfigAsync(opts.raw)
  const galeforceResult = await compile({
    candidates: [],
    inputCss,
    config: loaded.resolved as Record<string, unknown>,
  })
  return { oracle: oracleResult.css, galeforce: galeforceResult.css }
}

function assertNoDiff(oracle: string, galeforce: string): void {
  const o = normalizeCss(oracle)
  const b = normalizeCss(galeforce)
  const diff = diffStylesheets(b, o, {})
  if (diff.length > 0) throw new Error(`diff:\n${formatDiff(diff)}`)
}

describe('preflight theme overrides', () => {
  it('borderColor.DEFAULT override flows into preflight', async () => {
    const { oracle, galeforce } = await runBoth({
      raw: {
        theme: { extend: { borderColor: { DEFAULT: '#ff0000' } } },
      },
    })
    assertNoDiff(oracle, galeforce)
  })

  it('fontFamily.sans override flows into preflight', async () => {
    const { oracle, galeforce } = await runBoth({
      raw: {
        theme: { extend: { fontFamily: { sans: ['Inter', 'system-ui'] } } },
      },
    })
    assertNoDiff(oracle, galeforce)
  })
})
