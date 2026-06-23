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

// Semantic diff between two normalized stylesheets.
//
// Output is a human-readable report (newline-separated). The runner asserts
// that the report is empty — if it's non-empty, it's printed as-is so the
// developer can see exactly what diverged.

import type { NormalizedRule, NormalizedStylesheet } from './normalize.js'

export interface DiffOptions {
  /**
   * If true, the order of rules within the same at-rule context must match
   * exactly. Default: false. Tailwind v3's output order is *mostly* stable
   * but not contractually guaranteed for every utility, so most fixtures
   * compare as sets. Conflict-sensitive fixtures (e.g. `p-2 p-4`) opt in.
   */
  orderSensitive?: boolean
}

export interface DiffEntry {
  kind: 'missing' | 'extra' | 'changed'
  context: string[]
  selector: string
  detail: string
}

export function diffStylesheets(
  actual: NormalizedStylesheet,
  expected: NormalizedStylesheet,
  opts: DiffOptions = {},
): DiffEntry[] {
  const entries: DiffEntry[] = []
  const orderSensitive = opts.orderSensitive === true

  if (orderSensitive) {
    diffOrdered(actual.rules, expected.rules, entries, orderSensitive)
  } else {
    diffUnordered(actual.rules, expected.rules, entries, orderSensitive)
  }
  return entries
}

function ruleKey(rule: NormalizedRule): string {
  return rule.context.join(' / ') + ' :: ' + rule.selector
}

function diffUnordered(
  actual: NormalizedRule[],
  expected: NormalizedRule[],
  out: DiffEntry[],
  orderSensitive: boolean,
): void {
  const expectedByKey = groupByKey(expected)
  const actualByKey = groupByKey(actual)

  for (const [key, expectedRules] of expectedByKey) {
    const actualRules = actualByKey.get(key) ?? []
    diffRulesAtKey(key, actualRules, expectedRules, out, orderSensitive)
  }
  for (const [key, actualRules] of actualByKey) {
    if (expectedByKey.has(key)) continue
    for (const rule of actualRules) {
      out.push({
        kind: 'extra',
        context: rule.context,
        selector: rule.selector,
        detail: 'rule present in actual but not expected',
      })
    }
  }
}

function diffOrdered(
  actual: NormalizedRule[],
  expected: NormalizedRule[],
  out: DiffEntry[],
  orderSensitive: boolean,
): void {
  const len = Math.max(actual.length, expected.length)
  for (let i = 0; i < len; i++) {
    const a = actual[i]
    const e = expected[i]
    if (!e && a) {
      out.push({ kind: 'extra', context: a.context, selector: a.selector, detail: `at index ${i}` })
      continue
    }
    if (!a && e) {
      out.push({
        kind: 'missing',
        context: e.context,
        selector: e.selector,
        detail: `at index ${i}`,
      })
      continue
    }
    if (a && e) {
      if (ruleKey(a) !== ruleKey(e)) {
        out.push({
          kind: 'changed',
          context: e.context,
          selector: e.selector,
          detail: `position ${i}: actual was ${ruleKey(a)}`,
        })
        continue
      }
      diffDeclarations(a, e, out, orderSensitive)
    }
  }
}

function groupByKey(rules: NormalizedRule[]): Map<string, NormalizedRule[]> {
  const map = new Map<string, NormalizedRule[]>()
  for (const rule of rules) {
    const key = ruleKey(rule)
    const list = map.get(key)
    if (list) list.push(rule)
    else map.set(key, [rule])
  }
  return map
}

