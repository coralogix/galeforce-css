// Load and resolve a `tailwind.config.{js,cjs,mjs,ts,cts,mts}` into the
// JSON shape the GaleforceCSS Rust compiler consumes.
//
// We deliberately delegate the heavy lifting to Tailwind's own
// `resolveConfig` (imported from the pinned `tailwindcss@3.4.19` npm
// package). That keeps the merge logic — default theme + user theme +
// `theme.extend` — bit-for-bit identical to what the oracle does. We
// just have to:
//
//   1. Find the config file (mirrors Tailwind's `resolveConfigPath.js`
//      lookup order).
//   2. Load it via `jiti` so .ts/.mjs/.cjs all work without a build step.
//   3. Strip user-defined plugins (we don't run them in v1; warn if
//      present so the developer knows their plugin isn't doing anything).
//   4. Hand the bare object to `resolveConfig` for the merge.
//   5. Return as a JSON-safe Record (no functions, no symbols) so it
//      can be passed through to the Rust compiler over the napi
//      boundary or via the JSON CLI bridge.

import { existsSync } from 'node:fs'
import { resolve as resolvePath, dirname } from 'node:path'
import { createJiti } from 'jiti'
import postcss from 'postcss'
import { runPlugins } from './plugin-runner.js'
import { resolveTailwind } from './resolve-tailwind.js'

/**
 * Default config filenames Tailwind looks for, in priority order. Mirrors
 * `defaultConfigFiles` in `vendor/tailwindcss-v3/src/util/resolveConfigPath.js`.
 */
export const DEFAULT_CONFIG_FILES = [
  './tailwind.config.js',
  './tailwind.config.cjs',
  './tailwind.config.mjs',
  './tailwind.config.ts',
  './tailwind.config.cts',
  './tailwind.config.mts',
] as const

export interface LoadConfigOptions {
  /**
   * Explicit path to the config file. If not provided, walks up from
   * `cwd` looking for any of `DEFAULT_CONFIG_FILES`.
   */
  path?: string
  /**
   * Search root. Defaults to `process.cwd()`. Only consulted when `path`
   * is not given.
   */
  cwd?: string
}

/**
 * Captured output from a Tailwind plugin run. Plugins call helper
 * functions like `addUtilities`, `addComponents`, `addBase`,
 * `addVariant`, `matchUtilities` against a context object; we record
 * those calls and ship the captured data to the Rust compiler so it
 * can merge plugin-defined utilities/variants alongside built-ins.
 *
 * `matchUtilities` is materialized at JS-time: for each `value` in
 * the plugin's `values` option we invoke the function and capture the
 * resulting declarations as a static utility. This loses runtime
 * dynamism (the function can't generate values per-candidate at
 * compile time) but covers the common case of "plugin-defined values
 * keyed by theme key".
 */
export interface PluginOutput {
  /** `addUtilities` + materialized `matchUtilities`. Emitted at the
   * `@tailwind utilities;` slot. */
  utilities: PluginRule[]
  /** `addComponents` + materialized `matchComponents`. */
  components: PluginRule[]
  /** `addBase` calls (CSS that lands in the base layer). Emitted as
   * top-level rules inside the base slot. */
  base: PluginRule[]
  /** `addVariant` / `matchVariant` registrations. */
  variants: PluginVariant[]
}

export interface PluginRule {
  selector: string
  /** Property -> value pairs. Note: a single rule with multiple
   * declarations preserves order. */
  declarations: Array<{ property: string; value: string; important?: boolean }>
  /** At-rule wrapping context, outermost-first.
   * E.g. `["@media (min-width: 768px)"]`. */
  atRules: string[]
  /** Whether this rule should pick up the user's
   * `important: true` / `important: '<sel>'` config. Defaults to
   * `true` for `addUtilities` and `false` for `addComponents` —
   * plugins can override per-call via the `respectImportant`
   * option. The Rust compiler reads this when applying the
   * global important config. */
  respectImportant?: boolean
}

