// Vite plugin for GaleforceCSS — drop-in replacement for the Tailwind v3
// PostCSS plugin.
//
// **How it integrates with Vite.** We register a PostCSS plugin via
// `config.css.postcss.plugins`. Vite's built-in CSS pipeline
// (`vite:css`) runs PostCSS internally on every `.css` / `.scss` /
// `.sass` / `.less` / `.styl` file the bundler touches — after the
// preprocessor has produced CSS, before the result is wrapped into a
// JS module. This is exactly the hook Tailwind v3 uses, so an existing
// Tailwind setup ports over by swapping plugins.
//
// We do NOT use a Vite `transform()` hook for this. At `enforce: 'pre'`
// we'd see SCSS source (no `@tailwind` would resolve cleanly); at
// `enforce: 'post'` Vite has already converted the CSS into a JS
// module. PostCSS is the right layer.
//
// **What the Vite plugin still owns:**
//   - Loading the Tailwind config (via `galeforcecss-config-loader`).
//   - Scanning content roots for candidate classes.
//   - Watch / HMR: invalidating CSS modules when a content file changes
//     adds or removes a candidate.
//   - The optional `virtual:galeforcecss.css` module — kept for projects
//     that want a single plugin-owned bundle instead of inline CSS
//     expansion.

import { readFile } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { isAbsolute, resolve, sep } from 'node:path'
import { glob as tinyGlob } from 'tinyglobby'
import postcss, { type Plugin as PostcssPlugin, type Root as PostcssRoot } from 'postcss'
import {
  compile,
  createCompileStream,
  loadConfig,
  type CompileStream,
  type LoadedConfig,
} from '@cx/galeforcecss'
import type { Plugin, ResolvedConfig, UserConfig, ViteDevServer } from 'vite'

export interface GaleforceCssPluginOptions {
  /**
   * Path to a CSS entry used by the virtual-module mode. When set,
   * the plugin exposes `virtual:galeforcecss.css` whose contents are the
   * compiled output for this entry plus the candidate set. Most
   * projects don't need this — the PostCSS-driven pipeline already
   * handles `@tailwind`/`@apply` directives inline in any CSS/SCSS file.
   */
  input?: string
  /** Path to a tailwind config file. If omitted, walks up from `cwd` looking for the defaults. */
  config?: string
  /**
   * Content roots to scan for candidates. Defaults to the `content`
   * array in the loaded Tailwind config (matching the Tailwind v3
   * PostCSS plugin's behavior). If neither is set, falls back to the
   * Vite project root.
   */
  content?: string[]
  /**
   * Public name of the virtual module. Defaults to `'virtual:galeforcecss.css'`.
   * Only relevant when `input` is set.
   */
  virtualId?: string
  /**
   * Minify the output CSS via Lightning CSS. Defaults to Vite's
   * `build.cssMinify` for production builds, off for dev.
   *
   * Note: when minify is on, Lightning CSS runs once — on the final
   * compiled CSS — not per @apply-resolved file. Vite's own CSS
   * minifier may also run downstream; if you want only one minify
   * pass, set Vite's `build.cssMinify: false`.
   */
  minify?: boolean
  /**
   * Emit Source Map v3 alongside the CSS. Defaults to Vite's
   * `build.sourcemap` setting.
   */
  sourceMap?: boolean
  /**
   * Per-browser minimum versions (e.g. `{ chrome: '95', firefox: '94' }`).
   * Forwarded to Lightning CSS for prefixing + feature lowering.
   */
  targets?: Record<string, string>
  /**
   * Version-compatibility flags for matching legacy Tailwind output
   * byte-for-byte. Defaults match the pinned target Tailwind v3.4.19.
   */
  compat?: {
    /**
     * Which Tailwind v3 patch line to match byte-for-byte. Defaults
     * to `'3.4'`. Set to `'3.3'` when your project is still on
     * `tailwindcss@3.3.x` and you want a byte-identical swap — the
     * v3.3 mode bundles every cosmetic byte difference between the
     * patch lines (opacity-var fallback, selector comma escape,
     * math-function comma spacing). All differences are cosmetic;
     * runtime CSS behavior is identical either way.
     */
    tailwindVersion?: '3.3' | '3.4'
  }
  /**
   * Enable nested-CSS expansion — a Rust port of `tailwindcss/nesting`
   * + `postcss-nested@6.2.0`. Lets you author `.foo { &:hover { ... } }`
   * and `.btn { &-primary { ... } }` directly in your `.css` files;
   * the compiler flattens them before `@apply` / `@tailwind` / `@screen`
   * directives run. Default false — projects that don't author nested
   * CSS pay zero cost.
   */
  nesting?: boolean
  /**
   * Extra PostCSS plugins to run alongside GaleforceCSS.
   *
   * By design, this Vite plugin sets `css.postcss` explicitly to ONLY
   * include GaleforceCSS — bypassing Vite's auto-discovery of any
   * `postcss.config.{js,cjs,mjs}` in the project root. That makes the
   * Vite path independent of a legacy PostCSS setup (e.g. an Angular
   * CLI build that needs its own postcss.config.js with tailwindcss in
   * it).
   *
   * If you want autoprefixer / postcss-nesting / etc. on the Vite
   * path, pass them here. They run AFTER GaleforceCSS, so they see the
   * compiled utility classes.
   */
  postcssPlugins?: PostcssPlugin[]
}

