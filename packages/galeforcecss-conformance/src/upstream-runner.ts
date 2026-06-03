// Run extracted upstream test cases against both the oracle (live
// `tailwindcss@3.4.19` plugin) and Galeforce, comparing CSS output.
//
// The extractor (`upstream-extract.ts`) gives us `(name, configSource,
// inputCss, expectedCss)` tuples. To run them we need to:
//
//   1. Eval `configSource` in a small sandbox that supplies the
//      `html`/`css`/`javascript` template tags + a `path` shim and
//      `__dirname`. Function-form values (theme extenders, plugins)
//      survive eval as native JS callables.
//   2. Drive the OFFICIAL plugin against the resolved config + input
//      CSS, capturing the result. This is the oracle baseline.
//   3. Drive Galeforce against the same config + input. We need to
//      synthesise a candidate set; we extract candidates from the
//      `content[].raw` HTML strings using the same regex Tailwind's
//      `defaultExtractor` ships with.
//   4. Format both outputs and string-equality compare. Any diff is a
//      Galeforce divergence — we surface it on the test result so
//      Vitest can render the failure clearly.
//
// Tests whose config can't be evaluated (e.g. references to upstream
// internals via relative imports) are flagged unsupported.

import postcss from 'postcss'
import tailwindcss from 'tailwindcss'
import { compile } from '@cx/galeforcecss'
import { processRawConfig, processRawConfigAsync } from '@cx/galeforcecss-config-loader'
import type { ExtractedTest } from './upstream-extract.js'

export interface RunResult {
  name: string
  /** When set, this case did not run; the reason is human-readable. */
  skipReason?: string
  /** Oracle output (raw CSS) — present iff oracleErr is unset. */
  oracleCss?: string
  oracleErr?: string
  /** Galeforce output — present iff galeforceErr is unset. */
  galeforceCss?: string
  galeforceErr?: string
  /** Diff message — empty when the two outputs match. */
  diff?: string
}

/**
 * Best-effort eval of a JS config object source. Returns the resolved
 * config or an Error. The sandbox provides `html`, `css`, `javascript`
 * (all `String.raw`), `path`, and `__dirname`. Functions inside the
 * config are real callables.
 */
export function evalConfigSource(source: string): unknown {
  const wrapped = `
    "use strict";
    const html = String.raw;
    const css = String.raw;
    const javascript = String.raw;
    const __dirname = "/__upstream__";
    const path = {
      resolve: (...parts) => parts.join('/'),
      join: (...parts) => parts.join('/'),
      dirname: (p) => p.split('/').slice(0, -1).join('/'),
      basename: (p) => p.split('/').pop(),
    };
    return (${source});
  `
  // eslint-disable-next-line @typescript-eslint/no-implied-eval
  const fn = new Function(wrapped)
  return fn()
}

/**
 * Default Tailwind class extractor (regex). Mirrors
 * `vendor/tailwindcss-v3/src/lib/defaultExtractor.js` closely
 * enough for the upstream test corpus where every class is inside
 * a `class="…"` attribute.
 */
// Upstream's `defaultExtractor` re-exported via @cx/galeforcecss-oracle so
// we can call it from this package (which doesn't have direct access
// to `tailwindcss/src/...`). Matches the regex set Tailwind ships
// for content scanning — including the edge cases where embedded
// quotes / brackets cause sub-tokens like `underline` to fall out
// of `aria-[labelledby='a_b']:underline`.
import { extractWithDefaultExtractor } from '@cx/galeforcecss-oracle'

/** True iff `()`, `[]`, `{}` all balance. */
function bracketsAndParensBalanced(s: string): boolean {
  const stack: string[] = []
  const pairs: Record<string, string> = { ')': '(', ']': '[', '}': '{' }
  for (let i = 0; i < s.length; i++) {
    const c = s[i]!
    if (c === '\\' && i + 1 < s.length) {
      i++
      continue
    }
    if (c === '"' || c === "'") {
      const q = c
      i++
      while (i < s.length && s[i] !== q) {
        if (s[i] === '\\' && i + 1 < s.length) i += 2
        else i++
      }
      continue
    }
    if (c === '(' || c === '[' || c === '{') stack.push(c)
    else if (c === ')' || c === ']' || c === '}') {
      if (stack.pop() !== pairs[c]) return false
    }
  }
  return stack.length === 0
}

