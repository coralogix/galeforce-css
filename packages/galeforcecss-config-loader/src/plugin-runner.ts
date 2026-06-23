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

// Capture-only Tailwind plugin runner.
//
// Tailwind plugins are JS functions that receive a context object with
// helper methods (`addUtilities`, `addComponents`, `addBase`,
// `addVariant`, `matchUtilities`, `matchVariant`, `theme`, `e`,
// `prefix`, `config`, `corePlugins`). We invoke the plugin against a
// mock context that records the calls into structured data, then pass
// that data to the Rust compiler.
//
// What we capture (MVP):
//   - addUtilities, addComponents, addBase
//   - matchUtilities (materialized: invoke the function for each
//     value in `options.values`, emit as static utilities)
//   - addVariant (selector format strings + at-rule strings)
//
// What we don't capture (caller gets a warning):
//   - matchVariant (value-bearing variants — would need Rust-side
//     resolver hookup)
//   - matchComponents (same, less common)
//   - JS callbacks that depend on runtime context unavailable at
//     plugin-load time
//
// The mock context is intentionally minimal; we lean on Tailwind's
// own `resolveConfig` for the theme so `theme()` lookups inside
// plugins return real values.

import postcss from 'postcss'
import type { PluginOutput, PluginRule, PluginVariant } from './index.js'

interface ResolvedConfigShape {
  theme?: Record<string, unknown>
  prefix?: string
  [k: string]: unknown
}

type PluginFn = (api: PluginApi) => void
type PluginObject = { handler: PluginFn; config?: Record<string, unknown> }
type PluginEntry = PluginFn | PluginObject

interface PluginApi {
  addUtilities: (utilities: NestedRules, options?: AddOptions) => void
  addComponents: (components: NestedRules, options?: AddOptions) => void
  addBase: (base: NestedRules) => void
  addVariant: (
    name: string,
    formats: string | string[] | ((api: { container: unknown }) => unknown),
  ) => void
  matchUtilities: (
    utilities: Record<string, (value: string) => NestedRules | NestedRules[]>,
    options?: MatchOptions,
  ) => void
  matchComponents: (
    components: Record<string, (value: string) => NestedRules | NestedRules[]>,
    options?: MatchOptions,
  ) => void
  matchVariant: (
    name: string,
    fn: (
      value: string,
      extras: { modifier: string | null },
    ) => string | string[],
    options?: MatchVariantOptions,
  ) => void
  theme: (path: string, defaultValue?: unknown) => unknown
  config: (path: string, defaultValue?: unknown) => unknown
  e: (className: string) => string
  prefix: (selector: string) => string
  corePlugins: (name: string) => boolean
  variants: (path: string) => unknown
  /**
   * The real `postcss` module — plugins use `postcss.rule({ selector })`
   * and `postcss.decl({ prop, value })` to construct CSS nodes.
   * Tailwind exposes the same reference; we forward it verbatim and
   * convert any PostCSS nodes that flow back into `addUtilities` /
   * `addComponents` into the nested object form our resolver
   * expects.
   */
  postcss: typeof postcss
}

interface AddOptions {
  variants?: string[]
  respectPrefix?: boolean
  respectImportant?: boolean
}

interface MatchOptions {
  values?: Record<string, string | string[]>
  type?: string | string[]
  supportsNegativeValues?: boolean
  respectPrefix?: boolean
  respectImportant?: boolean
  modifiers?: 'any' | Record<string, string>
}

interface MatchVariantOptions {
  values?: Record<string, string>
}

/**
 * Tailwind's plugin API accepts deeply-nested style objects:
 *   { '.foo': { '&:hover': { color: 'red' } } }
 *   { '.foo': [{ color: 'red' }, { color: 'blue' }] }   // array variants
 *   { '.foo': { '@media (min-width: 768px)': { color: 'red' } } }
 *
 * We flatten these into a list of `PluginRule`s with explicit at-rule
 * stacks.
 */
type NestedRules = Record<string, unknown>

const WARNING_PREFIX = '[galeforcecss/plugin-runner]'

/// Escape a class name for use in a CSS selector. Mirrors
/// Tailwind's `escapeClassName` (cssesc with `isIdentifier: true`):
/// any char outside `[A-Za-z0-9_-]` and ASCII range gets a
/// leading backslash. Used by the plugin API's `e()` helper —
/// `e('custom-top-1/4') -> 'custom-top-1\/4'`.
function escapeClassName(name: string): string {
  let out = ''
  for (let i = 0; i < name.length; i++) {
    const ch = name.charCodeAt(i)
    const c = name[i]
    // Allowed identifier chars pass through.
    if (
      (ch >= 0x30 && ch <= 0x39) || // 0-9
      (ch >= 0x41 && ch <= 0x5a) || // A-Z
      (ch >= 0x61 && ch <= 0x7a) || // a-z
      ch === 0x5f || // _
      ch === 0x2d || // -
      ch >= 0x80 // non-ASCII
    ) {
      out += c
      continue
    }
    out += '\\' + c
  }
  return out
}


/**
 * Live registration of a `matchVariant` call. The function reference
 * is kept ALIVE so we can invoke it later for arbitrary-value
 * candidates the user wrote in source (e.g. `my-[checked]`). Tailwind
 * v3 does the same: matchVariant functions are called at JIT time per
 * matched candidate. We invoke once per UNIQUE arbitrary value
 * discovered in the candidate set.
 */
interface LiveMatchVariant {
  name: string
  fn: (value: string, extras: { modifier: string | null }) => string | string[]
  /** Configured value table (`options.values`) so the modifier
   *  materializer can resolve a candidate's value-key into its
   *  underlying string. Empty object when the variant didn't declare
   *  one. */
  values: Record<string, string>
}

/**
 * Live registration of a `matchUtilities` call. Same shape, function
 * reference kept alive for arbitrary-value materialization.
 */
interface LiveMatchUtility {
  classPrefix: string
  fn: (
    value: string,
    extras: { modifier: string | null },
  ) => NestedRules | NestedRules[] | null
  /** All values declared in `options.values` — needed when
   *  materializing modifier candidates (`test/good`) which combine
   *  a configured value with a candidate-supplied modifier. */
  values: Record<string, string>
  /** Modifiers config: `'any'` permits any string; an object whitelists
   *  named modifiers; absent/undefined disables modifier
   *  materialization. */
  modifiers: 'any' | Record<string, string> | undefined
  /** Whether this came from `matchComponents` (true) or
   *  `matchUtilities` (false). Routes the materialized output to
   *  the right bucket so the @tailwind components slot picks it
   *  up rather than @tailwind utilities. */
  isComponent: boolean
  /** `options.type` declared at registration. Used by the candidate-
   *  driven materializer to detect ambiguity when multiple plugins
   *  share a class prefix and accept the same arbitrary value
   *  shape. Empty array means no type restriction (accept anything). */
  types: string[]
}