export interface PluginVariant {
  name: string
  /** Selector format(s) containing `&` for substitution, e.g.
   * `["&:hover", "&:focus"]` for `addVariant('hocus', […])`. May be
   * empty if the variant is purely an at-rule. */
  selectorFormats: string[]
  /** At-rule string without trailing braces, e.g.
   * `"@supports (display: grid)"`. */
  atRule: string | null
  /** Optional declaration-value transform captured from a
   *  function-form addVariant that mutates `decl.value` via
   *  `container.walkDecls`. Stored as a template where `{}` marks
   *  the original value position (e.g. `calc(0 + {})`). The Rust
   *  compiler applies it to every declaration of rules using this
   *  variant after the value resolves. `null` when the function
   *  didn't touch decl values. */
  declValueTemplate?: string | null
}

export interface LoadedConfig {
  /** Absolute path to the loaded file, or `null` if no file was found. */
  path: string | null
  /** The user's *raw* exported config (before resolveConfig merges defaults). */
  raw: Record<string, unknown>
  /** Tailwind-`resolveConfig`-merged config. JSON-safe. */
  resolved: Record<string, unknown>
  /** Plugin names the user declared. */
  pluginNames: string[]
  /** Captured output from running the plugins. Shipped to the Rust
   * compiler under `config.__pluginOutput`. */
  pluginOutput: PluginOutput
}

/**
 * Locate the tailwind config file. Returns `null` if none of the default
 * filenames exist in `cwd`.
 */
export function findConfigPath(cwd: string = process.cwd()): string | null {
  for (const candidate of DEFAULT_CONFIG_FILES) {
    const full = resolvePath(cwd, candidate)
    if (existsSync(full)) return full
  }
  return null
}

/**
 * Load a Tailwind config from disk and resolve it through Tailwind's own
 * merger. If neither `opts.path` nor any default filename in `cwd` exists,
 * returns a `LoadedConfig` with `path: null` and `raw: {}` — the resolved
 * config is the pure default theme (still useful, e.g. for `theme()` in
 * input CSS without a project config).
 */
export async function loadConfig(opts: LoadConfigOptions = {}): Promise<LoadedConfig> {
  const cwd = opts.cwd ?? process.cwd()
  const path = opts.path ? resolvePath(cwd, opts.path) : findConfigPath(cwd)

  let raw: Record<string, unknown> = {}
  if (path !== null) {
    if (!existsSync(path)) {
      throw new Error(`tailwind config not found at ${path}`)
    }
    const jiti = createJiti(import.meta.url, {
      // Cache off — the loader is intended for one-shot CLI runs and
      // dev-server warmup; we want the user's edits to take effect on
      // re-load without manual cache busting. The Vite plugin will
      // call us once on server start and again on file change.
      fsCache: false,
      moduleCache: false,
    })
    const mod = await jiti.import<{ default?: Record<string, unknown> } & Record<string, unknown>>(
      path,
    )
    // Handle both ESM (`export default {...}`) and CJS (`module.exports = {...}`).
    raw = (mod.default ?? mod) as Record<string, unknown>
  }

  return await processRawConfigAsync(raw, path)
}

/**
 * Async variant of `processRawConfig` that ALSO resolves pattern-form
 * safelist entries (`{ pattern: /…/, variants: [...] }`) into flat
 * strings. Resolution piggybacks on Tailwind's own JIT: we run a stub
 * Tailwind compile with the user's theme and just the safelist
 * patterns, capture the resulting class names, and merge them into a
 * canonical string-form safelist that the Rust compiler can consume
 * via the existing `safelist: [...]` path.
 */
