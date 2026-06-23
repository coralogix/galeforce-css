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

// Variant-emission-order regression guard.
//
// Compiles a known set of variants against the same base utility
// (`flex`) and compares Galeforce's rule emission ORDER to
// Tailwind 3.4.19's. Caught the `dark:` < `md:` ordering bug — our
// previous sort used `min_width` as the tiebreaker for at-rule
// variants, which put `dark:` (no min-width, parsed as 0) BEFORE
// `sm/md/lg` instead of after.
//
// The companion `emission-order.test.ts` covers within-plugin
// static-utility order; this file covers cross-variant order
// (and group/peer composites). Together they pin both axes the
// fixture-based harness was missing.

import { describe, expect, it } from 'vitest'
import { compileWithTailwind3 } from '@coralogix/galeforcecss-oracle'
import { compile } from '@coralogix/galeforcecss'

const config = { corePlugins: { preflight: false } }
const inputCss = '@tailwind utilities;'

interface Probe {
  label: string
  candidates: string[]
}

const PROBES: Probe[] = [
  {
    label: 'responsive vs dark',
    candidates: ['bg-red-500', 'dark:bg-red-500', 'hover:bg-red-500', 'md:bg-red-500'],
  },
  {
    label: 'all responsive breakpoints',
    candidates: ['flex', 'sm:flex', 'md:flex', 'lg:flex', 'xl:flex', '2xl:flex'],
  },
  {
    label: 'interactive pseudo-classes',
    candidates: ['flex', 'hover:flex', 'focus:flex', 'active:flex', 'disabled:flex'],
  },
  {
    label: 'group/peer composites',
    candidates: ['flex', 'group-hover:flex', 'group-focus:flex', 'peer-hover:flex', 'peer-checked:flex'],
  },
  {
    label: 'motion / contrast / dark / print',
    candidates: [
      'flex',
      'motion-safe:flex',
      'motion-reduce:flex',
      'contrast-more:flex',
      'dark:flex',
      'print:flex',
    ],
  },
  {
    label: 'pseudo-elements',
    candidates: [
      'flex',
      'before:flex',
      'after:flex',
      'placeholder:flex',
      'file:flex',
      'marker:flex',
      'selection:flex',
    ],
  },
  {
    label: 'direction',
    candidates: ['flex', 'ltr:flex', 'rtl:flex'],
  },
  {
    label: 'aria + data',
    candidates: [
      'flex',
      'aria-busy:flex',
      'aria-checked:flex',
      'data-[state=open]:flex',
    ],
  },
]

function extractOrder(css: string, candidates: string[]): string[] {
  const out: string[] = []
  const seen = new Set<string>()
  // PostCSS escapes `:` in selectors as `\:` — strip backslashes
  // before matching the candidate.
  const re = /\.([a-zA-Z0-9_\\:.[\]\-=()]+?)(?:\s*\{|:hover|:focus|:active|:disabled|:checked|\s+|\[|~|>)/g
  let m: RegExpExecArray | null
  const lookup = new Set(candidates)
  while ((m = re.exec(css)) !== null) {
    const name = m[1].replace(/\\/g, '')
    if (lookup.has(name) && !seen.has(name)) {
      out.push(name)
      seen.add(name)
    }
  }
  return out
}

describe('variant emission order matches Tailwind', () => {
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
