// Value-utility emission-order regression guard.
//
// `tw-w-64` and `tw-w-fit` both come from the `width` plugin via
// `createUtilityPlugin` → `matchUtilities`. Tailwind v3 registers them
// under a single `Offsets.create('utilities')` slot, then orders them
// by the alphabetic candidate sort done in `expandTailwindAtRules.js`.
// `'6' (0x36) < 'f' (0x66)` so `.w-64` emits BEFORE `.w-fit` and the
// later `.w-fit` wins the cascade tie when both classes coexist on
// one element.
//
// Galeforce mirrors this by packing the alphabetic-presort rank into
// `input_index` and using it as the final tiebreaker in
// `RuleOffset.index`. The bug this test guards against: the
// `input_index` slot was sized too small in `build_rule_offset`
// (`crates/galeforce-compiler/src/lib.rs`). When the candidate count
// exceeds the slot capacity, distinct candidates collide on
// `input_index & mask` and ties resolve via the par_iter scheduling
// order — non-deterministic across builds.
//
// PROBES exercises mixed numeric/keyword pairs at small candidate
// counts (a sanity floor). OVERFLOW exercises the same pairs with a
// large padding cohort so `input_index` exceeds the historical 4096
// cap. Both run against the @cx/galeforcecss-oracle Tailwind reference
// — a divergence on either fails the test.

import { describe, expect, it } from 'vitest'
import { compileWithTailwind3 } from '@cx/galeforcecss-oracle'
import { compile } from '@cx/galeforcecss'

const config = { corePlugins: { preflight: false } }
const inputCss = '@tailwind utilities;'

interface Probe {
  label: string
  candidates: string[]
}

const PROBES: Probe[] = [
  {
    label: 'width: numeric vs fit',
    candidates: ['w-64', 'w-fit'],
  },
  {
    label: 'width: keyword spread',
    candidates: ['w-auto', 'w-full', 'w-screen', 'w-min', 'w-max', 'w-fit'],
  },
  {
    label: 'width: mixed numeric + keyword',
    candidates: ['w-0', 'w-1', 'w-64', 'w-96', 'w-full', 'w-fit', 'w-auto'],
  },
  {
    label: 'width: numeric scale',
    candidates: ['w-0', 'w-1', 'w-2', 'w-4', 'w-8', 'w-16', 'w-32', 'w-64', 'w-96'],
  },
  {
    label: 'height: numeric vs keyword',
    candidates: ['h-0', 'h-32', 'h-64', 'h-full', 'h-fit', 'h-auto'],
  },
  {
    label: 'min-width / max-width',
    candidates: ['min-w-0', 'min-w-full', 'min-w-fit', 'max-w-0', 'max-w-full', 'max-w-fit'],
  },
  {
    label: 'padding: numeric',
    candidates: ['p-0', 'p-1', 'p-2', 'p-4', 'p-8', 'px-2', 'py-4'],
  },
  {
    label: 'margin: numeric + auto',
    candidates: ['m-0', 'm-1', 'm-4', 'm-auto', 'mx-auto', 'my-4'],
  },
  {
    label: 'inset family ordering',
    candidates: ['inset-0', 'inset-x-0', 'top-0', 'right-0', 'bottom-0', 'left-0'],
  },
  {
    label: 'arbitrary widths',
    candidates: ['w-[100px]', 'w-[200px]', 'w-64', 'w-fit', 'w-full'],
  },
]