export async function processRawConfigAsync(
  raw: Record<string, unknown>,
  path: string | null = null,
  candidates: readonly string[] = [],
): Promise<LoadedConfig> {
  const result = processRawConfig(raw, path, candidates)
  const patternStrings = await resolvePatternSafelist(raw, path)
  if (patternStrings.length > 0) {
    // Merge into the resolved safelist as flat strings. The Rust
    // compiler already handles flat-string safelist; pattern form
    // is now a JS-side translation step.
    const existing = (result.resolved.safelist as unknown[] | undefined) ?? []
    const stringExisting = (existing as unknown[]).filter((v): v is string => typeof v === 'string')
    result.resolved.safelist = [...stringExisting, ...patternStrings]
  }
  // If the user overrode any of the theme paths preflight reads via
  // embedded `theme()` calls (`borderColor.DEFAULT`, `fontFamily.sans`,
  // `fontFamily.mono`, `colors.gray.400`), our vendored preflight
  // would diverge. Detect those overrides and pre-resolve the base
  // block via Tailwind itself; ship the resolved string under
  // `config.__resolvedBase` for Rust to use instead of the vendored
  // form.
  if (
    preflightThemeOverridden(raw) ||
    corePluginsRestricted(raw) ||
    baseBlockThemeOverridden(raw) ||
    futureFlagsSet(raw)
  ) {
    try {
      const resolvedBase = await resolveBaseBlock(raw, path)
      if (resolvedBase !== null) {
        // An empty string is meaningful — when corePlugins is
        // restricted to a set that triggers no cascade-var defaults
        // (and preflight is off), Tailwind emits nothing. We must
        // store that empty string so the Rust compiler suppresses
        // the vendored block instead of falling back to it.
        ;(result.resolved as Record<string, unknown>).__resolvedBase = resolvedBase
      }
    } catch {
      // Best-effort; if Tailwind throws we keep the vendored form.
    }
  }
  return result
}

/// True when the user overrides theme paths that the cascade-var
/// defaults block reads (`ringColor.DEFAULT`,
/// `ringOpacity.DEFAULT`, `boxShadow.DEFAULT`, etc.). When set,
/// the vendored base block diverges from what Tailwind would
/// emit for this user, so we re-run Tailwind to capture the
/// correct values.
function baseBlockThemeOverridden(raw: Record<string, unknown>): boolean {
  const theme = (raw.theme as Record<string, unknown> | undefined) ?? {}
  const extend = (theme.extend as Record<string, unknown> | undefined) ?? {}
  const keys = [
    'ringColor',
    'ringOpacity',
    'ringWidth',
    'ringOffsetWidth',
    'ringOffsetColor',
    'boxShadow',
    'boxShadowColor',
  ]
  for (const k of keys) {
    if (theme[k] !== undefined || extend[k] !== undefined) return true
  }
  return false
}

/// True when any `future.*` or `experimental.*` flag is set. These
/// flags can change cascade-var default emission (e.g.
/// `respectDefaultRingColorOpacity`, `optimizeUniversalDefaults`),
/// so the base block must be re-resolved by Tailwind rather than
/// served from the vendored form. `optimizeUniversalDefaults`
/// specifically lives under `experimental` in v3.4.19.
function futureFlagsSet(raw: Record<string, unknown>): boolean {
  for (const key of ['future', 'experimental'] as const) {
    const v = raw[key]
    if (typeof v === 'object' && v !== null && Object.keys(v).length > 0) {
      return true
    }
  }
  return false
}

/// True when the user has restricted the set of enabled core
/// plugins via `corePlugins: [...]` (allowlist) or
/// `corePlugins: { foo: false, ... }` (blocklist excluding more
/// than just `preflight`). Either form changes which `--tw-*`
/// cascade-var defaults the base block emits, so the vendored
/// form is no longer accurate — we re-run Tailwind to get the
/// right subset.
function corePluginsRestricted(raw: Record<string, unknown>): boolean {
  const cp = raw.corePlugins
  if (Array.isArray(cp)) return true
  if (cp && typeof cp === 'object') {
    for (const [k, v] of Object.entries(cp)) {
      if (k === 'preflight') continue // already handled
      if (v === false) return true
    }
  }
  return false
}

