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

// Emission-order regression guard.
//
// The main conformance harness normalises CSS via PostCSS (semantic
// equivalence — rules-as-a-set), which is what we want for catching
// missing or wrong declarations. It deliberately ignores rule ORDER
// because most utilities are commutative under the cascade.
//
// `display`-style plugins are the exception: `class="flex hidden"`
// expects `display: none` to win, which only happens if `.hidden`
// appears AFTER `.flex` in source order. Our sort-key used to
// tie-break by a pseudo-random `prefix_hash` within a plugin, so
// `hidden` landed before `flex` and `block` — silently broke the
// cascade in real apps.
//
// This file compiles every static-utility family with all of the
// plugin's candidates as input, then asserts that Galeforce's emission
// order matches Tailwind 3.4.19's order class-for-class. If a new
// plugin is added without a matching declaration-order in
// `static_utilities.rs`, this test fails — caught in CI, not in a
// user's UI.

import { readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { compileWithTailwind3 } from '@coralogix/galeforcecss-oracle'
import { compile } from '@coralogix/galeforcecss'

const here = path.dirname(fileURLToPath(import.meta.url))
const staticUtilsPath = path.resolve(
  here,
  '../../../crates/galeforce-compiler/src/static_utilities.rs',
)

function loadPluginGroups(): Map<string, string[]> {
  const src = readFileSync(staticUtilsPath, 'utf8')
  const re = /u!\(\s*"([^"]+)"\s*,\s*"([^"]+)"/g
  const byPlugin = new Map<string, string[]>()
  for (const m of src.matchAll(re)) {
    const [, name, plugin] = m
    if (!byPlugin.has(plugin)) byPlugin.set(plugin, [])
    byPlugin.get(plugin)!.push(name)
  }
  return byPlugin
}

function extractClassOrder(css: string, candidates: string[]): string[] {
  const out: string[] = []
  const seen = new Set<string>()
  const re = /\.([a-z0-9_\\:-]+)\s*\{/g
  let m: RegExpExecArray | null
  while ((m = re.exec(css)) !== null) {
    const name = m[1].replace(/\\/g, '')
    if (candidates.includes(name) && !seen.has(name)) {
      out.push(name)
      seen.add(name)
    }
  }
  return out
}

const groups = loadPluginGroups()
const config = { corePlugins: { preflight: false } }

describe('static utility emission order matches Tailwind', () => {
  for (const [plugin, candidates] of [...groups].sort()) {
    if (candidates.length < 2) continue
    it(`plugin=${plugin} (${candidates.length} utilities)`, async () => {
      const [tw, bz] = await Promise.all([
        compileWithTailwind3({ candidates, inputCss: '@tailwind utilities;', config }),
        compile({ candidates, inputCss: '@tailwind utilities;', config }),
      ])
      const twOrder = extractClassOrder(tw.css, candidates)
      const bzOrder = extractClassOrder(bz.css, candidates)
      expect(bzOrder, `plugin=${plugin}`).toEqual(twOrder)
    })
  }
})