interface ResolvedOptions {
  input: string | undefined
  configPath: string | undefined
  /** `null` means "fall back to config.content at scan time". */
  contentRoots: string[] | null
  virtualId: string
  resolvedVirtualId: string
  minify: boolean
  sourceMap: boolean
  targets: Record<string, string> | undefined
  compat: { tailwindVersion: '3.3' | '3.4' }
  nesting: boolean
  postcssPlugins: PostcssPlugin[]
}

/**
 * Regex that detects any Tailwind v3 directive we expand. Cheap to run
 * on every CSS file we see — if it doesn't match, we return the file
 * unchanged. Matches:
 *   - `@tailwind <base|components|utilities>;`
 *   - `@apply <classes>`
 *   - `@layer <name> { ... }`
 *   - `@screen <name> { ... }`
 *   - `theme(<path>)` calls in declaration values
 *   - `screen(<name>)` calls in declaration values
 */
const TAILWIND_DIRECTIVE_RE = /@(?:tailwind|apply|layer|screen)\b|\btheme\s*\(|\bscreen\s*\(/

/**
 * Detects native CSS nesting via the `&` parent selector. Cheap to
 * grep — if not present, we can early-return like we always have.
 *
 * The pattern matches `&` only when it's part of a selector, not when
 * it appears inside a string or a property value (e.g. content rules
 * like `content: "a & b"`). The simplest reliable signal is `&` either:
 *   - Followed by a CSS combinator/space/colon (`&:hover`, `& .foo`,
 *     `&>p`, `&+p`, `& *`)
 *   - Or `&` followed by `{` (`& { ... }` — rare but valid)
 *
 * False positives are tolerable here because the only consequence is
 * routing the file through the Rust compiler when we didn't strictly
 * need to. False negatives would let the bug back in.
 */
const NATIVE_NESTING_RE = /(^|[\s{};])&(?:[\s.#:[+~*>,{]|$)/m

/**
 * Detects whether a CSS file emits the project's full atomic utility
 * stylesheet via a `@tailwind base|components|utilities;` directive.
 * That's the ONLY situation where the per-file compile needs the
 * project-wide scanned candidate set — without one of these directives,
 * the only candidate-dependent work is resolving `@apply` tokens, which
 * is far cheaper to feed with just the tokens the file references.
 */
const TAILWIND_ENTRY_RE = /@tailwind\s+(?:base|components|utilities)\b/

/**
 * Pull every class name referenced by an `@apply` directive in the
 * input CSS. Mirrors Tailwind v3's apply-token regex:
 *   `@apply foo bar !baz` → ['foo', 'bar', '!baz']
 * Whitespace-separated, no comma support. Variant prefixes (`hover:foo`,
 * `[&:hover]:foo`), arbitrary values, and the `!` important suffix all
 * pass through as a single token.
 */
function extractApplyTokens(css: string): string[] {
  const out: string[] = []
  // Each `@apply` declaration is terminated by `;` or end-of-rule.
  const applyRe = /@apply\s+([^;]+?)(?=;|$)/g
  let m: RegExpExecArray | null
  while ((m = applyRe.exec(css)) !== null) {
    const body = m[1]!.trim()
    // Class tokens are whitespace-separated. Brackets in arbitrary
    // values (`hover:bg-[#abc]`) never contain whitespace per Tailwind's
    // tokenizer, so a plain split is safe.
    for (const tok of body.split(/\s+/)) {
      if (tok.length > 0) out.push(tok)
    }
  }
  return out
}

function toAbs(p: string, root: string): string {
  return isAbsolute(p) ? p : resolve(root, p)
}

function resolveOptions(raw: GaleforceCssPluginOptions, vite: ResolvedConfig): ResolvedOptions {
  const root = vite.root
  const virtualId = raw.virtualId ?? 'virtual:galeforcecss.css'
  const isProduction = vite.command === 'build'
  const defaultMinify = isProduction && !!vite.build.cssMinify
  return {
    input: raw.input ? toAbs(raw.input, root) : undefined,
    configPath: raw.config ? toAbs(raw.config, root) : undefined,
    contentRoots:
      raw.content && raw.content.length > 0 ? raw.content.map((c) => toAbs(c, root)) : null,
    virtualId,
    resolvedVirtualId: '\0' + virtualId,
    minify: raw.minify ?? defaultMinify,
    sourceMap: raw.sourceMap ?? !!vite.build.sourcemap,
    targets: raw.targets,
    compat: { tailwindVersion: raw.compat?.tailwindVersion ?? '3.4' },
    nesting: raw.nesting ?? false,
    postcssPlugins: raw.postcssPlugins ?? [],
  }
}

/**
 * Detect glob metacharacters Tailwind users commonly write:
 *   * ? [ ] { } !( | (extglob alternation)
 *
 * The Rust scanner's `WalkBuilder` treats roots as literal filesystem
 * paths and doesn't expand globs, so we must pre-expand JS-side
 * before handing them across.
 */
function looksLikeGlob(p: string): boolean {
  for (let i = 0; i < p.length; i++) {
    const c = p[i]
    if (c === '*' || c === '?' || c === '[' || c === ']' || c === '{' || c === '}') return true
    if (c === '!' && p[i + 1] === '(') return true
  }
  return false
}

/**
 * Expand a list of content roots into concrete file paths the Rust
 * scanner can walk directly. Plain directory / file paths pass
 * through unchanged; entries with glob metacharacters are expanded
 * via `tinyglobby` (which supports extglob alternation like
 * `!(*.stories|*.spec).{ts,html}` that the Rust globset can't).
 *
 * Anything that doesn't exist on disk is dropped silently — matches
 * Tailwind's behavior when a content glob has no matches.
 */
async function expandContentRoots(roots: readonly string[]): Promise<string[]> {
  const out: string[] = []
  const globPatterns: string[] = []
  for (const r of roots) {
    if (looksLikeGlob(r)) {
      globPatterns.push(r)
    } else if (existsSync(r)) {
      out.push(r)
    }
  }
  if (globPatterns.length > 0) {
    try {
      const expanded = await tinyGlob(globPatterns, {
        absolute: true,
        onlyFiles: true,
        // Tailwind's config globs are case-sensitive on Linux but
        // many Windows + macOS contributors author paths in any
        // case. Match Tailwind's own behavior (case-insensitive).
        caseSensitiveMatch: false,
        // NB: `followSymbolicLinks` left at tinyglobby's default
        // (true). On macOS the system tmp dir (`/var/folders/...`)
        // is itself a symlink to `/private/var/folders/...`; with
        // `followSymbolicLinks: false` we'd return empty results
        // there. Real projects might want to opt out via the
        // glob-driver later if `node_modules` symlinks become an
        // issue, but until that bites we follow upstream behavior.
      })
      for (const f of expanded) out.push(f)
    } catch {
      // Best-effort. A malformed pattern shouldn't break the build.
    }
  }
  return out
}

/**
 * Pull glob-shaped content entries from the resolved Tailwind config.
 * Supports `string[]`, `{ files: string[] }`, and the synthetic
 * `{ raw, extension }` form (which is ignored for scanner roots).
 */
function readConfigContent(config: LoadedConfig | null, root: string): string[] {
  if (!config) return [root]
  const c = config.resolved.content as unknown
  const globs: string[] = []
  const pushGlob = (g: unknown): void => {
    if (typeof g === 'string') globs.push(toAbs(g, root))
  }
  if (Array.isArray(c)) {
    for (const entry of c) pushGlob(entry)
  } else if (c && typeof c === 'object') {
    const files = (c as { files?: unknown }).files
    if (Array.isArray(files)) for (const f of files) pushGlob(f)
  }
  return globs.length > 0 ? globs : [root]
}

/** Plugin-local state, captured in a closure so the PostCSS adapter can
 * read the latest candidates + config without going back through Vite. */
interface PluginState {
  opts: ResolvedOptions | null
  stream: CompileStream | null
  cachedConfig: LoadedConfig | null
  globalTokens: Set<string>
  /** Resolves once config + initial token union are ready. PostCSS waits on this
   *  before processing the first CSS file. */
  ready: Promise<void> | null
  /** Resolves once the per-file token map is populated. Only HMR needs this —
   *  cold start can serve CSS without it. Runs in the background. */
  fileTokensReady: Promise<void> | null
  warnedPlugins: boolean
}

/**
 * The PostCSS plugin form of GaleforceCSS. Receives a Root parsed by
 * PostCSS, stringifies it back to CSS (we don't operate on AST nodes),
 * runs the GaleforceCSS compiler, and replaces the Root with the parsed
 * output. Tailwind v3 takes the same round-trip approach internally.
 *
 * The plugin function is closure-bound to the Vite plugin's state so
 * each compile sees the latest globalTokens / config.
 */
function postcssGaleforcePlugin(state: PluginState): PostcssPlugin {
  return {
    postcssPlugin: 'galeforcecss',
    async OnceExit(root) {
      const BENCH = process.env['CX_BENCH_GALEFORCE_INTERNAL'] === '1'
      const tStart = BENCH ? performance.now() : 0
      if (!state.opts) return
      // Cheap directive sniff BEFORE awaiting state.ready. Most CSS
      // files in a real project (SCSS partials, theme files, component
      // styles) never reach for Tailwind directives — making them wait
      // for the cold-start content scan to finish is pure overhead.
      // Only the file(s) that actually use `@tailwind` / `@apply` /
      // `@layer` / `theme()` / `screen()` need the candidate set.
      const inputCss = root.toString()
      const hasTailwindDirective = TAILWIND_DIRECTIVE_RE.test(inputCss)
      // When the user opted in to `nesting`, files with `&` selectors
      // still need a flatten pass even if they have no Tailwind
      // directive — otherwise CSS frameworks downstream that operate on
      // flat selectors (e.g. Angular's emulated view encapsulation,
      // certain SCSS-style style rewriters) see the raw `&` form and
      // either silently drop the rule or mis-scope it.
      //
      // This matches what `tailwindcss/nesting` does in the legacy
      // PostCSS pipeline: it runs on every CSS file, not just the ones
      // that consume Tailwind output. We restrict to the opt-in case
      // (the `nesting: true` option) so default users still pay zero
      // cost on plain component CSS.
      const needsNestingFlatten = state.opts.nesting && NATIVE_NESTING_RE.test(inputCss)
      if (!hasTailwindDirective && !needsNestingFlatten) {
        if (BENCH) {
          // eslint-disable-next-line no-console
          console.log(
            `[galeforce:css] skip (no directives) ${(performance.now() - tStart).toFixed(0)}ms`,
          )
        }
        return
      }

      // Files with `@tailwind base|components|utilities` are the
      // entry stylesheet — they emit the project's full atomic
      // utilities and need the scanned candidate set. Files with only
      // `@apply` / `theme()` / `@layer` need just the @apply tokens
      // (the compiler resolves each on its own); passing 276k+
      // candidates per file balloons IPC + parse cost for nothing.
      const needsScanCandidates = TAILWIND_ENTRY_RE.test(inputCss)
      let candidates: string[]
      if (needsScanCandidates) {
        // Entry CSS: full scanned set. Wait for cold-start scan.
        if (state.ready) await state.ready
        candidates = [...state.globalTokens]
      } else {
        // Non-entry: only the tokens this file references via @apply.
        // No need to wait for the project-wide scan — these tokens
        // come from inside this file.
        const apply = extractApplyTokens(inputCss)
        candidates = apply.length > 0 ? apply : []
      }
      const tReady = BENCH ? performance.now() : 0

      const config = state.cachedConfig
      if (!config) return

      const sourceId = (root.source?.input as { from?: string } | undefined)?.from ?? undefined
      const result = state.stream
        ? await state.stream.compile({
            candidates,
            inputCss,
            inputCssPath: sourceId,
            config: config.resolved,
            // Don't pass `content` here — the candidate set was already
            // resolved in buildStart + handleHotUpdate. Re-scanning per
            // PostCSS invocation would be wasteful and racy.
            features: {
              // Skip Lightning CSS in-flight; the user's downstream
              // Vite CSS pipeline (`build.cssMinify`) handles minification
              // for production builds. Running Lightning twice would
              // double the work.
              minify: false,
              sourceMaps: false,
              targets: undefined,
              compat: state.opts.compat,
              nesting: state.opts.nesting,
            },
          })
        : await compile({
            candidates,
            inputCss,
            inputCssPath: sourceId,
            config: config.resolved,
            features: {
              minify: false,
              sourceMaps: false,
              targets: undefined,
              compat: state.opts.compat,
              nesting: state.opts.nesting,
            },
          })

      surfaceDiagnostics(result.diagnostics, config, state)
      const tCompile = BENCH ? performance.now() : 0
      replaceRoot(root, result.css)
      if (BENCH) {
        const tFinal = performance.now()
        const id = sourceId ?? '?'
        const short = id.length > 60 ? '...' + id.slice(-57) : id
        // eslint-disable-next-line no-console
        console.log(
          `[galeforce:css] ${short} ready-wait ${(tReady - tStart).toFixed(0)}ms compile ${(tCompile - tReady).toFixed(0)}ms replace ${(tFinal - tCompile).toFixed(0)}ms total ${(tFinal - tStart).toFixed(0)}ms (in ${inputCss.length}B out ${result.css.length}B)`,
        )
      }
    },
  }
}
postcssGaleforcePlugin.postcss = true

/**
 * Replace `root`'s children with the parsed contents of `css`. Re-uses
 * `root.source` so PostCSS source-map tracking still attributes nodes
 * to the original file.
 */
function replaceRoot(root: PostcssRoot, css: string): void {
  const newRoot = postcss.parse(css, { from: root.source?.input.file ?? undefined })
  root.removeAll()
  // postcss.Container.append handles arrays of nodes; cast through
  // unknown because the types expect Container nodes and `newRoot.nodes`
  // is `ChildNode[]` which is structurally compatible.
  root.append(...(newRoot.nodes as unknown as Parameters<typeof root.append>))
}

export default function galeforcecss(rawOptions: GaleforceCssPluginOptions = {}): Plugin {
  const state: PluginState = {
    opts: null,
    stream: null,
    cachedConfig: null,
    globalTokens: new Set(),
    ready: null,
    fileTokensReady: null,
    warnedPlugins: false,
  }
  let isDev = false
  let viteRoot = ''
  // Per-file candidate sets — used by handleHotUpdate to compute deltas.
  const fileTokens: Map<string, Set<string>> = new Map()
  // Effective scan roots — resolved from plugin options OR config.content.
  let effectiveContentRoots: string[] = []
  // CSS module ids the plugin has affected. Invalidated when the
  // global candidate set changes so PostCSS re-runs with new tokens.
  const transformedIds: Set<string> = new Set()
  let lastVirtualCss = ''

  async function getConfig(): Promise<LoadedConfig> {
    if (state.cachedConfig) return state.cachedConfig
    state.cachedConfig = await loadConfig({
      path: state.opts?.configPath,
      cwd: viteRoot,
    })
    return state.cachedConfig
  }

  async function readInputCss(): Promise<string | undefined> {
    if (!state.opts?.input) return undefined
    if (!existsSync(state.opts.input)) return undefined
    return await readFile(state.opts.input, 'utf8')
  }

  function buildFeatures(): {
    minify: boolean
    sourceMaps: boolean
    targets: Record<string, string> | undefined
    compat: { tailwindVersion: '3.3' | '3.4' }
  } {
    return {
      minify: state.opts!.minify,
      sourceMaps: state.opts!.sourceMap,
      targets: state.opts!.targets,
      compat: state.opts!.compat,
    }
  }

  async function compileVirtual(): Promise<string> {
    if (!state.opts) throw new Error('galeforcecss plugin: configResolved did not run')
    const inputCss = await readInputCss()
    const config = await getConfig()
    const result = state.stream
      ? await state.stream.compile({
          candidates: [...state.globalTokens],
          inputCss,
          inputCssPath: state.opts.input,
          config: config.resolved,
          content: effectiveContentRoots,
          features: buildFeatures(),
        })
      : await compile({
          candidates: [...state.globalTokens],
          inputCss,
          inputCssPath: state.opts.input,
          config: config.resolved,
          content: effectiveContentRoots,
          features: buildFeatures(),
        })
    surfaceDiagnostics(result.diagnostics, config, state)
    lastVirtualCss = result.css
    return result.css
  }

  function watchedFiles(): string[] {
    if (!state.opts) return []
    const files: string[] = []
    if (state.opts.input) files.push(state.opts.input)
    if (state.opts.configPath) files.push(state.opts.configPath)
    return files
  }

  function invalidateTransformed(server: ViteDevServer): void {
    for (const id of transformedIds) {
      const mod = server.moduleGraph.getModuleById(id)
      if (mod) server.moduleGraph.invalidateModule(mod)
    }
  }

  return {
    name: 'vite-plugin-galeforcecss',

    // Inject GaleforceCSS into Vite's CSS pipeline as the SOLE PostCSS
    // plugin. We deliberately bypass Vite's `postcss-load-config`
    // auto-discovery: returning a `css.postcss` object here makes Vite
    // treat it as authoritative and skip the disk-config lookup.
    //
    // Why bypass? Many projects (especially monorepos with a parallel
    // build path, e.g. Angular CLI + Vite OXC) keep a legacy
    // `postcss.config.js` that's still needed by the non-Vite path.
    // Auto-discovering it from Vite would run `tailwindcss` AGAIN on
    // top of our galeforce pipeline — duplicate work, possible duplicate
    // class output. Users who want autoprefixer / nesting / etc. on
    // the Vite path pass them via `postcssPlugins`.
    config(): UserConfig {
      return {
        css: {
          postcss: {
            plugins: [
              postcssGaleforcePlugin(state),
              ...(rawOptions.postcssPlugins ?? []),
            ],
          },
        },
      }
    },

    configResolved(config) {
      state.opts = resolveOptions(rawOptions, config)
      isDev = config.command === 'serve'
      viteRoot = config.root
    },

    buildStart() {
      if (!state.opts) return
      // Pre-resolve config + scan content so PostCSS callers (which
      // start the moment Vite touches a CSS file) see a populated
      // candidate set. The `ready` promise gates PostCSS until this
      // finishes — but we intentionally DO NOT `await` it here.
      // Returning from buildStart immediately lets Vite proceed with
      // optimizeDeps and `server.listen()` in parallel with our scan.
      // PostCSS gates on `state.ready` before its first invocation, so
      // first-CSS correctness is preserved.
      state.ready = (async () => {
        const BENCH = process.env['CX_BENCH_GALEFORCE_INTERNAL'] === '1'
        const mark = (label: string, t0: number): void => {
          if (BENCH) {
            // eslint-disable-next-line no-console
            console.log(`[galeforce:init] ${label} ${(performance.now() - t0).toFixed(1)}ms`)
          }
        }
        const tConfig = performance.now()
        const config = await getConfig()
        mark('loadConfig', tConfig)
        effectiveContentRoots =
          state.opts!.contentRoots !== null
            ? state.opts!.contentRoots!
            : readConfigContent(config, viteRoot)

        if (isDev) {
          const tStream = performance.now()
          state.stream = createCompileStream()
          mark('createCompileStream', tStream)
        }

        if (effectiveContentRoots.length > 0) {
          // Tailwind v3 projects routinely use extglob patterns like
          // `**/!(*.stories|*.spec).{ts,html}` in their `content`
          // config. The Rust scanner can't expand those — its
          // `WalkBuilder` takes literal filesystem paths only. So
          // expand globs JS-side before crossing the napi/CLI
          // boundary.
          const tGlob = performance.now()
          const scanRoots = await expandContentRoots(effectiveContentRoots)
          mark(`expandContentRoots (${scanRoots.length} files)`, tGlob)
          if (scanRoots.length === 0 && effectiveContentRoots.length > 0) {
            console.warn(
              `[vite-plugin-galeforcecss] No files matched the ${effectiveContentRoots.length} content ` +
                `pattern(s) configured in tailwind.config — utility classes will not be generated. ` +
                `Check your \`content\` globs (e.g. ${JSON.stringify(effectiveContentRoots[0])}).`,
            )
          }

          // FAST PATH: get just the union of candidates for the first
          // CSS compile. Returns a sorted flat array — smaller JSON,
          // cheaper to parse, no path-keyed object overhead. This is
          // all PostCSS needs to populate `state.globalTokens` and
          // start serving CSS. Refcount-based HMR fileTokens are
          // populated in the background via scanPerFile below.
          const stream = state.stream
          if (stream) {
            try {
              const tScan = performance.now()
              const union = (await stream.scan(scanRoots)) as string[]
              mark(`scan union (${union.length} tokens)`, tScan)
              for (const t of union) state.globalTokens.add(t)
            } catch {
              // best-effort
            }
            // BACKGROUND: populate per-file map for accurate HMR deltas.
            // No await — completes silently before any HMR can fire.
            state.fileTokensReady = (async () => {
              try {
                const tPer = performance.now()
                const perFile = (await stream.scanPerFile(scanRoots)) as Record<string, string[]>
                mark(`scanPerFile (bg)`, tPer)
                for (const [file, candidates] of Object.entries(perFile) as [string, string[]][]) {
                  fileTokens.set(file, new Set<string>(candidates))
                  // Top up globalTokens in case the union scan missed a
                  // token added between the two calls (rare race —
                  // both calls share the same file list).
                  for (const t of candidates) state.globalTokens.add(t)
                }
              } catch {
                // best-effort
              }
            })()
          } else {
            // Production build: one-shot stream for the scan.
            const oneShot = createCompileStream()
            try {
              const perFile = (await oneShot.scanPerFile(scanRoots)) as Record<
                string,
                string[]
              >
              for (const [file, candidates] of Object.entries(perFile) as [string, string[]][]) {
                fileTokens.set(file, new Set<string>(candidates))
                for (const t of candidates) state.globalTokens.add(t)
              }
            } catch {
              // best-effort
            } finally {
              await oneShot.close()
            }
          }
        }
      })()
      // Intentionally not awaiting state.ready — Vite proceeds while
      // we scan; PostCSS gates on state.ready before the first CSS
      // touches the pipeline.
    },

    async buildEnd() {
      if (state.stream) {
        await state.stream.close()
        state.stream = null
      }
    },

    configureServer(srv) {
      for (const f of watchedFiles()) srv.watcher.add(f)
    },

    async handleHotUpdate({ file, server }) {
      if (!state.opts || !state.stream) return
      // Wait for the initial scan + per-file map to finish before
      // computing HMR deltas. In practice both promises usually resolve
      // long before the user can trigger an edit, but on a very fast
      // first edit the await guarantees `fileTokens.get(file)` has the
      // pre-edit token set (otherwise removed tokens would leak).
      if (state.ready) await state.ready
      if (state.fileTokensReady) await state.fileTokensReady

      const files = watchedFiles()
      if (files.includes(file)) {
        if (file === state.opts.configPath) {
          state.cachedConfig = null
          const reloaded = await getConfig()
          effectiveContentRoots =
            state.opts.contentRoots !== null
              ? state.opts.contentRoots
              : readConfigContent(reloaded, viteRoot)
        }
        if (state.opts.input) {
          const css = await compileVirtual()
          if (css !== lastVirtualCss) invalidateVirtual(server, state.opts.resolvedVirtualId)
        }
        invalidateTransformed(server)
        return []
      }

      const isContent = effectiveContentRoots.some(
        (r) => file.startsWith(r + '/') || file.startsWith(r + sep) || file === r,
      )
      if (!isContent) return

      const newFileTokens = new Set<string>((await state.stream.scan([file])) as string[])
      const oldFileTokens = fileTokens.get(file) ?? new Set<string>()

      const added = [...newFileTokens].filter((t) => !oldFileTokens.has(t))
      const removed = [...oldFileTokens].filter((t) => !newFileTokens.has(t))

      if (added.length === 0 && removed.length === 0) return []

      fileTokens.set(file, newFileTokens)
      for (const t of removed) {
        const stillPresent = [...fileTokens.values()].some(
          (s) => s !== newFileTokens && s.has(t as string),
        )
        if (!stillPresent) state.globalTokens.delete(t as string)
      }
      for (const t of added) state.globalTokens.add(t as string)

      // Invalidate every CSS module that went through our PostCSS
      // plugin so Vite re-runs the pipeline with the new candidates.
      invalidateTransformed(server)

      if (state.opts.input) {
        const css = await compileVirtual()
        if (css !== lastVirtualCss) invalidateVirtual(server, state.opts.resolvedVirtualId)
      }
      return []
    },

    resolveId(id) {
      if (!state.opts) return null
      if (id === state.opts.virtualId) return state.opts.resolvedVirtualId
      return null
    },

    async load(id) {
      if (!state.opts) return null
      if (id !== state.opts.resolvedVirtualId) return null
      // Virtual-module loader runs at first browser request; wait for
      // the cold-start scan before compiling so the emitted CSS has
      // the full candidate set.
      if (state.ready) await state.ready
      return await compileVirtual()
    },

    // Tracks which CSS modules went through the PostCSS pipeline so we
    // can invalidate them on a candidate-set change. We can't easily
    // know from the PostCSS plugin which Vite module id corresponds to
    // each Root, so we record every CSS module id Vite asks us about
    // here. This hook is the cheapest place to do that — it runs on
    // every module.
    transform(_code, id) {
      if (!state.opts) return null
      // Only record ids that LOOK like CSS — skip JS modules.
      const bare = id.split('?')[0]!.toLowerCase()
      if (
        bare.endsWith('.css') ||
        bare.endsWith('.scss') ||
        bare.endsWith('.sass') ||
        bare.endsWith('.less') ||
        bare.endsWith('.styl') ||
        bare.endsWith('.stylus')
      ) {
        transformedIds.add(id)
      }
      return null
    },
  }
}

function invalidateVirtual(srv: ViteDevServer, resolvedVirtualId: string): void {
  const mod = srv.moduleGraph.getModuleById(resolvedVirtualId)
  if (mod) {
    srv.moduleGraph.invalidateModule(mod)
    const hot = (srv as ViteDevServer & { hot?: ViteDevServer['ws'] }).hot
    if (hot) {
      hot.send({ type: 'full-reload' })
    } else {
      srv.ws.send({ type: 'full-reload' })
    }
  }
}

function surfaceDiagnostics(
  diagnostics: { severity: 'error' | 'warning' | 'info'; code: string; message: string }[],
  config: LoadedConfig,
  state: PluginState,
): void {
  if (!state.warnedPlugins && config.pluginNames.length > 0) {
    state.warnedPlugins = true
    // eslint-disable-next-line no-console
    console.warn(
      `[vite-plugin-galeforcecss] Tailwind plugins declared in your config but GaleforceCSS v1 ignores them: ${config.pluginNames.join(', ')}.`,
    )
  }
  for (const d of diagnostics) {
    if (d.severity === 'error') {
      // eslint-disable-next-line no-console
      console.error(`[vite-plugin-galeforcecss] [${d.code}] ${d.message}`)
    }
  }
}