/** Did the user override any of the four theme paths preflight reads? */
function preflightThemeOverridden(raw: Record<string, unknown>): boolean {
  const theme = (raw.theme as Record<string, unknown> | undefined) ?? {}
  const extend = (theme.extend as Record<string, unknown> | undefined) ?? {}
  for (const t of [theme, extend]) {
    // ANY override of `borderColor`/`fontFamily` (function-form,
    // object-with-DEFAULT, full replacement) means we have to ask
    // Tailwind for the resolved preflight — function values need
    // the API call to produce a value, and full replacements may
    // omit `DEFAULT`/`sans`/`mono`.
    if (t.borderColor !== undefined) return true
    if (typeof t.fontFamily === 'function') return true
    if (t.fontFamily && typeof t.fontFamily === 'object') {
      const ff = t.fontFamily as Record<string, unknown>
      if (ff.sans !== undefined || ff.mono !== undefined) return true
    }
    const colors = t.colors
    if (typeof colors === 'function') return true
    if (colors && typeof colors === 'object') {
      const cobj = colors as Record<string, unknown>
      if (cobj.gray && typeof cobj.gray === 'object') return true
      if (Object.keys(cobj).length > 0) return true
    }
  }
  return false
}

async function resolveBaseBlock(
  raw: Record<string, unknown>,
  configPath: string | null,
): Promise<string | null> {
  const stub: Record<string, unknown> = {
    ...raw,
    content: [{ raw: '', extension: 'html' }],
    plugins: [], // Skip plugins — they don't affect preflight.
  }
  const tw = resolveTailwind(configPath ? dirname(configPath) : null)
  const tailwindcss = tw.tailwindcss as (cfg: unknown) => postcss.AcceptedPlugin
  const result = await postcss([tailwindcss(stub)]).process('@tailwind base;', {
    from: undefined,
  })
  // Return the string as-is — including an empty string when the
  // user restricted corePlugins down to a set that doesn't trigger
  // any cascade-var defaults or preflight. The Rust compiler reads
  // this in place of the vendored block; an empty string correctly
  // suppresses our default emission.
  return result.css
}

/**
 * Convert a raw config object (already loaded into memory) into a
 * `LoadedConfig`. Useful for tests that build a plugin config inline
 * without writing to disk, and for callers like the Vite plugin that
 * already have the user's exports cached.
 */