function extractOrder(css: string, candidates: Iterable<string>): string[] {
  const out: string[] = []
  const seen = new Set<string>()
  const re = /\.([a-zA-Z0-9_\\:./[\]()#%-]+?)\s*\{/g
  const lookup = candidates instanceof Set ? candidates : new Set(candidates)
  let m: RegExpExecArray | null
  while ((m = re.exec(css)) !== null) {
    const name = m[1].replace(/\\/g, '')
    if (lookup.has(name) && !seen.has(name)) {
      out.push(name)
      seen.add(name)
    }
  }
  return out
}

describe('value-utility emission order matches Tailwind', () => {
  for (const probe of PROBES) {
    it(probe.label, async () => {
      const [tw, bz] = await Promise.all([
        compileWithTailwind3({ candidates: probe.candidates, inputCss, config }),
        compile({ candidates: probe.candidates, inputCss, config }),
      ])
      const twOrder = extractOrder(tw.css, probe.candidates)
      const bzOrder = extractOrder(bz.css, probe.candidates)
      expect(bzOrder, `probe=${probe.label}`).toEqual(twOrder)
    })
  }
})

// ---- High-cardinality overflow guard ----
//
// Regression for the `input_index` slot in `build_rule_offset`'s
// packed `RuleOffset.index`. The slot used to be 12 bits → only 4096
// distinct values. Above that, distinct candidates collide on
// `input_index & 0xFFF` and the final stable sort falls back on the
// par_iter emit order, scrambling intra-plugin emission order.
//
// The slot was widened to 24 bits (16M candidates). These tests
// stress the new slot from two angles:
//   (a) Same-cohort within-plugin order (`w-64` vs `w-fit`), padded
//       with arbitrary widths so input_indexes spread across multiple
//       4096-blocks.
//   (b) Cross-cohort ordering — many candidates in many plugins
//       around the >4096 boundary, asserting EVERY rule's emission
//       position matches Tailwind 3.4.19's exactly.
describe('value-utility emission order under high candidate count', () => {
  function buildPadding(count: number): string[] {
    const out: string[] = []
    for (let i = 0; i < count; i++) {
      // Use arbitrary widths — each unique, each a valid candidate.
      // Pad numbers to keep alphabetic order stable and dense.
      out.push(`w-[${1000 + i}px]`)
    }
    return out
  }

  const SCENARIOS = [
    { label: '5000 candidates', count: 5000 },
    { label: '8200 candidates', count: 8200 },
    // Tests the EXACT old-slot boundary, where the largest collision
    // density would have lived (4096 + a handful so input_indexes
    // for `w-64`/`w-fit` straddle the wrap).
    { label: '4097 candidates (slot boundary)', count: 4097 },
    // Deep stress — confirms the 24-bit slot's headroom holds well
    // past any realistic project size.
    { label: '20000 candidates', count: 20000 },
  ]

  for (const scenario of SCENARIOS) {
    it(`w-64 emits before w-fit with ${scenario.label}`, async () => {
      const candidates = ['w-64', 'w-fit', ...buildPadding(scenario.count)]
      const [tw, bz] = await Promise.all([
        compileWithTailwind3({ candidates, inputCss, config }),
        compile({ candidates, inputCss, config }),
      ])
      const focus = ['w-64', 'w-fit']
      const twOrder = extractOrder(tw.css, focus)
      const bzOrder = extractOrder(bz.css, focus)
      expect(twOrder).toEqual(['w-64', 'w-fit'])
      expect(bzOrder, `scenario=${scenario.label}`).toEqual(twOrder)
    })
  }

  // Cross-plugin stress: build a corpus that mixes many groups
  // around the >4096 boundary so the packed key's low bits are
  // densely populated. Every shared rule must emit in the same
  // relative position as Tailwind v3.
  it('cross-plugin ordering identical to Tailwind v3 at >4096 candidates', async () => {
    const candidates: string[] = []
    // Build a wide candidate set spanning many groups. Use arbitrary
    // values (different inner numbers) per group so each candidate
    // is unique and distinguishable.
    const groups = [
      'w',
      'h',
      'min-w',
      'max-w',
      'min-h',
      'max-h',
      'p',
      'px',
      'py',
      'm',
      'mx',
      'top',
      'inset',
      'gap',
      'basis',
      'translate-x',
      'translate-y',
      'scale-x',
      'scale-y',
      'skew-x',
      'skew-y',
    ]
    // Approximate per-group: 250 arbitrary entries — total ~5250
    // candidates, well past the 4096-bit slot boundary.
    for (const prefix of groups) {
      for (let i = 0; i < 250; i++) {
        candidates.push(`${prefix}-[${1000 + i}px]`)
      }
    }
    // Sprinkle in the keyword + numeric variants of width as a
    // canary pair — `w-64` vs `w-fit` is the most common
    // mixed-cohort divergence (digit vs keyword in the same plugin).
    candidates.push('w-64', 'w-fit', 'w-full', 'w-auto', 'w-1/2')

    const candidateSet = new Set(candidates)

    const [tw, bz] = await Promise.all([
      compileWithTailwind3({ candidates, inputCss, config }),
      compile({ candidates, inputCss, config }),
    ])
    const twOrder = extractOrder(tw.css, candidateSet)
    const bzOrder = extractOrder(bz.css, candidateSet)
    expect(twOrder.length).toBeGreaterThan(4096)
    expect(bzOrder, 'cross-plugin order diverges past slot boundary').toEqual(twOrder)
  })
})

// ---- Transform-family wpo sharing ----
//
// Tailwind v3 registers `translate`, `skew`, and `scale` via
// `createUtilityPlugin` with grouped utility tuples — both axes of
// `translate-x`/`translate-y` (and the `skew`, `scale` analogues)
// emit through a SINGLE `matchUtilities` call. They therefore share
// `Offsets.create('utilities')` slot and tie-break by candidate
// alphabetic input order. Without the group-share, galeforce gave
// each axis a unique `value_table_index` → distinct
// `within_plugin_order` → all `*-x` clustered ahead of all `*-y`,
// instead of being interleaved alphabetically by candidate.
describe('transform-family axes share within_plugin_order', () => {
  const TRANSFORM_GROUPS = [
    {
      label: 'translate (x + y interleaved)',
      candidates: [
        '-translate-x-1/2',
        '-translate-y-1/2',
        'translate-x-0',
        'translate-y-0',
        'translate-x-1/2',
        'translate-y-1/2',
      ],
    },
    {
      label: 'skew (x + y interleaved)',
      candidates: [
        '-skew-x-1',
        '-skew-y-1',
        'skew-x-0',
        'skew-y-0',
        'skew-x-3',
        'skew-y-3',
      ],
    },
    {
      label: 'scale (x + y interleaved)',
      candidates: [
        '-scale-x-100',
        '-scale-y-100',
        'scale-x-0',
        'scale-y-0',
        'scale-x-100',
        'scale-y-100',
        'scale-100',
      ],
    },
  ]
  for (const group of TRANSFORM_GROUPS) {
    it(group.label, async () => {
      const candidateSet = new Set(group.candidates)
      const [tw, bz] = await Promise.all([
        compileWithTailwind3({ candidates: group.candidates, inputCss, config }),
        compile({ candidates: group.candidates, inputCss, config }),
      ])
      const twOrder = extractOrder(tw.css, candidateSet)
      const bzOrder = extractOrder(bz.css, candidateSet)
      expect(twOrder.length).toBeGreaterThan(0)
      expect(bzOrder, group.label).toEqual(twOrder)
    })
  }
})