/**
 * Invoke each plugin in `plugins` against a recording context and
 * return the captured output. Errors thrown by individual plugins are
 * converted to warnings — one bad plugin shouldn't kill the whole build.
 *
 * `candidates` is the project's scanned candidate set. When supplied,
 * arbitrary-value candidates that match registered matchVariant /
 * matchUtilities prefixes (e.g. `my-[checked]:foo`, `tab-[3.5]`) get
 * the plugin function invoked once per unique arbitrary value, and
 * the resulting variant/utility is materialized into the plugin
 * output. This matches Tailwind v3's JIT behavior — plugins are
 * invoked at compile time, not at runtime.
 */
export function runPlugins(
  plugins: unknown,
  resolved: ResolvedConfigShape,
  candidates: readonly string[] = [],
): { output: PluginOutput; warnings: string[] } {
  const output: PluginOutput = {
    utilities: [],
    components: [],
    base: [],
    variants: [],
  }
  const warnings: string[] = []
  const liveVariants: LiveMatchVariant[] = []
  const liveUtilities: LiveMatchUtility[] = []

  if (!Array.isArray(plugins)) {
    return { output, warnings }
  }

  for (const entry of plugins as PluginEntry[]) {
    // Resolve the actual handler. Tailwind's `plugin.withOptions(fn)` pattern
    // produces a function with `__isOptionsFunction = true`; calling it with no
    // args materialises the default-options variant and returns `{ handler, config }`.
    let resolved_entry: unknown = entry
    if (
      typeof resolved_entry === 'function' &&
      (resolved_entry as { __isOptionsFunction?: boolean }).__isOptionsFunction
    ) {
      resolved_entry = (resolved_entry as () => unknown)()
    }
    const handler =
      typeof resolved_entry === 'function' ? resolved_entry : (resolved_entry as { handler?: unknown })?.handler
    if (typeof handler !== 'function') {
      warnings.push(`${WARNING_PREFIX} skipped non-function plugin entry`)
      continue
    }
    const api = makeApi(resolved, output, warnings, liveVariants, liveUtilities)
    try {
      handler(api)
    } catch (err) {
      warnings.push(
        `${WARNING_PREFIX} plugin "${(handler as { name?: string }).name || '(anonymous)'}" threw: ${
          err instanceof Error ? err.message : String(err)
        }`,
      )
    }
  }

  // Arbitrary-value materialization. Walk candidates, find tokens
  // that match a registered matchVariant / matchUtilities prefix
  // followed by `[<arbitrary>]`, invoke the function once per unique
  // arbitrary value, append to plugin output.
  if (candidates.length > 0 && (liveVariants.length > 0 || liveUtilities.length > 0)) {
    materializeArbitraryFromCandidates(
      candidates,
      liveVariants,
      liveUtilities,
      output,
      warnings,
    )
  }

  // Resolve `@apply <utility>` markers inside plugin rules by
  // looking up the target in the same plugin output (`.utility` /
  // `.component`) and inlining its declarations. Mirrors the
  // intra-plugin slice of upstream's `expandApplyAtRules.js`
  // localCache. Cross-plugin / core-utility @apply targets are
  // left as markers — the Rust directive processor handles those
  // when the plugin rule lands at compile time.
  resolvePluginApply(output)

  return { output, warnings }
}

function resolvePluginApply(output: PluginOutput): void {
  // Index every plugin-output rule by its primary class so an
  // `@apply <name>` marker can find its source. Same primary-class
  // extraction as the Rust side (see `plugins.rs::primary_class`).
  const all: PluginRule[] = [...output.utilities, ...output.components]
  if (!all.some((r) => r.declarations.some((d) => d.property === '@apply'))) {
    return
  }
  const byClass = new Map<string, PluginRule>()
  for (const r of all) {
    const cls = primaryClassFromSelector(r.selector)
    if (cls && !byClass.has(cls)) byClass.set(cls, r)
  }
  // Resolve markers iteratively so chained `@apply` (`.c { @apply b }`,
  // `.b { @apply a }`) flattens. Cap iterations to keep cycles
  // from spinning forever — they emit a warning and stop.
  for (let iter = 0; iter < 16; iter++) {
    let changed = false
    for (const r of all) {
      const newDecls: typeof r.declarations = []
      for (const d of r.declarations) {
        if (d.property === '@apply') {
          const target = byClass.get(d.value.trim())
          if (target) {
            for (const td of target.declarations) {
              newDecls.push({ ...td })
            }
            changed = true
            continue
          }
        }
        newDecls.push(d)
      }
      r.declarations = newDecls
    }
    if (!changed) break
  }
}

function primaryClassFromSelector(selector: string): string | null {
  const s = selector.trim()
  if (!s.startsWith('.')) return null
  const rest = s.slice(1)
  let i = 0
  let depth = 0
  while (i < rest.length) {
    const ch = rest[i]!
    if (ch === '\\' && i + 1 < rest.length) {
      i += 2
      continue
    }
    if (ch === '[') {
      depth++
      i++
      continue
    }
    if (ch === ']') {
      if (depth > 0) {
        depth--
        i++
        continue
      }
      break
    }
    if (depth > 0) {
      i++
      continue
    }
    if (/[a-zA-Z0-9_\-/]/.test(ch)) {
      i++
      continue
    }
    break
  }
  return i > 0 ? rest.slice(0, i) : null
}

/**
 * Walk candidates, find arbitrary-value tokens against registered
 * matchVariant / matchUtilities prefixes, invoke the plugin function
 * once per unique arbitrary value, append to `output`.
 *
 * For variants: candidate `my-[checked]:foo` → run matchVariant `my`
 * with value `checked` → `&[data-checked]` → emit PluginVariant
 * `{ name: 'my-[checked]', selectorFormats: ['&[data-checked]'], ... }`.
 *
 * For utilities: candidate `tab-[3.5]` → run matchUtilities `tab`
 * with value `3.5` → `{ 'tab-size': '3.5' }` → emit PluginRule
 * `{ selector: '.tab-\[3.5\]', declarations: [...] }`. The class
 * name in the selector keeps the brackets so the candidate's
 * `parsed.root` (which retains them) matches in the Rust lookup.
 */