export function processRawConfig(
  raw: Record<string, unknown>,
  path: string | null = null,
  candidates: readonly string[] = [],
): LoadedConfig {
  const pluginNames = collectPluginNames(raw)
  const stripped = stripPlugins(raw)
  // Resolve the user's `tailwindcss` install from the directory of
  // the config file. This is what lets us pick up v3.3.x's
  // `defaultTheme` when the project is pinned at v3.3.x even though
  // galeforcecss bundles ^3.4.19 as a fallback peer. With no `path` we
  // fall back to the bundled copy unconditionally.
  const tw = resolveTailwind(path ? dirname(path) : null)
  const resolvedRaw = tw.resolveConfig(stripped) as Record<string, unknown>
  // `darkMode: ['variant', () => [...]]` ships a function that
  // jsonSafe would drop. Invoke it once at config-load time and
  // replace with the resolved string-array form before the
  // resolved config ships to Rust. Mirrors upstream's lazy-call
  // approach in `setupContextUtils.js` — they invoke the fn once
  // per build.
  resolveDarkModeFn(resolvedRaw)
  // Function-form theme colors (`colors: { blue: ({ opacityValue })
  // => '...' }`) are dropped by `jsonSafe`. Pre-materialize them by
  // invoking the function for each opacity-modifier seen in the
  // candidate set (and a default `1` form so plain `bg-blue` works
  // without a modifier). Replaces the function entry with a nested
  // object the existing color resolver picks up via slash-keyed
  // first-match.
  resolveFunctionThemeColors(resolvedRaw, candidates)
  const resolved = jsonSafe(resolvedRaw) as Record<string, unknown>
  // Run plugins against a recording context AFTER resolveConfig so
  // they see the merged theme (the most common pattern: `theme()`
  // lookups inside the plugin body). Captured output is shipped to
  // the Rust compiler under `__pluginOutput`. When `candidates` is
  // supplied the runner ALSO materializes arbitrary-value forms
  // (`my-[checked]`, `tab-[3.5]`) by invoking the plugin function
  // for each unique arbitrary value found in the candidate set —
  // mirroring Tailwind v3's JIT behavior.
  // Plugins can live in the user's raw config OR in any preset they
  // extend. Tailwind's `resolveConfig` merges presets transitively but
  // doesn't expose a single flat plugin list — we collect that
  // ourselves so plugin-emitted utilities defined in a base config
  // (e.g. a workspace `tailwind.config.ts` reused via
  // `presets: [base]`) still get captured.
  const allPlugins = collectAllPlugins(raw)
  const { output: pluginOutput, warnings } = runPlugins(allPlugins, resolved, candidates)
  for (const w of warnings) {
    // eslint-disable-next-line no-console
    console.warn(w)
  }
  // Embed the captured plugin output into the resolved config so it
  // flows through the existing JSON CompileOptions.config wire format
  // without a new top-level field. Rust reads `__pluginOutput` if
  // present.
  ;(resolved as Record<string, unknown>).__pluginOutput = pluginOutput

  return { path, raw, resolved, pluginNames, pluginOutput }
}

/**
 * Walk `raw.plugins` AND every entry of `raw.presets` recursively,
 * returning the combined flat plugin list. Mirrors Tailwind's preset
 * resolution: a preset is just a sub-config whose plugins (and own
 * presets) merge into the consumer.
 *
 * Inline preset objects (`presets: [{...}]`) are walked directly.
 * Preset modules with a `default` export (`presets: [require('./base')]`)
 * are unwrapped one level. Anything else (functions, primitives) is
 * skipped — Tailwind itself errors on those.
 */
function collectAllPlugins(raw: Record<string, unknown>): unknown[] {
  const out: unknown[] = []
  const visit = (cfg: unknown): void => {
    if (!cfg || typeof cfg !== 'object') return
    const obj = (cfg as { default?: unknown }).default ?? cfg
    if (!obj || typeof obj !== 'object') return
    const plugins = (obj as { plugins?: unknown }).plugins
    if (Array.isArray(plugins)) {
      for (const p of plugins) out.push(p)
    }
    const presets = (obj as { presets?: unknown }).presets
    if (Array.isArray(presets)) {
      for (const preset of presets) visit(preset)
    }
  }
  visit(raw)
  return out
}

function collectPluginNames(raw: Record<string, unknown>): string[] {
  const plugins = collectAllPlugins(raw)
  return plugins
    .map((p) => {
      if (typeof p === 'function') return (p as { name?: string }).name || '(anonymous)'
      if (typeof p === 'object' && p !== null && 'handler' in p) return '(plugin object)'
      if (typeof p === 'object' && p !== null && 'config' in p) return '(plugin object)'
      return String(p)
    })
    .filter((s) => s.length > 0)
}

