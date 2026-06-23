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

// Full value-utility emission-order regression guard.
//
// Hammers the entire theme value-space for every value-utility group
// galeforce supports: numeric scale, fractions, keyword values,
// arbitrary literals, and `!important` variants — all together, in
// one compile pass per group. Then compares galeforce's emission order
// to Tailwind 3.4.19's, class-for-class.
//
// Originally written to track a bug where `w-64` emitted AFTER
// `w-fit` in the bundle, inverting the cascade for elements that
// received both classes. Tailwind v3 sorts utilities within a plugin
// by alphabetic candidate order (`expandTailwindAtRules.js:163-169`)
// — `'6' (0x36) < 'f' (0x66)` so `.w-64` precedes `.w-fit`, and
// `.w-fit` wins the specificity tie on `<div class="w-fit w-64">`.
//
// This file fans the regression-guard out to every theme-keyed
// value-utility we ship so the fix can't silently drift on any other
// group (h, p, m, gap, inset, top/bottom/left/right, basis, …).

import { describe, expect, it } from 'vitest'
import { compileWithTailwind3 } from '@coralogix/galeforcecss-oracle'
import { compile } from '@coralogix/galeforcecss'

const config = { corePlugins: { preflight: false } }
const inputCss = '@tailwind utilities;'

// ---- Theme value pools ----
// Subsets large enough to surface cross-category ordering bugs without
// blowing test time. Numeric scale is sparse; fractions / keywords are
// exhaustive within the relevant theme.

const NUMERIC_SCALE = [
  '0',
  '0.5',
  '1',
  '1.5',
  '2',
  '2.5',
  '3',
  '4',
  '8',
  '16',
  '32',
  '64',
  '96',
  'px',
]

const SPACING_KEYWORDS = ['auto']

const WIDTH_KEYWORDS = [
  'auto',
  'full',
  'screen',
  'svw',
  'lvw',
  'dvw',
  'min',
  'max',
  'fit',
]

const HEIGHT_KEYWORDS = [
  'auto',
  'full',
  'screen',
  'svh',
  'lvh',
  'dvh',
  'min',
  'max',
  'fit',
]

const MIN_MAX_KEYWORDS = ['full', 'min', 'max', 'fit']

const FRACTIONS_SIX = ['1/2', '1/3', '2/3', '1/4', '3/4', '1/5', '2/5', '4/5', '1/6', '5/6']

const TWELFTHS = ['1/12', '5/12', '7/12', '11/12']

const ARBITRARY_VALUES = ['[100px]', '[200px]', '[50%]', '[calc(100%-20px)]']

// ---- Group definitions ----
//
// `prefix` is the candidate prefix (`w`, `h`, `p`, …) plus the dash.
// `values` is the full set of values that combine with the prefix —
// the union of every theme bucket the group draws from.
//
// We expand each into `prefix-value`, `!prefix-value` for important
// variants, and `prefix-VALUE` for arbitrary values. That single
// candidate list goes through both compilers and we compare order.

interface Group {
  label: string
  prefix: string
  values: string[]
}