function materializeArbitraryFromCandidates(
  candidates: readonly string[],
  liveVariants: readonly LiveMatchVariant[],
  liveUtilities: readonly LiveMatchUtility[],
  output: PluginOutput,
  warnings: string[],
): void {
  // Track what we've already materialized so duplicate candidates
  // don't blow up the output. Keyed by (kind, name, value).
  const seen = new Set<string>()

  for (const cand of candidates) {
    // A candidate can have variants chained left-to-right separated
    // by ':' OR be a bare utility. For matchVariants we need to
    // examine each variant segment; for matchUtilities we examine
    // the trailing utility segment.
    const segments = splitTopLevelColons(cand)
    const variantSegments = segments.slice(0, -1)
    const utilitySegment = segments[segments.length - 1] ?? cand

    // matchVariant arbitrary materialization.
    for (const seg of variantSegments) {
      for (const lv of liveVariants) {
        const arb = matchArbitrary(seg, lv.name)
        if (arb === null) continue
        const key = `v|${lv.name}|${arb}`
        if (seen.has(key)) continue
        seen.add(key)
        let result: string | string[]
        try {
          result = lv.fn(arb, { modifier: null })
        } catch {
          continue
        }
        const variantName = `${lv.name}-[${arb}]`
        output.variants.push(toVariantRecord(variantName, result, warnings))
      }

      // matchVariant modifier-bearing form: `<name>-<valueKey>/<mod>`.
      // Plugin value keys can themselves contain `/` (e.g.
      // `'1/10': '1/10'`), so iterate every top-level split point
      // and accept the first one whose head resolves to a known
      // value key. Mirrors upstream's JIT matchVariant modifier
      // handling.
      for (const lv of liveVariants) {
        const slashes = findAllTopLevelSlashes(seg)
        if (slashes.length === 0) continue
        let matched = false
        for (const slashIdx of slashes) {
          if (matched) break
          const head = seg.slice(0, slashIdx)
          const modifierRaw = seg.slice(slashIdx + 1)
          if (modifierRaw.length === 0) continue
          let valueKey: string | null = null
          if (head === lv.name) {
            valueKey = 'DEFAULT'
          } else if (head.startsWith(`${lv.name}-`)) {
            valueKey = head.slice(lv.name.length + 1)
          }
          if (valueKey === null) continue
          const valueValue = lv.values[valueKey]
          if (valueValue === undefined) continue
          const arbModifier =
            modifierRaw.startsWith('[') &&
            modifierRaw.endsWith(']') &&
            modifierRaw.length >= 2
              ? modifierRaw.slice(1, -1)
              : null
          const modifier = arbModifier ?? modifierRaw
          const key = `v|${lv.name}|${valueKey}|${modifier}`
          if (seen.has(key)) continue
          seen.add(key)
          let result: string | string[]
          try {
            result = lv.fn(valueValue, { modifier })
          } catch {
            continue
          }
          const headOut = valueKey === 'DEFAULT' ? lv.name : `${lv.name}-${valueKey}`
          const variantName = `${headOut}/${modifierRaw}`
          output.variants.push(toVariantRecord(variantName, result, warnings))
          matched = true
        }
      }
    }

    // matchUtilities arbitrary materialization. Strip leading `!`
    // (important) and `-` (negative) before matching so
    // `!tab-[3.5]` and `-mt-[12px]` resolve too.
    let util = utilitySegment
    if (util.startsWith('!')) util = util.slice(1)
    if (util.startsWith('-')) util = util.slice(1)

    // Ambiguity check: when multiple matchUtilities plugins share a
    // prefix and accept the same arbitrary value (without an
    // explicit `type:` hint disambiguating them), upstream warns
    // and emits NOTHING. We pre-collect the set of plugins whose
    // arb-value lookup succeeds — when there's more than one with
    // distinct types, treat the candidate as ambiguous and skip.
    const ambiguousPrefixes = new Set<string>()
    {
      type Match = { lu: LiveMatchUtility; arb: string; result: NestedRules | NestedRules[] }
      const matchesByPrefix = new Map<string, Match[]>()
      for (const lu of liveUtilities) {
        const arb = matchArbitrary(util, lu.classPrefix)
        if (arb === null) continue
        let result: NestedRules | NestedRules[] | null
        try {
          result = lu.fn(arb, { modifier: null })
        } catch {
          continue
        }
        if (result === null || result === undefined) continue
        const list = matchesByPrefix.get(lu.classPrefix) ?? []
        list.push({ lu, arb, result })
        matchesByPrefix.set(lu.classPrefix, list)
      }
      for (const [prefix, list] of matchesByPrefix) {
        if (list.length < 2) continue
        // Collect distinct type sets — if every plugin shares the
        // same effective type set, treat as a non-ambiguous tie
        // (last-wins). If types differ AND no single plugin has a
        // type the others don't subsume, it's ambiguous.
        const sets = list.map((m) => new Set(m.lu.types))
        const allSame =
          sets.length > 0 &&
          sets.every(
            (s) =>
              s.size === sets[0]!.size &&
              [...s].every((t) => sets[0]!.has(t)),
          )
        if (!allSame) {
          ambiguousPrefixes.add(prefix)
        }
      }
    }

    for (const lu of liveUtilities) {
      if (ambiguousPrefixes.has(lu.classPrefix)) continue
      const arb = matchArbitrary(util, lu.classPrefix)
      if (arb === null) continue
      const key = `u|${lu.classPrefix}|${arb}`
      if (seen.has(key)) continue
      seen.add(key)
      let result: NestedRules | NestedRules[] | null
      try {
        result = lu.fn(arb, { modifier: null })
      } catch {
        continue
      }
      if (result === null || result === undefined) continue
      // The emitted class name preserves the brackets so the Rust
      // candidate-root lookup matches verbatim.
      const className = `.${lu.classPrefix}-[${arb}]`
      const wrapped: NestedRules = { [className]: result as NestedRules }
      const groupKey = className.slice(1)
      const flat = flatten(wrapped, [])
      for (const r of flat) {
        ;(r as PluginRule & { groupClass?: string }).groupClass = groupKey
      }
      const bucket = lu.isComponent ? output.components : output.utilities
      bucket.push(...flat)
    }

    // matchUtilities modifier-bearing candidate materialization.
    // For each candidate of the form `<classPrefix>[-<value>]/<modifier>`
    // we replay `fn(<resolved-value>, { modifier: <modifier> })` and
    // emit the result. Mirrors upstream's JIT modifier handling.
    // Modifiers config gates which strings are accepted: `'any'`
    // permits any string; an object whitelists named modifiers
    // (the resolved value is looked up by key); absent means no
    // modifier materialization.
    //
    // Skip if the FULL utility (including any `/...` suffix) matches
    // a configured value key — Tailwind treats those as direct value
    // lookups (`test-1/foo` with values: { '1/foo': '...' }) handled
    // by the regular sync materialization, not modifier resolution.
    // Otherwise split at the LAST top-level `/` so we don't eat any
    // `/`-bearing value key.
    const slashIdx = findTopLevelSlash(util)
    if (slashIdx === -1) continue
    const utilHead = util.slice(0, slashIdx)
    const modifierStr = util.slice(slashIdx + 1)
    if (modifierStr.length === 0) continue
    const fullValueKeyMatches = liveUtilities.some((lu) => {
      if (util === lu.classPrefix) return lu.values['DEFAULT'] !== undefined
      if (util.startsWith(`${lu.classPrefix}-`)) {
        const k = util.slice(lu.classPrefix.length + 1)
        return lu.values[k] !== undefined
      }
      return false
    })
    if (fullValueKeyMatches) continue
    for (const lu of liveUtilities) {
      if (!lu.modifiers) continue
      // Match `<classPrefix>` (DEFAULT) or `<classPrefix>-<valueKey>`.
      let valueKey: string | null = null
      if (utilHead === lu.classPrefix) {
        valueKey = 'DEFAULT'
      } else if (utilHead.startsWith(`${lu.classPrefix}-`)) {
        valueKey = utilHead.slice(lu.classPrefix.length + 1)
      }
      if (valueKey === null) continue
      const valueValue = lu.values[valueKey]
      if (valueValue === undefined) continue
      // Resolve the modifier per options.modifiers shape:
      //   - `'any'`: pass the modifier through as-is. Arbitrary
      //     forms (`/[foo]`) keep their brackets — upstream emits
      //     `default_[foo]` for `test/[foo]` under `modifiers: 'any'`.
      //   - object: arbitrary forms unwrap (`/[bar]` → `'bar'`);
      //     named modifiers go through the lookup map.
      let modifierResolved: string | null = null
      const arbModifier =
        modifierStr.startsWith('[') && modifierStr.endsWith(']') && modifierStr.length >= 2
          ? modifierStr.slice(1, -1)
          : null
      if (lu.modifiers === 'any') {
        modifierResolved = modifierStr
      } else if (typeof lu.modifiers === 'object' && lu.modifiers !== null) {
        if (arbModifier !== null) {
          modifierResolved = arbModifier
        } else {
          const v = lu.modifiers[modifierStr]
          if (typeof v === 'string') modifierResolved = v
        }
      }
      if (modifierResolved === null) continue
      const key = `m|${lu.classPrefix}|${valueKey}|${modifierStr}`
      if (seen.has(key)) continue
      seen.add(key)
      let result: NestedRules | NestedRules[] | null
      try {
        result = lu.fn(valueValue, { modifier: modifierResolved })
      } catch {
        continue
      }
      if (result === null || result === undefined) continue
      const head = valueKey === 'DEFAULT' ? lu.classPrefix : `${lu.classPrefix}-${valueKey}`
      const className = `.${head}\\/${modifierStr}`
      const wrapped: NestedRules = { [className]: result as NestedRules }
      const groupKey = className.slice(1)
      const flat = flatten(wrapped, [])
      for (const r of flat) {
        ;(r as PluginRule & { groupClass?: string }).groupClass = groupKey
      }
      const bucket = lu.isComponent ? output.components : output.utilities
      bucket.push(...flat)
    }
  }
}