function stripPlugins(raw: Record<string, unknown>): Record<string, unknown> {
  // Plugins can carry a `config` payload (`plugins: [{ config:
  // { prefix: 'tw-' } }]`). Tailwind's `resolveConfig` would merge
  // that config in via the preset-like flow, but we strip plugins
  // before calling it. To preserve the merge, hoist any
  // `entry.config` payloads into a synthetic preset that
  // resolveConfig will see. Function-form plugins (no `config`)
  // are dropped outright.
  const { plugins, presets, ...rest } = raw
  const hoistedPresets: Array<Record<string, unknown>> = []
  if (Array.isArray(plugins)) {
    for (const entry of plugins as unknown[]) {
      if (
        entry &&
        typeof entry === 'object' &&
        !Array.isArray(entry) &&
        typeof (entry as { config?: unknown }).config === 'object' &&
        (entry as { config?: unknown }).config !== null
      ) {
        hoistedPresets.push(
          (entry as { config: Record<string, unknown> }).config,
        )
      }
    }
  }
  const existingPresets = Array.isArray(presets) ? (presets as unknown[]) : []
  const mergedPresets = [...existingPresets, ...hoistedPresets]
  if (mergedPresets.length > 0) {
    return { ...rest, presets: mergedPresets }
  }
  return rest
}

/**
 * Recursively turn a value into something `JSON.stringify` will accept
 * losslessly: drop function values, replace with `null`. Tailwind's
 * resolved config can contain function values (e.g. theme.fontFamily
 * defaults are arrays of strings, but theme.spacing references a
 * `({theme}) => ...` lambda before resolveConfig evaluates it. After
 * resolveConfig those should be flat, but stragglers exist in some
 * paths — `[].theme.colors` for instance has function-shaped color
 * helpers).
 */
/**
 * If `darkMode` is `['variant', fn]` (or `[fn]`), invoke each
 * function with `('&')` and replace it with the returned string /
 * string array in-place. Tailwind's setupContextUtils calls the fn
 * the same way (with the rule's selector); using the upstream
 * placeholder character keeps the resulting selectors composable
 * with our existing variant chain.
 */
function resolveDarkModeFn(resolved: Record<string, unknown>): void {
  const dm = resolved.darkMode
  if (!Array.isArray(dm)) return
  const out: unknown[] = []
  for (const item of dm) {
    if (typeof item === 'function') {
      try {
        const result = (item as (sel: string) => string | string[])('&')
        if (Array.isArray(result)) {
          out.push(...result)
        } else if (typeof result === 'string') {
          out.push(result)
        }
      } catch {
        // Best-effort; swallow throws and drop the entry.
      }
      continue
    }
    out.push(item)
  }
  resolved.darkMode = out
}

/**
 * Find function-shaped color entries in the resolved theme and
 * pre-invoke them with each opacity value found in the candidate
 * set. Mirrors upstream's `withAlphaValue` runtime behavior:
 * `theme.colors.blue = ({ opacityValue }) => ...` gets called with
 * `{ opacityValue: 0.5 }` for `bg-blue/50`. Since we can't ship
 * functions through the JSON wire to Rust, replace the function
 * with `{ DEFAULT: <fn(1)>, '50': <fn(0.5)>, ... }` and let the
 * existing color resolver match by slashed-key first.
 *
 * Walks `theme.colors` and `theme.<plugin>` keys that hold a color
 * value table (heuristic: any object whose values can be functions
 * or strings). Function values DEEP inside (e.g.
 * `colors.blue.500 = (...) => ...`) are also handled.
 */
function resolveFunctionThemeColors(
  resolved: Record<string, unknown>,
  candidates: readonly string[],
): void {
  const theme = resolved.theme
  if (!theme || typeof theme !== 'object') return
  const { opacityValues, modifierLabels } = collectOpacityModifiersWithLabels(
    candidates,
    resolved,
  )
  // Always include `1` as the default so plain `bg-blue` works
  // without a modifier (matches upstream's no-modifier code path).
  if (!opacityValues.has('1')) opacityValues.add('1')
  walkAndMaterialize(
    theme as Record<string, unknown>,
    opacityValues,
    modifierLabels,
    resolved,
  )
}

