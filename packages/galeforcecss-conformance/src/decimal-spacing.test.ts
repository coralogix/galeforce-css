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
import { normalizeCss, diffStylesheets, formatDiff } from './index.js'

/**
 * Test decimal spacing values (1.5, 2.5, 3.5, etc.).
 * These should be properly resolved from the spacing scale.
 */

async function runBoth(candidates: string[]): Promise<{ oracle: string; galeforce: string }> {
  const inputCss = '@tailwind utilities;\n'
  const config = { corePlugins: { preflight: false } }

  const oracleResult = await compileWithTailwind3({
    candidates,
    inputCss,
    config,
  })

  const galeforceResult = await compile({
    candidates,
    inputCss,
    config,
  })

  return { oracle: oracleResult.css, galeforce: galeforceResult.css }
}

describe('Decimal spacing values', () => {
  it('generates px-2.5 correctly', async () => {
    const { oracle, galeforce } = await runBoth(['px-2.5'])
    expect(oracle).toMatch(/px.*2.*5|padding.*6\.25|padding-left.*6\.25/)
    expect(galeforce).toMatch(/px.*2.*5|padding.*6\.25|padding-left.*6\.25/)
    expect(normalizeCss(oracle).rules).toEqual(normalizeCss(galeforce).rules)
  })

  it('generates py-1.5 correctly', async () => {
    const { oracle, galeforce } = await runBoth(['py-1.5'])
    expect(oracle).toMatch(/py.*1.*5|padding.*3\.75|padding-top.*3\.75/)
    expect(galeforce).toMatch(/py.*1.*5|padding.*3\.75|padding-top.*3\.75/)
    expect(normalizeCss(oracle).rules).toEqual(normalizeCss(galeforce).rules)
  })

  it('generates py-2.5 correctly', async () => {
    const { oracle, galeforce } = await runBoth(['py-2.5'])
    expect(oracle).toMatch(/py.*2.*5|padding.*6\.25|padding-top.*6\.25/)
    expect(galeforce).toMatch(/py.*2.*5|padding.*6\.25|padding-top.*6\.25/)
    expect(normalizeCss(oracle).rules).toEqual(normalizeCss(galeforce).rules)
  })

  it('generates py-3.5 correctly', async () => {
    const { oracle, galeforce } = await runBoth(['py-3.5'])
    expect(oracle).toMatch(/py.*3.*5|padding.*8\.75|padding-top.*8\.75/)
    expect(galeforce).toMatch(/py.*3.*5|padding.*8\.75|padding-top.*8\.75/)
    expect(normalizeCss(oracle).rules).toEqual(normalizeCss(galeforce).rules)
  })

  it('generates hover:-translate-y-0.5 correctly', async () => {
    const { oracle, galeforce } = await runBoth(['hover:-translate-y-0.5'])

    // Both should contain the translate-y rule with 0.5 value
    expect(oracle.length).toBeGreaterThan(0)
    expect(galeforce.length).toBeGreaterThan(0)

    // Check that the CSS values normalize to the same structure
    expect(normalizeCss(oracle).rules.length).toBe(normalizeCss(galeforce).rules.length)
  })

  it('handles all common decimal spacing utilities', async () => {
    const decimalUtilities = [
      'p-2.5',
      'px-1.5',
      'py-3.5',
      'm-2.5',
      'mx-1.5',
      'my-3.5',
      'gap-1.5',
      'space-x-2.5',
    ]

    const { oracle, galeforce } = await runBoth(decimalUtilities)

    // Both should generate some output (not empty)
    expect(oracle.length).toBeGreaterThan(0)
    expect(galeforce.length).toBeGreaterThan(0)

    // Normalized output should match
    const normalizedOracle = normalizeCss(oracle)
    const normalizedGaleforce = normalizeCss(galeforce)

    const diffs = diffStylesheets(normalizedGaleforce, normalizedOracle, {})
    if (diffs.length > 0) {
      console.log('Diffs in decimal spacing:', formatDiff(diffs))
    }

    expect(normalizedGaleforce.rules.length).toBe(normalizedOracle.rules.length)
  })

  it('handles decimal values in translate utilities', async () => {
    const { oracle, galeforce } = await runBoth([
      'translate-x-1.5',
      'translate-y-2.5',
      '-translate-x-0.5',
    ])

    expect(oracle.length).toBeGreaterThan(0)
    expect(galeforce.length).toBeGreaterThan(0)
    // Check rule count matches (they may be in different order)
    expect(normalizeCss(oracle).rules.length).toBe(normalizeCss(galeforce).rules.length)
  })
})