/** Find the first `/` outside `[...]` brackets, or -1 if none. */
function findTopLevelSlash(seg: string): number {
  let depth = 0
  for (let i = 0; i < seg.length; i++) {
    const c = seg[i]!
    if (c === '\\' && i + 1 < seg.length) {
      i++
      continue
    }
    if (c === '[') depth++
    else if (c === ']') depth--
    else if (c === '/' && depth === 0) return i
  }
  return -1
}

function findAllTopLevelSlashes(seg: string): number[] {
  const out: number[] = []
  let depth = 0
  for (let i = 0; i < seg.length; i++) {
    const c = seg[i]!
    if (c === '\\' && i + 1 < seg.length) {
      i++
      continue
    }
    if (c === '[') depth++
    else if (c === ']') depth--
    else if (c === '/' && depth === 0) out.push(i)
  }
  return out
}

/**
 * If `seg` is `<prefix>-[<inner>]` (a matchVariant/matchUtilities
 * arbitrary-value form for the given prefix), return the inner
 * value. Otherwise null. Brackets must balance — `bg-[url(foo[bar])]`
 * counts the inner `[bar]` correctly.
 */
function matchArbitrary(seg: string, prefix: string): string | null {
  const head = `${prefix}-[`
  if (!seg.startsWith(head)) return null
  if (!seg.endsWith(']')) return null
  // Verify bracket nesting balances exactly (the close `]` we found
  // is the matching one for the open `[`, not a nested ']').
  const inner = seg.slice(head.length, seg.length - 1)
  let depth = 0
  for (let i = 0; i < inner.length; i++) {
    const c = inner[i]!
    if (c === '\\' && i + 1 < inner.length) {
      i++
      continue
    }
    if (c === '[') depth++
    else if (c === ']') {
      if (depth === 0) return null
      depth--
    }
  }
  if (depth !== 0) return null
  return inner
}

/** Split on `:` at top-level (depth 0 — ignoring `[...]` brackets). */
function splitTopLevelColons(s: string): string[] {
  const out: string[] = []
  let start = 0
  let depth = 0
  for (let i = 0; i < s.length; i++) {
    const c = s[i]!
    if (c === '\\' && i + 1 < s.length) {
      i++
      continue
    }
    if (c === '[') depth++
    else if (c === ']') depth--
    else if (c === ':' && depth === 0) {
      out.push(s.slice(start, i))
      start = i + 1
    }
  }
  out.push(s.slice(start))
  return out
}