function walkAndMaterialize(
  obj: Record<string, unknown>,
  opacityValues: ReadonlySet<string>,
  modifierLabels: ReadonlyMap<string, string>,
  resolved: Record<string, unknown>,
): void {
  // Snapshot the entries so we can mutate the object during the
  // walk (function entries become an object + slashed siblings).
  const entries = Object.entries(obj)
  for (const [k, v] of entries) {
    if (typeof v === 'function') {
      const fn = v as (extras: Record<string, unknown>) => unknown
      // Keep a DEFAULT entry under the original key (so plain
      // `bg-blue` works) AND inject sibling slashed keys for each
      // opacity modifier observed in the candidate set (so
      // `bg-blue/50` resolves via our existing slashed-first-match
      // path in the color resolver).
      const defaultResult = invokeFnSafe(fn, '1')
      if (typeof defaultResult === 'string') {
        obj[k] = defaultResult
      }
      for (const v of opacityValues) {
        if (v === '1') continue
        const result = invokeFnSafe(fn, v)
        if (typeof result !== 'string') continue
        const label = modifierLabels.get(v) ?? v
        obj[`${k}/${label}`] = result
      }
      continue
    }
    if (v && typeof v === 'object' && !Array.isArray(v)) {
      walkAndMaterialize(
        v as Record<string, unknown>,
        opacityValues,
        modifierLabels,
        resolved,
      )
    }
  }
}

function invokeFnSafe(
  fn: (extras: Record<string, unknown>) => unknown,
  opacityValue: string,
): unknown {
  try {
    return fn({ opacityValue, opacityVariable: '' })
  } catch {
    return null
  }
}

/**
 * Walk every candidate, find any `/<modifier>` segment that looks
 * like an opacity value (fraction-of-100 alpha), and return the
 * resolved alpha (e.g. `50` -> `0.5`). Arbitrary opacity forms
 * (`/[0.45]`) keep the inner value verbatim.
 */
function collectOpacityModifiersWithLabels(
  candidates: readonly string[],
  resolved: Record<string, unknown>,
): { opacityValues: Set<string>; modifierLabels: Map<string, string> } {
  const opacityValues = new Set<string>()
  // Maps the resolved opacity value back to the candidate's source
  // label (e.g. `0.5` -> `50` so we can store `colors.blue/50` in
  // the materialized theme, matching the slashed-first-match key
  // the color resolver expects).
  const modifierLabels = new Map<string, string>()
  const opacityTable =
    (resolved.theme as Record<string, unknown> | undefined)?.opacity ?? {}
  for (const cand of candidates) {
    const segs = cand.split(':')
    const last = segs[segs.length - 1] ?? ''
    const slashIdx = lastTopLevelSlash(last)
    if (slashIdx < 0) continue
    const modifier = last.slice(slashIdx + 1)
    if (modifier.length === 0) continue
    if (modifier.startsWith('[') && modifier.endsWith(']')) {
      const inner = modifier.slice(1, -1)
      opacityValues.add(inner)
      modifierLabels.set(inner, modifier) // keep brackets in label
      continue
    }
    const fromTheme =
      typeof (opacityTable as Record<string, unknown>)[modifier] === 'string'
        ? ((opacityTable as Record<string, unknown>)[modifier] as string)
        : null
    if (fromTheme !== null) {
      opacityValues.add(fromTheme)
      modifierLabels.set(fromTheme, modifier)
    } else if (/^\d+$/.test(modifier)) {
      const n = Number(modifier) / 100
      opacityValues.add(String(n))
      modifierLabels.set(String(n), modifier)
    } else {
      opacityValues.add(modifier)
      modifierLabels.set(modifier, modifier)
    }
  }
  return { opacityValues, modifierLabels }
}

function lastTopLevelSlash(s: string): number {
  let depth = 0
  let last = -1
  for (let i = 0; i < s.length; i++) {
    const c = s[i]!
    if (c === '\\' && i + 1 < s.length) {
      i++
      continue
    }
    if (c === '[') depth++
    else if (c === ']') depth--
    else if (c === '/' && depth === 0) last = i
  }
  return last
}