function diffRulesAtKey(
  key: string,
  actual: NormalizedRule[],
  expected: NormalizedRule[],
  out: DiffEntry[],
  orderSensitive: boolean,
): void {
  // Most keys appear once; pair them by index if multiple. Tailwind sometimes
  // emits the same selector twice under different at-rules, but those would
  // already differ by `ruleKey`.
  const len = Math.max(actual.length, expected.length)
  for (let i = 0; i < len; i++) {
    const a = actual[i]
    const e = expected[i]
    if (e && !a) {
      out.push({
        kind: 'missing',
        context: e.context,
        selector: e.selector,
        detail: `selector "${key}" missing from actual`,
      })
      continue
    }
    if (a && !e) {
      out.push({
        kind: 'extra',
        context: a.context,
        selector: a.selector,
        detail: `duplicate selector "${key}" in actual`,
      })
      continue
    }
    if (a && e) diffDeclarations(a, e, out, orderSensitive)
  }
}

function diffDeclarations(
  actual: NormalizedRule,
  expected: NormalizedRule,
  out: DiffEntry[],
  orderSensitive = false,
): void {
  if (orderSensitive) {
    diffDeclarationsOrdered(actual, expected, out)
  } else {
    diffDeclarationsUnordered(actual, expected, out)
  }
}

/** Strict pairwise declaration comparison — catches vendor-prefix
 *  reversals (`-webkit-x; x` vs `x; -webkit-x`) and fallback-order
 *  bugs the multiset version silently passes. */
function diffDeclarationsOrdered(
  actual: NormalizedRule,
  expected: NormalizedRule,
  out: DiffEntry[],
): void {
  const len = Math.max(actual.declarations.length, expected.declarations.length)
  for (let i = 0; i < len; i++) {
    const a = actual.declarations[i]
    const e = expected.declarations[i]
    const ka = a ? `${a.property}:${a.value}${a.important ? '!important' : ''}` : null
    const ke = e ? `${e.property}:${e.value}${e.important ? '!important' : ''}` : null
    if (ka && !ke) {
      out.push({
        kind: 'extra',
        context: actual.context,
        selector: actual.selector,
        detail: `declaration at index ${i}: ${ka}`,
      })
      continue
    }
    if (!ka && ke) {
      out.push({
        kind: 'missing',
        context: expected.context,
        selector: expected.selector,
        detail: `declaration at index ${i}: ${ke}`,
      })
      continue
    }
    if (ka && ke && ka !== ke) {
      out.push({
        kind: 'changed',
        context: expected.context,
        selector: expected.selector,
        detail: `declaration at index ${i}: actual ${ka}, expected ${ke}`,
      })
    }
  }
}

function diffDeclarationsUnordered(
  actual: NormalizedRule,
  expected: NormalizedRule,
  out: DiffEntry[],
): void {
  // Compare declarations as a multiset (property+value+important). We don't
  // assert ordering inside a rule — most rules are order-insensitive, and the
  // ones that aren't (e.g. `--tw-…` cascade) deserve their own ordered fixture.
  const actualMs = new Map<string, number>()
  for (const d of actual.declarations) {
    const k = `${d.property}:${d.value}${d.important ? '!important' : ''}`
    actualMs.set(k, (actualMs.get(k) ?? 0) + 1)
  }
  for (const d of expected.declarations) {
    const k = `${d.property}:${d.value}${d.important ? '!important' : ''}`
    const count = actualMs.get(k) ?? 0
    if (count <= 0) {
      out.push({
        kind: 'missing',
        context: expected.context,
        selector: expected.selector,
        detail: `declaration ${k}`,
      })
    } else {
      actualMs.set(k, count - 1)
    }
  }
  for (const [k, count] of actualMs) {
    if (count <= 0) continue
    for (let i = 0; i < count; i++) {
      out.push({
        kind: 'extra',
        context: actual.context,
        selector: actual.selector,
        detail: `declaration ${k}`,
      })
    }
  }
}

export function formatDiff(entries: DiffEntry[]): string {
  if (entries.length === 0) return ''
  return entries
    .map((e) => {
      const ctx = e.context.length > 0 ? `[${e.context.join(' > ')}] ` : ''
      const tag =
        e.kind === 'missing' ? '- ' : e.kind === 'extra' ? '+ ' : '~ '
      return `${tag}${ctx}${e.selector}  ${e.detail}`
    })
    .join('\n')
}