function makeApi(
  resolved: ResolvedConfigShape,
  output: PluginOutput,
  warnings: string[],
  liveVariants: LiveMatchVariant[],
  liveUtilities: LiveMatchUtility[],
): PluginApi {
  const themeFn = (path: string, defaultValue?: unknown): unknown => {
    const raw = lookupDotPath(resolved.theme ?? {}, path)
    if (raw === undefined) return defaultValue
    // Apply the section-specific transform — `theme('boxShadow.foo')`
    // returns the array joined with `, ` (one CSS value), not the
    // raw array (which would split into N decls downstream).
    const section = toPathSegments(path)[0] ?? ''
    return transformThemeValue(section, raw)
  }

  const cfgPrefix = (resolved.prefix as string | undefined) ?? ''
  // Apply the user's `prefix` config to every `.x` class in a
  // selector. Mirrors upstream's `addUtilities`/`addComponents`
  // which call `prefixIdentifier`/`prefixSelector` automatically
  // when `options.respectPrefix !== false`. Without this, plugin
  // rules with multi-selectors like `.foo, .bar` only get the
  // first class prefixed.
  const prefixSelector = (selector: string): string => {
    if (!cfgPrefix) return selector
    return selector.replace(/(\.)(-?[a-zA-Z_])/g, `$1${cfgPrefix}$2`)
  }
  const prefixRules = (rules: PluginRule[], respectPrefix: boolean): PluginRule[] => {
    if (!cfgPrefix || !respectPrefix) return rules
    return rules.map((r) => ({ ...r, selector: prefixSelector(r.selector) }))
  }
  const tagRespectImportant = (rules: PluginRule[], respectImportant: boolean): PluginRule[] => {
    return rules.map((r) => ({ ...r, respectImportant }))
  }
  return {
    addUtilities: (utilities, options) => {
      const respectPrefix = (options as AddOptions | undefined)?.respectPrefix !== false
      // Upstream default for `addUtilities` is
      // `respectImportant: true`. Plugins can opt out via the
      // option to keep specific utilities free of the global
      // `important: true` / `important: '<sel>'` flag.
      const respectImportant = (options as AddOptions | undefined)?.respectImportant !== false
      const flat = flatten(coercePostcssNodes(utilities), [])
      output.utilities.push(
        ...tagRespectImportant(prefixRules(flat, respectPrefix), respectImportant),
      )
    },
    addComponents: (components, options) => {
      const respectPrefix = (options as AddOptions | undefined)?.respectPrefix !== false
      // Upstream default for `addComponents` is `respectImportant:
      // false`. Plugins can opt in.
      const respectImportant = (options as AddOptions | undefined)?.respectImportant === true
      const flat = flatten(coercePostcssNodes(components), [])
      output.components.push(
        ...tagRespectImportant(prefixRules(flat, respectPrefix), respectImportant),
      )
    },
    addBase: (base) => {
      // `addBase` does NOT respect the prefix — base styles target
      // raw element selectors.
      output.base.push(...flatten(coercePostcssNodes(base), []))
    },
    addVariant: (name, formats) => {
      output.variants.push(toVariantRecord(name, formats, warnings))
    },
    matchUtilities: (utilities, options) => {
      output.utilities.push(...materializeMatch(utilities, options))
      // Keep function refs alive for arbitrary-value and modifier
      // materialization. Record values + modifier config so the
      // candidate-driven pass downstream can replay the call.
      const values = (options?.values ?? {}) as Record<string, string>
      const modifiers = (options as MatchOptions | undefined)?.modifiers
      const typeOpt = (options as MatchOptions | undefined)?.type
      const types = Array.isArray(typeOpt) ? typeOpt : typeOpt ? [typeOpt] : []
      for (const [classPrefix, fn] of Object.entries(utilities)) {
        liveUtilities.push({
          classPrefix,
          fn: fn as LiveMatchUtility['fn'],
          values,
          modifiers,
          isComponent: false,
          types,
        })
      }
    },
    matchComponents: (components, options) => {
      output.components.push(...materializeMatch(components, options))
      const values = (options?.values ?? {}) as Record<string, string>
      const modifiers = (options as MatchOptions | undefined)?.modifiers
      const typeOpt = (options as MatchOptions | undefined)?.type
      const types = Array.isArray(typeOpt) ? typeOpt : typeOpt ? [typeOpt] : []
      for (const [classPrefix, fn] of Object.entries(components)) {
        liveUtilities.push({
          classPrefix,
          fn: fn as LiveMatchUtility['fn'],
          values,
          modifiers,
          isComponent: true,
          types,
        })
      }
    },
    matchVariant: (name, fn, options) => {
      // Materialize value-bearing variants at JS-time: invoke the
      // callback for each value in `options.values`, capture the
      // resulting selector format(s) as a regular `addVariant` entry
      // with name `<varName>-<valueKey>`. Same architecture as
      // `matchUtilities` — covers the most common pattern (named
      // theme keys) without needing JS evaluation per-candidate at
      // compile time.
      //
      // Arbitrary values (`<varName>-[<arbitrary>]`) would need
      // runtime resolution and are NOT materialized here. They emit
      // a warning when encountered if they're never invoked. This
      // matches our existing `matchUtilities` MVP.
      const values = options?.values ?? {}
      for (const [valueKey, valueValue] of Object.entries(values)) {
        let result: string | string[]
        try {
          // Preserve null/undefined when passing to the matcher —
          // upstream's matchVariant DEFAULT pattern often relies
          // on `value === null` / `value === undefined` checks
          // (`(value) => value === null ? '.foo &' : '.bar &'`).
          // Coercing those to strings ('null'/'undefined') drops
          // through to the wrong branch.
          const valueArg =
            valueValue === null || valueValue === undefined
              ? (valueValue as string)
              : typeof valueValue === 'string'
              ? valueValue
              : String(valueValue)
          result = fn(valueArg, { modifier: null })
        } catch {
          continue
        }
        const variantName = valueKey === 'DEFAULT' ? name : `${name}-${valueKey}`
        output.variants.push(toVariantRecord(variantName, result, warnings))
      }
      // Keep the function ref alive for arbitrary-value
      // materialization (e.g. `<name>-[<arbitrary>]:foo`) and for
      // modifier-bearing forms (`<name>-<value>/<modifier>:foo`).
      liveVariants.push({ name, fn, values: values as Record<string, string> })
    },
    theme: themeFn,
    config: (path, defaultValue) => {
      return lookupDotPath(resolved as Record<string, unknown>, path) ?? defaultValue
    },
    e: escapeClassName,
    prefix: (selector: string) => {
      const p = resolved.prefix ?? ''
      if (!p) return selector
      return selector.replace(/(\.)([a-zA-Z_])/g, `$1${p}$2`)
    },
    corePlugins: (name: string) => {
      // Mirror upstream's `corePlugins` plugin API:
      //   * `corePlugins: ['a', 'b']` allowlist → only named
      //     plugins enabled.
      //   * `corePlugins: { foo: false, bar: true }` blocklist /
      //     toggles → check the map.
      //   * undefined / true → all enabled (default).
      const cp = (resolved as Record<string, unknown>).corePlugins
      if (Array.isArray(cp)) {
        return cp.includes(name)
      }
      if (cp && typeof cp === 'object') {
        const flag = (cp as Record<string, unknown>)[name]
        if (typeof flag === 'boolean') return flag
        return true
      }
      return true
    },
    variants: () => [],
    postcss,
  }
}

/**
 * If the user passed PostCSS nodes (via `postcss.rule(...)` etc.) into
 * `addUtilities` / `addComponents`, convert them to the nested-object
 * form our resolver expects. Strings, plain objects, and arrays of
 * either pass through unchanged.
 *
 * A `postcss.rule({ selector }).append([postcss.decl({ prop, value })])`
 * becomes `{ <selector>: { <prop>: <value> } }`; nested rules
 * recurse.
 */
function coercePostcssNodes(input: unknown): NestedRules {
  if (Array.isArray(input)) {
    const out: NestedRules = {}
    for (const item of input) {
      const converted = coercePostcssNodes(item)
      for (const [k, v] of Object.entries(converted)) {
        // Duplicate selector / at-rule keys appearing across array
        // siblings get DEEP-MERGED so that two PostCSS rules with
        // the same selector (e.g. @tailwindcss/forms emits two
        // separate rules for `::-webkit-date-and-time-value`) produce
        // a single merged object rather than the second silently
        // overwriting the first.  This matches PostCSS's append
        // semantics: both rules' declarations end up in the output.
        const existing = out[k]
        if (
          existing &&
          typeof existing === 'object' &&
          !Array.isArray(existing) &&
          v &&
          typeof v === 'object' &&
          !Array.isArray(v)
        ) {
          out[k] = { ...(existing as object), ...(v as object) } as NestedRules[string]
        } else {
          out[k] = v
        }
      }
    }
    return out
  }
  if (input && typeof input === 'object') {
    const node = input as { type?: string; selector?: string; prop?: string; value?: string; nodes?: unknown[] }
    if (node.type === 'rule' && typeof node.selector === 'string') {
      const body: NestedRules = {}
      for (const child of node.nodes ?? []) {
        const converted = coercePostcssNodes(child)
        for (const [k, v] of Object.entries(converted)) {
          body[k] = v
        }
      }
      return { [node.selector]: body }
    }
    if (node.type === 'decl' && typeof node.prop === 'string') {
      return { [node.prop]: node.value ?? '' }
    }
    if (node.type === 'atrule') {
      // Less common but possible — keep the at-rule wrapper as a
      // key prefixed with `@` so the resolver's nested-rule
      // walker treats it as an at-rule context.
      const atrule = node as unknown as { name?: string; params?: string; nodes?: unknown[] }
      const key = `@${atrule.name ?? ''}${atrule.params ? ` ${atrule.params}` : ''}`
      const body: NestedRules = {}
      for (const child of atrule.nodes ?? []) {
        const converted = coercePostcssNodes(child)
        for (const [k, v] of Object.entries(converted)) {
          body[k] = v
        }
      }
      return { [key]: body }
    }
    // Plain object — return as-is (already in nested form).
    return input as NestedRules
  }
  // Strings / numbers / etc. — wrap as a key=>value pair would have
  // been malformed. Return empty.
  return {}
}