function extractCandidates(html: string): string[] {
  const out = new Set<string>()
  // Strict class-attribute scan first — this catches the
  // structured-HTML inputs the harness ships (`<div class="foo bar"/>`)
  // without any false positives from upstream's permissive regex.
  const re = /class(?:Name)?\s*=\s*(?:"([^"]*)"|'([^']*)')/g
  let m: RegExpExecArray | null
  let foundAttr = false
  while ((m = re.exec(html)) !== null) {
    foundAttr = true
    const value = m[1] ?? m[2] ?? ''
    for (const tok of value.split(/\s+/)) {
      if (tok.length > 0) out.add(tok)
    }
  }
  // ALSO run upstream's extractor — it picks up sub-tokens that the
  // strict scan misses (`underline` from `aria-[…'…']:underline` when
  // the embedded quote splits the candidate). Filter the result down
  // to candidate-shaped tokens so we don't pollute with `div`/`class`/
  // etc.; the Rust compiler still rejects unknown candidates with a
  // diagnostic, so a few extras are harmless. Skip the upstream pass
  // for the no-attribute fallback (raw candidate strings).
  // ALWAYS run upstream's extractor too — it picks up sub-tokens
  // the strict scan misses, AND it's the only path that catches
  // non-attribute candidate forms like Svelte's `class:foo={...}`
  // (which we transformed to `  foo={...}` upstream of this fn).
  for (const tok of extractWithDefaultExtractor(html)) {
    const t = tok.trim()
    if (t.length === 0) continue
    if (out.has(t)) continue
    if (!/^[a-zA-Z_!\-\[]/.test(t)) continue
    if (t.includes('=') && !t.includes('[')) continue
    if (t.includes('"')) continue
    if (t.includes('<') || t.includes('>')) continue
    // Drop tokens with unbalanced brackets/parens. The upstream
    // regex over-matches inside complex arbitrary variants
    // (`[[data-foo='bar'][data-baz]_&]:underline` produces
    // sub-tokens like `[data-baz]_&]:underline` whose brackets
    // don't balance). Tailwind's own candidate parser rejects
    // them; mirror that here so we don't emit malformed rules.
    if (!bracketsAndParensBalanced(t)) continue
    out.add(t)
  }
  if (!foundAttr) {
    // Fallback for raw candidate strings (no `class=` attribute,
    // no extracted candidates). Split by whitespace and filter
    // to candidate-shaped tokens.
    for (const tok of html.split(/\s+/)) {
      const t = tok.trim()
      if (t.length === 0) continue
      if (/^[a-zA-Z_!\-\[]/.test(t)) {
        out.add(t)
      }
    }
  }
  return [...out]
}

function gatherCandidatesFromConfig(config: any): string[] {
  const seen = new Set<string>()
  // v3 honors `content`; v2 used `purge`. Tailwind's
  // `normalizeConfig` falls back to `purge` when `content` is
  // absent, including legacy `purge.content` / `purge.options`
  // / `purge.extract` / `purge.transform`.
  let content = config?.content
  if (!content) content = config?.purge
  if (!content) return []
  let items = Array.isArray(content) ? content : (content?.files ?? content?.content ?? [])
  if (!Array.isArray(items)) items = []
  // User extractors. `defaultExtractor` applies to every file;
  // `extract.<ext>` overrides per extension. `transform.<ext>`
  // pre-processes the raw content. Mirrors v2-config-shaped
  // `purge.options.defaultExtractor` / `purge.extract` /
  // `purge.transform`.
  const purgeOpts = config?.purge?.options ?? {}
  const defaultExtractor = (purgeOpts as any)?.defaultExtractor as
    | ((c: string) => string[])
    | undefined
  const extractMap =
    ((config?.purge?.extract ?? config?.extract) as Record<string, (c: string) => string[]>) ?? {}
  const transformMap =
    ((config?.purge?.transform ?? config?.transform) as Record<string, (c: string) => string>) ?? {}
  for (const item of items) {
    if (typeof item === 'string') continue
    if (!item || typeof item !== 'object' || typeof item.raw !== 'string') continue
    const ext = (item.extension as string | undefined) ?? 'html'
    let raw = item.raw
    // Built-in pre-extraction transforms — mirror upstream's
    // `expandTailwindAtRules.js` `builtInTransformers` map. For
    // Svelte specifically: `class:foo={…}` is rewritten to ` foo`
    // so the default extractor can pick `foo` as a candidate.
    if (ext === 'svelte') {
      raw = raw.replace(/(?:^|\s)class:/g, ' ')
    }
    if (typeof transformMap[ext] === 'function') {
      try {
        raw = transformMap[ext](raw)
      } catch {
        /* ignore */
      }
    }
    let toks: string[] | null = null
    if (typeof extractMap[ext] === 'function') {
      try {
        toks = extractMap[ext](raw)
      } catch {
        /* ignore */
      }
    } else if (typeof defaultExtractor === 'function') {
      try {
        toks = defaultExtractor(raw)
      } catch {
        /* ignore */
      }
    }
    if (toks !== null) {
      for (const t of toks) {
        if (typeof t !== 'string') continue
        const trimmed = t.trim()
        if (trimmed.length > 0) seen.add(trimmed)
      }
      continue
    }
    for (const c of extractCandidates(raw)) seen.add(c)
  }
  return [...seen]
}

/** Recursively check whether any value inside `obj` is a function. */
function hasFunctionThemeValue(obj: unknown): boolean {
  if (!obj || typeof obj !== 'object') return false
  for (const v of Object.values(obj as Record<string, unknown>)) {
    if (typeof v === 'function') return true
    if (v && typeof v === 'object' && !Array.isArray(v)) {
      if (hasFunctionThemeValue(v)) return true
    }
  }
  return false
}

async function runOracle(input: string, config: any): Promise<string> {
  const result = await postcss([tailwindcss(config)]).process(input, {
    from: undefined,
  })
  return result.css
}

async function runGaleforce(input: string, config: any): Promise<string> {
  const candidates = gatherCandidatesFromConfig(config)
  // Run the config through `@cx/galeforcecss-config-loader` whenever the
  // user supplied anything that needs Tailwind's own
  // `resolveConfig` to merge: plugins, presets, or theme.extend.
  // For pure overrides (`theme: { colors: {...} }`) we keep the
  // raw config — config-loader's resolveConfig sometimes
  // transforms theme keys in ways the corpus doesn't expect for
  // those simple cases, so we narrow to the merge cases.
  let resolvedConfig: Record<string, unknown> = config as Record<string, unknown>
  const cfgAny = config as any
  const needsResolve =
    (Array.isArray(cfgAny?.plugins) && cfgAny.plugins.length > 0) ||
    (Array.isArray(cfgAny?.presets) && cfgAny.presets.length > 0) ||
    (cfgAny?.theme?.extend && Object.keys(cfgAny.theme.extend).length > 0) ||
    hasFunctionThemeValue(cfgAny?.theme)
  // Async processing is needed when the config restricts
  // `corePlugins` (changes cascade-var defaults), overrides
  // preflight theme paths, or overrides the ring/shadow themes
  // that the base block reads — all require running Tailwind
  // itself to get the right base block content.
  const themeRoot = cfgAny?.theme ?? {}
  const themeExt = themeRoot?.extend ?? {}
  const baseRelevantKeys = [
    'borderColor',
    'fontFamily',
    'ringColor',
    'ringOpacity',
    'ringWidth',
    'ringOffsetWidth',
    'ringOffsetColor',
    'boxShadow',
    'boxShadowColor',
  ]
  const future = cfgAny?.future ?? {}
  const experimental = cfgAny?.experimental ?? {}
  // Pattern-form safelist (`{ pattern: /…/ }`) needs the async path
  // because resolution requires running Tailwind itself to enumerate
  // matches; sync mode skips them entirely.
  const hasPatternSafelist =
    Array.isArray(cfgAny?.safelist) &&
    cfgAny.safelist.some(
      (v: unknown) =>
        typeof v === 'object' && v !== null && 'pattern' in (v as Record<string, unknown>),
    )
  const needsAsync =
    (cfgAny?.corePlugins && typeof cfgAny.corePlugins === 'object') ||
    Object.keys(future).length > 0 ||
    Object.keys(experimental).length > 0 ||
    hasPatternSafelist ||
    baseRelevantKeys.some(
      (k) => themeRoot?.[k] !== undefined || themeExt?.[k] !== undefined,
    )
  if (needsAsync) {
    try {
      const loaded = await processRawConfigAsync(
        config as Record<string, unknown>,
        null,
        candidates,
      )
      resolvedConfig = loaded.resolved as Record<string, unknown>
    } catch {
      // Best-effort; fall through to sync path on failure.
    }
  } else if (needsResolve) {
    try {
      const loaded = processRawConfig(config as Record<string, unknown>, null, candidates)
      resolvedConfig = loaded.resolved as Record<string, unknown>
    } catch {
      // Best-effort; fall back to the raw config if the loader
      // can't process it.
    }
  }
  const r = await compile({
    candidates,
    inputCss: input,
    config: resolvedConfig,
  })
  return r.css
}

export interface RunOptions {
  /**
   * Use order-sensitive diff (rule AND declaration order matter).
   * Off by default for backwards compat — the main upstream-suite
   * test stays unordered for stability, while the byte-sensitive
   * snapshot test opts in.
   */
  orderSensitive?: boolean
}

export async function runExtractedTest(
  test: ExtractedTest,
  opts: RunOptions = {},
): Promise<RunResult> {
  if (test.skipReason) {
    return { name: test.name, skipReason: test.skipReason }
  }
  let resolvedConfig: any
  try {
    resolvedConfig = evalConfigSource(test.configSource)
  } catch (err) {
    return {
      name: test.name,
      skipReason: `config eval failed: ${(err as Error).message}`,
    }
  }
  // Tailwind accepts `{ config: <config-object> }` as a wrapper —
  // unwrap so our pipeline sees the actual config. Mirrors
  // `tailwindcss(opts)` honoring the `.config` property.
  if (
    resolvedConfig &&
    typeof resolvedConfig === 'object' &&
    !Array.isArray(resolvedConfig) &&
    typeof resolvedConfig.config === 'object' &&
    resolvedConfig.config !== null &&
    !Array.isArray(resolvedConfig.config)
  ) {
    resolvedConfig = resolvedConfig.config
  }
  // Many upstream configs reference helpers that don't survive our
  // sandbox (custom plugins importing internals, `path.resolve`'d
  // file paths for content, etc.). Bail with a clear reason.
  const contentItems = Array.isArray(resolvedConfig?.content)
    ? resolvedConfig.content
    : (resolvedConfig?.content?.files ?? [])
  if (
    Array.isArray(contentItems) &&
    contentItems.length > 0 &&
    contentItems.every((c: unknown) => typeof c === 'string')
  ) {
    return {
      name: test.name,
      skipReason: 'config.content references file paths (unsupported here)',
    }
  }
  let oracleCss: string | undefined
  let oracleErr: string | undefined
  try {
    oracleCss = await runOracle(test.inputCss, resolvedConfig)
  } catch (err) {
    oracleErr = (err as Error).message
  }
  let galeforceCss: string | undefined
  let galeforceErr: string | undefined
  try {
    galeforceCss = await runGaleforce(test.inputCss, resolvedConfig)
  } catch (err) {
    galeforceErr = (err as Error).message
  }
  if (oracleErr || galeforceErr || !oracleCss || !galeforceCss) {
    return {
      name: test.name,
      oracleCss,
      oracleErr,
      galeforceCss,
      galeforceErr,
    }
  }
  const diff = computeDiff(galeforceCss, oracleCss, opts.orderSensitive ?? false)
  return {
    name: test.name,
    oracleCss,
    galeforceCss,
    diff,
  }
}

import { normalizeCss } from './normalize.js'
import { diffStylesheets, formatDiff } from './diff.js'

/**
 * Compare actual (Galeforce) and expected (oracle) CSS using the same
 * unordered, semantic-equivalence diff the rest of the conformance
 * harness uses. Returns a human-readable diff string when they
 * disagree, or empty when they match.
 */
function computeDiff(actual: string, expected: string, orderSensitive = false): string {
  let a, e
  try {
    a = normalizeCss(actual)
    e = normalizeCss(expected)
  } catch (err) {
    return `normalizeCss error: ${(err as Error).message}\n\n--- Galeforce ---\n${actual}\n\n--- Oracle ---\n${expected}`
  }
  const entries = diffStylesheets(a, e, { orderSensitive })
  if (entries.length === 0) return ''
  return formatDiff(entries)
}