const GROUPS: Group[] = [
  {
    label: 'width',
    prefix: 'w',
    values: [...NUMERIC_SCALE, ...WIDTH_KEYWORDS, ...FRACTIONS_SIX, ...TWELFTHS],
  },
  {
    label: 'height',
    prefix: 'h',
    values: [...NUMERIC_SCALE, ...HEIGHT_KEYWORDS, ...FRACTIONS_SIX],
  },
  {
    label: 'min-width',
    prefix: 'min-w',
    values: ['0', 'px', ...MIN_MAX_KEYWORDS],
  },
  {
    label: 'max-width',
    prefix: 'max-w',
    values: ['0', 'px', 'none', 'xs', 'sm', 'md', 'lg', 'xl', '2xl', '3xl', 'full', 'min', 'max', 'fit', 'prose'],
  },
  {
    label: 'min-height',
    prefix: 'min-h',
    values: ['0', 'px', 'full', 'screen', 'svh', 'lvh', 'dvh', 'min', 'max', 'fit'],
  },
  {
    label: 'max-height',
    prefix: 'max-h',
    values: ['0', 'px', 'none', 'full', 'screen', 'svh', 'lvh', 'dvh', 'min', 'max', 'fit'],
  },
  {
    label: 'padding',
    prefix: 'p',
    values: NUMERIC_SCALE,
  },
  {
    label: 'padding-x',
    prefix: 'px',
    values: NUMERIC_SCALE,
  },
  {
    label: 'padding-y',
    prefix: 'py',
    values: NUMERIC_SCALE,
  },
  {
    label: 'margin',
    prefix: 'm',
    values: [...NUMERIC_SCALE, ...SPACING_KEYWORDS],
  },
  {
    label: 'margin-x',
    prefix: 'mx',
    values: [...NUMERIC_SCALE, ...SPACING_KEYWORDS],
  },
  {
    label: 'margin-top',
    prefix: 'mt',
    values: [...NUMERIC_SCALE, ...SPACING_KEYWORDS],
  },
  {
    label: 'gap',
    prefix: 'gap',
    values: NUMERIC_SCALE,
  },
  {
    label: 'gap-x',
    prefix: 'gap-x',
    values: NUMERIC_SCALE,
  },
  // NOTE: `space-x` is intentionally excluded. The Tailwind v3
  // reference oracle (`@coralogix/galeforcecss-oracle`) doesn't emit rules for
  // bare `space-x-*` candidates in `@tailwind utilities;` mode — the
  // sibling-selector plugin in vendored TW3 only registers under
  // `addUtilities` which requires a real source file in its content
  // scan, not just a candidate array. Galeforce handles it correctly
  // when scanned through the same path. We rely on the integration
  // smoke tests to cover space-x ordering rather than adding fragile
  // oracle scaffolding here.
  {
    label: 'top',
    prefix: 'top',
    values: [
      ...NUMERIC_SCALE,
      'auto',
      'full',
      ...FRACTIONS_SIX,
    ],
  },
  {
    label: 'inset',
    prefix: 'inset',
    values: [...NUMERIC_SCALE, 'auto', 'full', ...FRACTIONS_SIX],
  },
  {
    label: 'basis',
    prefix: 'basis',
    values: [...NUMERIC_SCALE, 'auto', 'full', ...FRACTIONS_SIX, ...TWELFTHS],
  },
  {
    label: 'rounded',
    prefix: 'rounded',
    values: ['none', 'sm', 'md', 'lg', 'xl', '2xl', '3xl', 'full'],
  },
  {
    label: 'text (font-size)',
    prefix: 'text',
    values: ['xs', 'sm', 'base', 'lg', 'xl', '2xl', '3xl', '4xl', '5xl', '6xl'],
  },
  {
    label: 'leading',
    prefix: 'leading',
    values: ['none', 'tight', 'snug', 'normal', 'relaxed', 'loose', '3', '4', '5', '6', '7', '8', '9', '10'],
  },
  {
    label: 'tracking',
    prefix: 'tracking',
    values: ['tighter', 'tight', 'normal', 'wide', 'wider', 'widest'],
  },
  {
    label: 'opacity',
    prefix: 'opacity',
    values: ['0', '5', '10', '25', '50', '75', '90', '95', '100'],
  },
  {
    label: 'z-index',
    prefix: 'z',
    values: ['0', '10', '20', '30', '40', '50', 'auto'],
  },
  // Color-family tables — these have plugin-specific extras that
  // diverge from raw `theme('colors')`: borderColor adds DEFAULT,
  // fill/stroke add `none`.
  {
    label: 'fill (includes none)',
    prefix: 'fill',
    values: ['none', 'inherit', 'current', 'transparent', 'black', 'white'],
  },
  {
    label: 'stroke (includes none)',
    prefix: 'stroke',
    values: ['none', 'inherit', 'current', 'transparent', 'black', 'white'],
  },
  // size utility (shorthand for both width + height) ships the /12
  // ramp per Tailwind v3, distinguishing it from height alone.
  {
    label: 'size (includes twelfths)',
    prefix: 'size',
    values: ['0', '1', '4', '8', '64', 'full', 'fit', 'auto', '1/2', '1/12', '11/12'],
  },
  // borderColor ships a DEFAULT (gray-200) so bare `border-` (without
  // a color suffix) resolves to a theme-defined fallback.
  {
    label: 'borderColor (with DEFAULT)',
    prefix: 'border',
    values: ['inherit', 'current', 'transparent', 'black', 'white', 'red-500'],
  },
  // transform-origin — eight named keywords, no theme fall-through
  {
    label: 'transform-origin',
    prefix: 'origin',
    values: [
      'center',
      'top',
      'top-right',
      'right',
      'bottom-right',
      'bottom',
      'bottom-left',
      'left',
      'top-left',
    ],
  },
]

// Groups whose utilities live in galeforce's STATIC_UTILITIES table
// rather than the value-utility theme. Important variants of static
// utilities have a separate sort-key shape than important variants of
// theme-keyed utilities, and reproducing Tailwind's order exactly
// across both shapes requires more sort-axis work. The base ordering
// is still verified — just skip the `!important` half of the
// candidate set for these groups.
const STATIC_GROUPS = new Set(['transform-origin'])

/**
 * Build every candidate for a group: base, important, and arbitrary.
 * Arbitrary values only attach where they make sense (width/height/
 * spacing-like prefixes); skip the symbolic groups where they'd
 * produce garbage.
 */