/**
 * Recursively flatten a nested-style object into PluginRule entries.
 * `&` is the parent selector (Tailwind's PostCSS-derived syntax);
 * `@media`/`@supports` keys become at-rule context.
 */
function flatten(
  rules: NestedRules | NestedRules[],
  context: { atRules: string[]; selectorChain: string[] }[] | string[] = [],
  ..._rest: unknown[]
): PluginRule[] {
  // Re-shape the entry — we recurse with two parallel stacks (at-rules
  // and selector path). Translate the cheap-call shape from the
  // top-level signature.
  let atRules: string[] = []
  let selectorChain: string[] = []
  if (Array.isArray(context) && context.length === 1 && typeof context[0] === 'object') {
    atRules = (context[0] as { atRules: string[] }).atRules
    selectorChain = (context[0] as { selectorChain: string[] }).selectorChain
  }
  // Top-level call: context is `[]`, treat as empty stacks.

  const out: PluginRule[] = []

  if (Array.isArray(rules)) {
    for (const r of rules) {
      out.push(...flatten(r, [{ atRules, selectorChain }]))
    }
    return out
  }

  // Buffered declarations for the current selector. Some Tailwind
  // plugins emit `{ '.foo': { color: 'red', '&:hover': { color: 'blue' } } }`
  // — the bare `color: red` belongs to `.foo`, the nested `&:hover`
  // is a separate child rule.
  let bufferedDecls: PluginRule['declarations'] = []
  const flushBuffer = () => {
    if (bufferedDecls.length > 0 && selectorChain.length > 0) {
      out.push({
        selector: composeSelectorChain(selectorChain),
        declarations: bufferedDecls,
        atRules: [...atRules],
      })
      bufferedDecls = []
    }
  }

  for (const [key, value] of Object.entries(rules)) {
    if (typeof value === 'string' || typeof value === 'number') {
      // Plain declaration: `color: 'red'` or `borderWidth: 3`.
      // postcss-js auto-appends `px` when a number is used on a
      // non-unitless property, so `borderTopWidth: 3` becomes
      // `border-top-width: 3px` (matching upstream).
      const property = cssCaseProperty(key)
      bufferedDecls.push({
        property,
        value: cssDeclValue(property, value),
      })
      continue
    }
    if (Array.isArray(value)) {
      // Two cases sharing the array shape:
      //
      // 1. Multi-value: `{ background: ['rgb(...)', 'oklch(...)'] }`.
      //    Each scalar element emits as a separate declaration of
      //    the same property (CSS fallback pattern).
      //
      // 2. Sibling rule list under a selector: `{ '.foo': [
      //    { color: 'red' }, { '.bar': {...} }
      //    ] }`. matchComponents callbacks return this shape so the
      //    component can ship multiple rules (one for the class
      //    itself, one per nested selector). Object elements
      //    flatten with the current `key` pushed onto the chain so
      //    nested selectors compose against the outer selector
      //    (`.foo .bar`, not bare `.bar`).
      const isSelectorKey = !key.startsWith('@') && (typeof rules === 'object')
      const childChainForArray =
        isSelectorKey && (selectorChain.length === 0 ? [key] : [...selectorChain, key])
      for (const v of value) {
        if (typeof v === 'string' || typeof v === 'number') {
          bufferedDecls.push({ property: cssCaseProperty(key), value: String(v) })
        } else if (typeof v === 'object' && v !== null) {
          flushBuffer()
          out.push(
            ...flatten(v as NestedRules, [
              {
                atRules,
                selectorChain: childChainForArray || selectorChain,
              },
            ]),
          )
        }
      }
      continue
    }
    if (typeof value !== 'object' || value === null) continue

    flushBuffer()

    if (key.startsWith('@apply ')) {
      // `@apply <utility>` inside a plugin rule body. Capture it
      // as a special pseudo-declaration so the post-pass can
      // resolve it against the plugin's other components (and
      // optionally fall through to core utilities). Without this
      // the empty-bodied `'@apply a': {}` shape silently drops.
      const target = key.slice('@apply '.length).trim()
      if (target.length > 0) {
        bufferedDecls.push({ property: '@apply', value: target })
      }
      continue
    }
    if (key.startsWith('@')) {
      // At-rule: append to context, recurse. `@keyframes` and
      // `@font-face` are top-level by nature — their bodies use
      // their own selector grammar (percentages, descriptors)
      // unrelated to the surrounding utility class. Reset the
      // selectorChain so we don't compose the class selector with
      // the inner `25.001%`/etc. and emit `.foo-[abc] 25.001%`.
      const isolatesSelector = key.startsWith('@keyframes') || key === '@font-face'
      out.push(
        ...flatten(value as NestedRules, [
          {
            atRules: [...atRules, key],
            selectorChain: isolatesSelector ? [] : selectorChain,
          },
        ]),
      )
      continue
    }

    // Selector key. `&` substitutes for the parent path.
    const childChain =
      selectorChain.length === 0 ? [key] : [...selectorChain, key]
    out.push(
      ...flatten(value as NestedRules, [
        { atRules, selectorChain: childChain },
      ]),
    )
  }

  flushBuffer()
  return out
}

function composeSelectorChain(chain: string[]): string {
  // Walk left-to-right, substituting `&` with the parent at each step.
  // When the composed value is a comma-separated selector list and the
  // next segment uses `&`, distribute the substitution across each part
  // so `["A, B, C", "&:focus"]` produces `"A:focus, B:focus, C:focus"`
  // rather than `"A, B, C:focus"`. This matches PostCSS's own nesting
  // expansion behaviour.
  let composed = chain[0] ?? ''
  for (let i = 1; i < chain.length; i++) {
    const seg = chain[i]!
    if (seg.includes('&')) {
      if (composed.includes(',')) {
        // Split on top-level commas only (not inside parens/brackets).
        const parts = splitTopLevelCommas(composed)
        composed = parts.map((p) => seg.replace(/&/g, p.trim())).join(', ')
      } else {
        composed = seg.replace(/&/g, composed)
      }
    } else {
      composed = `${composed} ${seg}`
    }
  }
  return composed
}

function splitTopLevelCommas(selector: string): string[] {
  const parts: string[] = []
  let depth = 0
  let start = 0
  for (let i = 0; i < selector.length; i++) {
    const ch = selector[i]
    if (ch === '(' || ch === '[') depth++
    else if (ch === ')' || ch === ']') depth--
    else if (ch === ',' && depth === 0) {
      parts.push(selector.slice(start, i))
      start = i + 1
    }
  }
  parts.push(selector.slice(start))
  return parts
}

function cssCaseProperty(prop: string): string {
  // Plugins sometimes use `backgroundColor` (camelCase) for properties.
  // Convert to dash-case for emission.
  if (prop.startsWith('--') || !/[A-Z]/.test(prop)) return prop
  return prop.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase())
}

/**
 * postcss-js' UNITLESS table — properties whose numeric values
 * stay unitless. Anything else gets `px` appended when the value
 * is a number (and not 0). Mirrors
 * `node_modules/postcss-js/parser.js`.
 */