function jsonSafe(value: unknown): unknown {
  if (value === null || value === undefined) return value
  if (typeof value === 'function') return null
  if (typeof value !== 'object') return value
  if (Array.isArray(value)) return value.map(jsonSafe)
  const out: Record<string, unknown> = {}
  for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
    out[k] = jsonSafe(v)
  }
  return out
}

/**
 * For each pattern-form safelist entry in `raw.safelist`, run a stub
 * Tailwind compile to enumerate matching candidates. Returns a flat
 * string list ready to be merged into `safelist: ['p-4', ...]` form
 * that the Rust compiler already consumes.
 *
 * Async because Tailwind's PostCSS pipeline is async. The work is
 * paid once per `loadConfig`, not per build cycle. Patterns
 * without a `variants:` field expand to bare candidate names; with
 * `variants: ['hover','md']` each match is duplicated for each
 * listed variant.
 */
async function resolvePatternSafelist(
  raw: Record<string, unknown>,
  configPath: string | null,
): Promise<string[]> {
  const safelist = raw.safelist
  if (!Array.isArray(safelist)) return []
  const patterns = safelist.filter(
    (v): v is { pattern: RegExp; variants?: string[] } =>
      typeof v === 'object' && v !== null && 'pattern' in (v as Record<string, unknown>),
  )
  if (patterns.length === 0) return []

  // Run Tailwind once with just the patterns + the user's theme. Empty
  // content means Tailwind only emits safelist matches.
  const stubConfig: Record<string, unknown> = {
    ...raw,
    safelist: patterns,
    content: [{ raw: '', extension: 'html' }],
    // Drop user plugins — we don't need their output for enumeration,
    // and running them may have side effects we don't want here.
    plugins: [],
  }
  const tw = resolveTailwind(configPath ? dirname(configPath) : null)
  const tailwindcss = tw.tailwindcss as (cfg: unknown) => postcss.AcceptedPlugin
  const result = await postcss([tailwindcss(stubConfig)]).process(
    '@tailwind utilities;',
    { from: undefined },
  )

  // Extract class names from emitted rules. Tailwind escapes special
  // chars (`:`, `/`, `[`, `.`, `,`) with `\`; reverse the escaping to
  // get the raw candidate name. The selector's class portion ends at
  // the first UNESCAPED `:` (start of pseudo-class) or whitespace
  // (descendant combinator).
  const classes = new Set<string>()
  result.root.walkRules((rule) => {
    for (const sel of rule.selector.split(',')) {
      const cls = extractClassName(sel.trim())
      if (cls !== null) classes.add(cls)
    }
  })
  return [...classes]
}

function extractClassName(selector: string): string | null {
  if (!selector.startsWith('.')) return null
  // Walk past the leading `.`; consume until an UNESCAPED selector
  // delimiter (`:` for pseudo-class, ` ` for descendant, `>` for
  // child, `[` for attribute, `~` / `+` for sibling). Bracketed
  // arbitrary-value runs (`[1fr_2fr]`) need to absorb their inner
  // content even though brackets normally end the run; track depth.
  let i = 1
  let depth = 0
  let out = ''
  while (i < selector.length) {
    const c = selector[i]!
    if (c === '\\' && i + 1 < selector.length) {
      out += c + selector[i + 1]!
      i += 2
      continue
    }
    if (c === '[') {
      depth++
      out += c
      i++
      continue
    }
    if (c === ']') {
      depth--
      out += c
      i++
      continue
    }
    if (depth === 0 && (c === ':' || c === ' ' || c === '>' || c === '~' || c === '+')) {
      break
    }
    out += c
    i++
  }
  if (out.length === 0) return null
  return unescapeClass(out)
}

function unescapeClass(s: string): string {
  // Reverse cssesc + escapeCommas. `\2c ` -> `,`; `\X` -> `X`.
  return s
    .replace(/\\2c\s/g, ',')
    .replace(/\\([\s\S])/g, '$1')
}