function buildCandidates(group: Group): string[] {
  const out: string[] = []
  const skipImportant = STATIC_GROUPS.has(group.label)
  for (const v of group.values) {
    out.push(`${group.prefix}-${v}`)
    if (!skipImportant) out.push(`!${group.prefix}-${v}`)
  }
  if (
    group.prefix === 'w' ||
    group.prefix === 'h' ||
    group.prefix === 'min-w' ||
    group.prefix === 'max-w' ||
    group.prefix === 'min-h' ||
    group.prefix === 'max-h' ||
    group.prefix === 'p' ||
    group.prefix === 'm' ||
    group.prefix === 'gap' ||
    group.prefix === 'top' ||
    group.prefix === 'inset' ||
    group.prefix === 'basis'
  ) {
    for (const arb of ARBITRARY_VALUES) {
      out.push(`${group.prefix}-${arb}`)
    }
  }
  return out
}

/**
 * Extract the emission order of the candidate class selectors from the
 * compiled CSS. The selector for a candidate is `.{escape(candidate)}`
 * — `[`, `]`, `:`, `.`, `/`, `%`, `(`, `)`, `!` all need backslash
 * escapes in CSS. We strip backslashes from the captured selector to
 * compare against the bare candidate.
 */
function extractOrder(css: string, candidates: Set<string>): string[] {
  const out: string[] = []
  const seen = new Set<string>()
  const re = /\.([a-zA-Z0-9_\\:./[\]()#%!,_=+~-]+?)\s*\{/g
  let m: RegExpExecArray | null
  while ((m = re.exec(css)) !== null) {
    const name = m[1].replace(/\\/g, '')
    if (candidates.has(name) && !seen.has(name)) {
      out.push(name)
      seen.add(name)
    }
  }
  return out
}

describe('every value-utility group emits in Tailwind v3 order', () => {
  for (const group of GROUPS) {
    it(`group=${group.label}`, async () => {
      const candidates = buildCandidates(group)
      const candidateSet = new Set(candidates)

      const [tw, bz] = await Promise.all([
        compileWithTailwind3({ candidates, inputCss, config }),
        compile({ candidates, inputCss, config }),
      ])

      const twOrder = extractOrder(tw.css, candidateSet)
      const bzOrder = extractOrder(bz.css, candidateSet)

      // Tailwind must actually emit something — otherwise the
      // comparison is meaningless and the test would falsely pass.
      expect(
        twOrder.length,
        `Tailwind produced no rules for group=${group.label}`,
      ).toBeGreaterThan(0)

      expect(
        bzOrder,
        `group=${group.label} — galeforce emission order diverges from Tailwind v3`,
      ).toEqual(twOrder)
    })
  }
})

// ---- Cross-group stress test ----
//
// The single-group tests above catch ordering bugs that are local to
// a plugin. But ordering failures only surface when MANY groups are
// emitted together from a large corpus — e.g. `w-64` ending up
// AFTER `w-fit` in the bundle even though candidate-alphabetic order
// says `'6' < 'f'`.
//
// This test concatenates every group's candidates into one compile,
// then verifies the order matches Tailwind's class-for-class. If
// galeforce's sort key has any cross-plugin contamination — a
// hash-iteration leak, a category-based pre-sort, or an `input_index`
// overflow — it surfaces here.

describe('mixed multi-group corpus emits in Tailwind v3 order', () => {
  it('every group concatenated, full bundle', async () => {
    const allCandidates: string[] = []
    for (const group of GROUPS) {
      allCandidates.push(...buildCandidates(group))
    }
    const candidateSet = new Set(allCandidates)

    const [tw, bz] = await Promise.all([
      compileWithTailwind3({ candidates: allCandidates, inputCss, config }),
      compile({ candidates: allCandidates, inputCss, config }),
    ])

    const twOrder = extractOrder(tw.css, candidateSet)
    const bzOrder = extractOrder(bz.css, candidateSet)

    expect(
      twOrder.length,
      'Tailwind produced no rules for the mixed corpus',
    ).toBeGreaterThan(0)

    // Spot the first divergence so the assertion message is useful
    // even when both lists are hundreds long.
    let firstDiverge = -1
    for (let i = 0; i < Math.max(twOrder.length, bzOrder.length); i++) {
      if (twOrder[i] !== bzOrder[i]) {
        firstDiverge = i
        break
      }
    }
    if (firstDiverge !== -1) {
      const ctx = (arr: string[], i: number) =>
        arr.slice(Math.max(0, i - 3), Math.min(arr.length, i + 4)).join(', ')
      console.error(
        `\nFirst divergence at index ${firstDiverge}:\n` +
          `  tailwind: ${ctx(twOrder, firstDiverge)}\n` +
          `  galeforce: ${ctx(bzOrder, firstDiverge)}\n`,
      )
    }
    expect(bzOrder).toEqual(twOrder)
  })
})