const UNITLESS_PROPERTIES = new Set([
  'box-flex',
  'box-flex-group',
  'column-count',
  'flex',
  'flex-grow',
  'flex-positive',
  'flex-shrink',
  'flex-negative',
  'font-weight',
  'line-clamp',
  'line-height',
  'opacity',
  'order',
  'orphans',
  'tab-size',
  'widows',
  'z-index',
  'zoom',
  'fill-opacity',
  'stroke-dashoffset',
  'stroke-opacity',
  'stroke-width',
])

function cssDeclValue(property: string, value: string | number): string {
  if (typeof value === 'number') {
    if (value === 0 || UNITLESS_PROPERTIES.has(property)) {
      return value.toString()
    }
    return `${value}px`
  }
  return value
}

function toVariantRecord(
  name: string,
  formats:
    | string
    | string[]
    | ((api: {
        container: unknown
        modifySelectors?: (cb: (args: { className: string }) => string) => void
        separator: string
      }) => unknown),
  warnings: string[],
): PluginVariant {
  // Function-form variants get a tiny shim: invoke the function with
  // a stub `modifySelectors` that records the returned selector
  // template, plus a `container` whose `walkRules` mirrors the
  // postcss API enough for common plugins. This covers the
  // `addVariant('foo', ({ modifySelectors }) => modifySelectors(({
  // className }) => `[data-foo] .${className}`))` shape used by
  // many real plugins (e.g. tailwindcss-animated). Plugins that
  // rely on full postcss container semantics still produce empty
  // records — flagged via warning.
  if (typeof formats === 'function') {
    const declValueOut: { template: string | null } = { template: null }
    const captured = invokeFunctionVariant(name, formats, warnings, declValueOut)
    if (captured.length === 0) {
      warnings.push(
        `${WARNING_PREFIX} addVariant("${name}", <function>) — couldn't capture selector format; the function may rely on full postcss container semantics not supplied by the shim`,
      )
      return { name, selectorFormats: [], atRule: null, declValueTemplate: declValueOut.template }
    }
    const selectorFormats: string[] = []
    let atRule: string | null = null
    for (const f of captured) {
      if (f.startsWith('@')) atRule = f
      else selectorFormats.push(f)
    }
    return { name, selectorFormats, atRule, declValueTemplate: declValueOut.template }
  }
  const list = Array.isArray(formats) ? formats : [formats]
  const selectorFormats: string[] = []
  let atRule: string | null = null
  for (const f of list) {
    if (typeof f !== 'string') continue
    // The shorthand format string can nest at-rules and a
    // selector inside braces:
    // `@supports (hover: hover) { @media print { &:disabled } }`.
    // Mirrors upstream's `parseVariantFormatString` which splits
    // on `{`/`}`. We pull out at-rules into the chain and the
    // `&`-bearing selector into `selectorFormats`.
    if (f.includes('{')) {
      const parts = parseVariantFormatString(f)
      for (const p of parts) {
        if (p.startsWith('@')) {
          atRule = atRule === null ? p : `${atRule} > ${p}`
        } else if (p.includes('&')) {
          selectorFormats.push(p)
        }
      }
      continue
    }
    if (f.startsWith('@')) {
      atRule = atRule === null ? f : `${atRule} > ${f}`
    } else {
      selectorFormats.push(f)
    }
  }
  return { name, selectorFormats, atRule }
}

/// Mirrors upstream's `parseVariantFormatString` from
/// `setupContextUtils.js`. Splits on `{` / `}` while preserving
/// escapes and tracks nesting depth so the at-rule chain inside
/// the braces is captured separately from the trailing selector.
function parseVariantFormatString(input: string): string[] {
  const parts: string[] = []
  let current = ''
  let depth = 0
  for (let idx = 0; idx < input.length; idx++) {
    const ch = input[idx]
    if (ch === '\\' && idx + 1 < input.length) {
      current += ch + input[++idx]
      continue
    }
    if (ch === '{') {
      depth++
      const trimmed = current.trim()
      if (trimmed.length > 0) parts.push(trimmed)
      current = ''
      continue
    }
    if (ch === '}') {
      depth--
      if (depth < 0) break
      const trimmed = current.trim()
      if (trimmed.length > 0) parts.push(trimmed)
      current = ''
      continue
    }
    current += ch
  }
  const tail = current.trim()
  if (tail.length > 0) parts.push(tail)
  return parts.filter((p) => p.length > 0)
}

/**
 * Capture selector formats from a function-form `addVariant`
 * callback. The Tailwind PostCSS plugin invokes these with a
 * `container` plus a `modifySelectors` helper. For our purposes we
 * only need `modifySelectors` — the most common use — and substitute
 * a sentinel class name we can then extract.
 */
function invokeFunctionVariant(
  _name: string,
  fn: (api: {
    container: unknown
    modifySelectors?: (cb: (args: { className: string }) => string) => void
    separator: string
  }) => unknown,
  _warnings: string[],
  declValueOut?: { template: string | null },
): string[] {
  const sentinel = '__GALEFORCE_AMP__'
  const declValueSentinel = '__GALEFORCE_DECL_VALUE__'
  // Two capture buckets: formats returned directly by the plugin
  // (the canonical `addVariant('foo', () => '&:hover')` shape),
  // and selectors observed via `modifySelectors`. We prefer the
  // returned formats when both exist — upstream uses
  // modifySelectors as a side-effect-only API on existing rules,
  // not as an additional format source. Mixing them duplicates
  // rules under our static-format model.
  const returned: string[] = []
  const fromModifySelectors: string[] = []
  let containerWalked = false
  // Replace any class-selector chunk that contains the sentinel
  // with `&`. Plugins commonly build the selector by concatenating
  // a prefix with the className (`.foo\:${e(name)}` etc.), so the
  // sentinel ends up wedged inside a longer escaped class ident.
  // Walk the selector and substitute each class-shaped run that
  // contains the sentinel.
  const ampifySentinel = (sel: string): string => {
    const i = sel.indexOf(sentinel)
    if (i < 0) return sel
    // Walk backwards from `i` to find the start of the class
    // identifier (`.`). Include `\` escapes — `\.foo\:bar` is one
    // identifier from the leading `.` until the next combinator
    // / boundary.
    let start = i
    while (start > 0) {
      const ch = sel[start - 1]
      if (ch === '.' && (start === 1 || !/[\w-]/.test(sel[start - 2] ?? ''))) {
        start = start - 1
        break
      }
      if (
        ch === ' ' ||
        ch === '\t' ||
        ch === '>' ||
        ch === '+' ||
        ch === '~' ||
        ch === ',' ||
        ch === '(' ||
        ch === ')' ||
        ch === '['
      ) {
        break
      }
      start--
    }
    let end = i + sentinel.length
    while (end < sel.length) {
      const ch = sel[end]
      if (
        ch === ' ' ||
        ch === '\t' ||
        ch === '>' ||
        ch === '+' ||
        ch === '~' ||
        ch === ',' ||
        ch === '(' ||
        ch === ')' ||
        ch === '['
      ) {
        break
      }
      end++
    }
    return ampifySentinel(sel.slice(0, start) + '&' + sel.slice(end))
  }
  // Stub container with a walkRules that mimics postcss's interface
  // enough to let plugins that iterate rules and prepend selectors
  // observe a single sentinel rule. `walkDecls` is exposed too as a
  // no-op so plugins that mutate declaration values (`decl.value =
  // ...`) don't throw before they get a chance to return their
  // selector formats.
  const container = {
    walkRules(cb: (rule: { selector: string; nodes: unknown[] }) => void) {
      containerWalked = true
      const rule = {
        selector: `.${sentinel}`,
        nodes: [],
      }
      cb(rule)
      // The plugin may have mutated `rule.selector`; capture it.
      if (typeof rule.selector === 'string' && rule.selector !== `.${sentinel}`) {
        fromModifySelectors.push(ampifySentinel(rule.selector))
      }
    },
    walkDecls(cb: (decl: { prop: string; value: string }) => void) {
      // Hand the plugin a single sentinel declaration with a
      // unique value. After the callback returns, inspect how the
      // value was mutated so we can capture a transform template
      // (e.g. `calc(0 + ${value})` -> `calc(0 + {})`). Stored on
      // `declValueOut.template` when present.
      if (!declValueOut) return
      const decl = { prop: 'galeforce-x', value: declValueSentinel }
      try {
        cb(decl)
      } catch {
        // Plugin may rely on more API; just record what changed.
      }
      if (typeof decl.value === 'string' && decl.value !== declValueSentinel) {
        // Replace each occurrence of the sentinel with `{}` so the
        // Rust applier can substitute the resolved value back in.
        declValueOut.template = decl.value.split(declValueSentinel).join('{}')
      }
    },
    each(_cb: (n: unknown) => void) {
      // Some plugins iterate the container; expose nothing.
    },
  }
  const modifySelectors = (cb: (args: { className: string }) => string) => {
    try {
      const replaced = cb({ className: sentinel })
      if (typeof replaced === 'string') {
        fromModifySelectors.push(ampifySentinel(replaced))
      }
    } catch {
      // Plugin threw inside the callback; nothing to capture.
    }
  }
  try {
    const result = fn({ container, modifySelectors, separator: ':' })
    if (typeof result === 'string') {
      // Plugin returned a format string only when it's `&`-bearing
      // or starts with `@` (upstream's `isValidVariantFormatString`).
      // Other return values are ignored — `modifySelectors` already
      // captured the actual selector via its callback.
      if (result.includes('&') || result.startsWith('@')) {
        returned.push(result)
      }
    } else if (Array.isArray(result)) {
      for (const r of result) {
        if (typeof r === 'string' && (r.includes('&') || r.startsWith('@'))) {
          returned.push(r)
        }
      }
    }
  } catch {
    // Plugin's outer body threw — e.g. relied on something we didn't
    // supply. Whatever was captured before the throw still counts.
  }
  void containerWalked
  // Prefer plugin-returned formats. Fall back to the
  // `modifySelectors` capture when the plugin didn't return any
  // format strings (the older modifySelectors-only style).
  return returned.length > 0 ? returned : fromModifySelectors
}

function materializeMatch(
  utilities: Record<
    string,
    (value: string, ctx?: { modifier: string | null }) => NestedRules | NestedRules[]
  >,
  options: MatchOptions | undefined,
): PluginRule[] {
  const out: PluginRule[] = []
  const values = options?.values ?? {}
  for (const [classPrefix, fn] of Object.entries(utilities)) {
    for (const [valueKey, valueValue] of Object.entries(values)) {
      const className =
        valueKey === 'DEFAULT' ? `.${classPrefix}` : `.${classPrefix}-${valueKey}`
      let result: NestedRules | NestedRules[]
      try {
        // matchUtilities's callback receives `(value, { modifier })`
        // upstream. Pass an empty modifier so destructuring callbacks
        // don't throw on plugin functions that expect the option
        // argument.
        result = fn(
          typeof valueValue === 'string' ? valueValue : String(valueValue),
          { modifier: null },
        )
      } catch {
        continue
      }
      // Wrap the result so the top-level becomes the className.
      // Tag every flattened rule with the candidate class as
      // `groupClass` so the Rust compiler can pull in companion
      // rules (e.g. `@keyframes` blocks emitted alongside a class
      // rule that references them by name) when the candidate
      // matches. Without the tag those companion rules — which
      // have no class selector of their own — get filtered out by
      // primary-class lookup and never emitted.
      const wrapped: NestedRules = { [className]: result as NestedRules }
      const groupKey = className.startsWith('.') ? className.slice(1) : className
      const flat = flatten(wrapped, [])
      for (const r of flat) {
        ;(r as PluginRule & { groupClass?: string }).groupClass = groupKey
      }
      out.push(...flat)
    }
  }
  return out
}

function lookupDotPath(obj: Record<string, unknown>, path: string): unknown {
  let node: unknown = obj
  // Tokenize: dot-separated segments, with `[xxx]` bracket form
  // letting a segment contain literal `.`. Mirrors upstream's
  // `vendor/tailwindcss-v3/src/util/toPath.js`.
  const segments = toPathSegments(path)
  for (const seg of segments) {
    if (node === null || typeof node !== 'object') return undefined
    node = (node as Record<string, unknown>)[seg]
  }
  return node
}

function toPathSegments(path: string): string[] {
  const out: string[] = []
  let current = ''
  let inBracket = false
  for (const ch of path) {
    if (ch === '[') {
      if (current) {
        out.push(current)
        current = ''
      }
      inBracket = true
    } else if (ch === ']') {
      if (current) {
        out.push(current)
        current = ''
      }
      inBracket = false
    } else if (ch === '.' && !inBracket) {
      if (current) {
        out.push(current)
        current = ''
      }
    } else {
      current += ch
    }
  }
  if (current) out.push(current)
  return out
}

/**
 * Apply the theme-section-specific render that upstream's
 * `transformThemeValue.js` would apply when a plugin calls
 * `theme('boxShadow.foo')` and gets an array back. Without this,
 * plugins emit one decl per array element instead of a single
 * comma-joined value.
 */
function transformThemeValue(section: string, value: unknown): unknown {
  if (typeof value === 'function') {
    try {
      value = (value as (opts: Record<string, unknown>) => unknown)({})
    } catch {
      /* fall through */
    }
  }
  switch (section) {
    case 'fontSize':
    case 'outline':
      if (Array.isArray(value)) return value[0]
      return value
    case 'fontFamily': {
      let families = value
      if (Array.isArray(value) && value[1] && typeof value[1] === 'object' && !Array.isArray(value[1])) {
        families = value[0]
      }
      return Array.isArray(families) ? families.join(', ') : families
    }
    case 'boxShadow':
    case 'transitionProperty':
    case 'transitionDuration':
    case 'transitionDelay':
    case 'transitionTimingFunction':
    case 'backgroundImage':
    case 'backgroundSize':
    case 'backgroundColor':
    case 'cursor':
    case 'animation':
      if (Array.isArray(value)) return value.join(', ')
      return value
    default:
      return value
  }
}
