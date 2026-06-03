// Public GaleforceCSS Node API. Two implementations under the hood:
//
//   1. **Native (napi-rs)** — preferred when the platform-specific
//      `galeforcecss-<os>-<arch>` package is installed. The `.node`
//      artifact is `require()`d directly, eliminating the CLI spawn
//      + JSON round-trip. `compile()` becomes a synchronous in-process
//      call wrapped in `Promise.resolve` so the API surface stays async.
//   2. **CLI bridge** — fallback when the `.node` artifact isn't
//      available. Spawns the `galeforcecss` binary and exchanges JSON
//      over stdin/stdout. Identical wire format to the conformance
//      harness; identical observable behavior to the native path.
//
// `createCompileStream` always uses the CLI bridge today — it gives
// strict ordering + the stream-style `scan`/`scanPerFile` calls that
// match the existing protocol. Worth porting to native once we have
// a use case where the per-call dispatch latency matters.

import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process'
import { existsSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createInterface } from 'node:readline'

export interface Diagnostic {
  severity: 'error' | 'warning' | 'info'
  code: string
  message: string
  candidate?: string
}

/**
 * Per-call compatibility flags for matching legacy Tailwind output
 * byte-for-byte. Defaults match the pinned target Tailwind v3.4.19.
 */
export interface CompileCompat {
  /**
   * Which Tailwind v3 patch line to match byte-for-byte. Defaults to
   * `'3.4'` (the pinned conformance oracle). Set to `'3.3'` when
   * your project is still on `tailwindcss@3.3.x` and the swap should
   * produce a byte-identical CSS bundle. The v3.3 mode bundles every
   * cosmetic byte difference between the two patch lines:
   *
   * - Color-utility `var(--tw-*-opacity)` calls omit the `, 1`
   *   fallback v3.4 added.
   * - Commas inside arbitrary-value selectors escape as `\,` instead
   *   of `\2c `.
   * - Commas inside math-function values stay tight
   *   (`min(420px,50vh)`) instead of getting a trailing space.
   *
   * All v3.3-vs-v3.4 differences here are cosmetic — runtime CSS
   * behavior is identical. The switch exists so projects can swap
   * GaleforceCSS in without their built CSS bundle changing visibly.
   */
  tailwindVersion?: '3.3' | '3.4'
}

export interface CompileFeatures {
  /** Minify the CSS output via Lightning CSS. */
  minify?: boolean
  /** Emit a Source Map v3 JSON alongside the CSS. */
  sourceMaps?: boolean
  /**
   * Per-browser minimum versions, e.g. `{ chrome: '95', firefox: '94' }`.
   * When set, Lightning CSS lowers and prefixes modern CSS features.
   * Resolve a browserslist query with the `browserslist` npm package
   * if you want the standard query syntax.
   */
  targets?: Record<string, string>
  /**
   * Version-compatibility flags. Default behavior matches the pinned
   * Tailwind v3.4.19 target; flip individual flags here to match
   * older v3 patch versions byte-for-byte.
   */
  compat?: CompileCompat
  /**
   * Enable nested-CSS expansion (a Rust port of
   * `tailwindcss/nesting` + `postcss-nested@6.2.0`). When true,
   * the compiler flattens `.foo { .bar { ... } }` and `.btn {
   * &:hover { ... } }` shapes before `@apply` / `@tailwind` /
   * `@screen` directives run, so existing Tailwind machinery sees
   * flat CSS. Default false — projects that don't author nested
   * CSS pay zero cost.
   */
  nesting?: boolean
}

export interface CompileInput {
  candidates?: string[]
  inputCss?: string
  /** Path the input CSS came from. Used as `sources[0]` in source maps. */
  inputCssPath?: string
  /** Resolved Tailwind config (use `@cx/galeforcecss-config-loader` to produce). */
  config?: Record<string, unknown>
  /**
   * Content roots to scan for candidates. The Rust side walks these
   * paths, tokenizes every file it can decode as UTF-8, dedupes, and
   * merges the result into `candidates`. Pre-supplied `candidates`
   * take priority and remain at the head of the list.
   */
  content?: string[]
  /** Final-stage transform options (minify / source maps / targets). */
  features?: CompileFeatures
}

export interface CompileOutput {
  css: string
  /** Source Map v3 JSON. Present only when `features.sourceMaps` was set. */
  map: string | null
  diagnostics: Diagnostic[]
  candidateCount: number
  ruleCount: number
}

export class GaleforceBinaryNotFoundError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'GaleforceBinaryNotFoundError'
  }
}

const here = dirname(fileURLToPath(import.meta.url))
// We resolve from this file's location: when the package is installed
// in a real project, this points into node_modules and we look for the
// shipped binary alongside it. In the monorepo it walks up to the repo
// root and finds the Cargo target directories.
const PACKAGE_ROOT = resolve(here, '..')
const REPO_ROOT_CANDIDATES = [
  resolve(PACKAGE_ROOT, '../..'), // monorepo: packages/galeforcecss/dist -> repo root
  resolve(PACKAGE_ROOT, '..'), // npm install: node_modules/galeforcecss/dist -> node_modules
]

function getPlatformPackageName(): string | null {
  const { platform, arch } = process
  if (platform === 'darwin' && arch === 'arm64') return '@cx/galeforcecss-darwin-arm64'
  if (platform === 'darwin' && arch === 'x64') return '@cx/galeforcecss-darwin-x64'
  if (platform === 'linux' && arch === 'x64') return '@cx/galeforcecss-linux-x64-gnu'
  if (platform === 'linux' && arch === 'arm64') return '@cx/galeforcecss-linux-arm64-gnu'
  if (platform === 'win32' && arch === 'x64') return '@cx/galeforcecss-win32-x64-msvc'
  return null
}

/**
 * The napi-rs binding surface, exported by `crates/galeforce-node`. We
 * load it lazily so missing `.node` artifacts fall back to the CLI
 * bridge instead of crashing at import time.
 */
interface NativeBinding {
  compile(optionsJson: string): string
  scan(pathsJson: string): string
  scanPerFile(rootsJson: string): string
  version(): string
}

let nativeBindingCache: NativeBinding | null | undefined = undefined

/**
 * Try to load the napi-rs binding for this platform. Returns `null` if
 * the artifact isn't installed (most installs hit this until publish)
 * or fails to load. Caches the result so we don't pay the resolve cost
 * on every call.
 *
 * Search order:
 * 1. `GALEFORCECSS_NATIVE_BIN` env var (escape hatch for dev iteration).
 * 2. The platform-specific optional package (e.g. `@cx/galeforcecss-darwin-arm64`).
 * 3. The repo-root `target/release/libgaleforce_node.dylib|.so|.dll` (monorepo dev).
 */
function loadNative(): NativeBinding | null {
  if (nativeBindingCache !== undefined) return nativeBindingCache
  nativeBindingCache = null
  if (process.env.GALEFORCECSS_DISABLE_NATIVE === '1') return null

  const req = createRequire(import.meta.url)
  const tryLoad = (path: string): NativeBinding | null => {
    try {
      if (!existsSync(path)) return null
      const mod = req(path) as NativeBinding
      if (typeof mod?.compile !== 'function') return null
      return mod
    } catch {
      return null
    }
  }

  const fromEnv = process.env.GALEFORCECSS_NATIVE_BIN
  if (fromEnv) {
    nativeBindingCache = tryLoad(fromEnv)
    if (nativeBindingCache) return nativeBindingCache
  }

  const platformPkg = getPlatformPackageName()
  if (platformPkg) {
    try {
      const resolved = req.resolve(`${platformPkg}/galeforcecss.node`)
      nativeBindingCache = tryLoad(resolved)
      if (nativeBindingCache) return nativeBindingCache
    } catch {
      // Optional dep not installed — fall through.
    }
  }

  // Monorepo dev fallback: look in Cargo's target directories.
  const monorepoExt =
    process.platform === 'win32'
      ? 'galeforce_node.dll'
      : process.platform === 'darwin'
        ? 'libgaleforce_node.dylib'
        : 'libgaleforce_node.so'
  for (const root of REPO_ROOT_CANDIDATES) {
    for (const profile of ['release', 'debug']) {
      const candidate = resolve(root, `target/${profile}/${monorepoExt}`)
      nativeBindingCache = tryLoad(candidate)
      if (nativeBindingCache) return nativeBindingCache
    }
  }
  return null
}

function resolveBinary(): string | null {
  const fromEnv = process.env.GALEFORCE_CLI_BIN
  if (fromEnv && existsSync(fromEnv)) return fromEnv

  // Try the platform-specific optional package first (installed users).
  const platformPkg = getPlatformPackageName()
  if (platformPkg) {
    const req = createRequire(import.meta.url)
    const binaryName = process.platform === 'win32' ? 'galeforcecss.exe' : 'galeforcecss'
    try {
      const resolved = req.resolve(`${platformPkg}/${binaryName}`)
      if (existsSync(resolved)) return resolved
    } catch {
      // Optional dependency not installed — fall through to target/ search.
    }
  }

  // Monorepo / local dev fallback: look in Cargo's target directories.
  for (const root of REPO_ROOT_CANDIDATES) {
    const candidates = [
      resolve(root, 'target/release/galeforcecss'),
      resolve(root, 'target/debug/galeforcecss'),
      resolve(root, 'galeforcecss/bin/galeforcecss'),
    ]
    for (const c of candidates) {
      if (existsSync(c)) return c
    }
  }
  return null
}

function requireBinary(): string {
  const bin = resolveBinary()
  if (!bin) {
    throw new GaleforceBinaryNotFoundError(
      `galeforcecss binary not found. Build it with \`cargo build -p galeforce-cli --release\` ` +
        `at the repo root, or set GALEFORCE_CLI_BIN to its path.`,
    )
  }
  return bin
}

interface RawResult {
  output: { css: string; map?: string | null }
  diagnostics: Diagnostic[]
  candidateCount: number
  ruleCount: number
}

function rawToOutput(raw: RawResult): CompileOutput {
  return {
    css: raw.output.css,
    map: raw.output.map ?? null,
    diagnostics: raw.diagnostics,
    candidateCount: raw.candidateCount,
    ruleCount: raw.ruleCount,
  }
}

function buildPayload(input: CompileInput): string {
  const features = input.features
  return JSON.stringify({
    candidates: input.candidates ?? [],
    inputCss: input.inputCss,
    inputCssPath: input.inputCssPath,
    config: input.config,
    content: input.content ?? [],
    features: features
      ? {
          minify: features.minify ?? false,
          sourceMaps: features.sourceMaps ?? false,
          targets: features.targets,
          compat: features.compat
            ? { tailwindVersion: features.compat.tailwindVersion ?? '3.4' }
            : undefined,
          nesting: features.nesting ?? false,
        }
      : undefined,
  })
}

/**
 * Compile a single batch of candidates + input CSS. Spawns the CLI for
 * each call. Use `createCompileStream` for repeated invocations.
 */
export async function compile(input: CompileInput = {}): Promise<CompileOutput> {
  // Prefer the native binding if available — eliminates the CLI spawn
  // + JSON I/O round-trip. The native call is synchronous; we wrap it
  // in `Promise.resolve` so callers don't have to know which path ran.
  const native = loadNative()
  if (native) {
    try {
      const raw = JSON.parse(native.compile(buildPayload(input))) as RawResult
      return Promise.resolve(rawToOutput(raw))
    } catch (err) {
      // If the native call throws (parse error, panic), fall through to
      // the CLI bridge rather than failing — better than no answer.
      // eslint-disable-next-line no-console
      console.warn(
        `[galeforcecss] native binding failed, falling back to CLI: ${(err as Error).message}`,
      )
    }
  }

  const bin = requireBinary()
  const payload = buildPayload(input)
  return new Promise((resolveProm, reject) => {
    const child = spawn(bin, ['compile-json'], { stdio: ['pipe', 'pipe', 'pipe'] })
    let stdout = ''
    let stderr = ''
    child.stdout.setEncoding('utf8')
    child.stderr.setEncoding('utf8')
    child.stdout.on('data', (c: string) => {
      stdout += c
    })
    child.stderr.on('data', (c: string) => {
      stderr += c
    })
    child.on('error', reject)
    child.on('close', (code) => {
      if (code !== 0) {
        reject(new Error(`galeforcecss compile-json exited with ${code}: ${stderr || '(no stderr)'}`))
        return
      }
      try {
        const raw = JSON.parse(stdout) as RawResult
        resolveProm(rawToOutput(raw))
      } catch (err) {
        reject(
          new Error(
            `galeforcecss compile-json produced non-JSON output: ${(err as Error).message}\n--- stdout ---\n${stdout}`,
          ),
        )
      }
    })
    child.stdin.write(payload)
    child.stdin.end()
  })
}

/**
 * Long-running compile interface. Spawns one CLI process and streams
 * JSONL requests/responses. Each `.compile()` call writes one line to
 * stdin and resolves on the matching response line. Call `.close()` to
 * gracefully terminate the child.
 *
 * Calls are serialised in the order they're issued — the underlying
 * `compile-stream` subcommand processes one request per line.
 */
export interface CompileStream {
  compile(input: CompileInput): Promise<CompileOutput>
  /** Scan a flat list of file paths and return their merged candidate set. */
  scan(paths: string[]): Promise<string[]>
  /** Scan content roots and return a per-file candidate map. */
  scanPerFile(roots: string[]): Promise<Record<string, string[]>>
  close(): Promise<void>
}

type Pending =
  | { kind: 'compile'; resolve: (out: CompileOutput) => void; reject: (e: Error) => void }
  | { kind: 'scan'; resolve: (candidates: string[]) => void; reject: (e: Error) => void }
  | {
      kind: 'scanPerFile'
      resolve: (files: Record<string, string[]>) => void
      reject: (e: Error) => void
    }

export function createCompileStream(): CompileStream {
  const bin = requireBinary()

  // Pending resolvers, in submission order. The CLI guarantees one
  // response per request line.
  const pending: Array<Pending> = []
  // Permanent close — set when the user calls .close(). Distinct from
  // the per-child `exit` event, which can fire spontaneously (panic,
  // OOM-kill, parent SIGPIPE, etc.) and is transparently recovered by
  // respawning the child on the next request.
  let userClosed = false
  let child: ChildProcessWithoutNullStreams | null = null
  let rl: ReturnType<typeof createInterface> | null = null
  // Per-child stderr buffer — cleared each time we (re)spawn so a death
  // diagnostic only ever surfaces stderr from THAT child's lifetime.
  let stderrBuf = ''

  function spawnChild(): void {
    const next = spawn(bin, ['compile-stream'], {
      stdio: ['pipe', 'pipe', 'pipe'],
    })
    next.stdout.setEncoding('utf8')
    next.stderr.setEncoding('utf8')
    child = next
    stderrBuf = ''
    rl = createInterface({ input: next.stdout })

    rl.on('line', (line) => {
      const head = pending.shift()
      if (!head) return
      try {
        const parsed = JSON.parse(line) as { type?: string }
        if (parsed.type === 'scan') {
          const msg = parsed as { type: 'scan'; candidates: string[] }
          if (head.kind === 'scan') {
            head.resolve(msg.candidates)
          } else {
            head.reject(new Error(`galeforcecss compile-stream: unexpected scan response`))
          }
        } else if (parsed.type === 'scan-per-file') {
          const msg = parsed as { type: 'scan-per-file'; files: Record<string, string[]> }
          if (head.kind === 'scanPerFile') {
            head.resolve(msg.files)
          } else {
            head.reject(new Error(`galeforcecss compile-stream: unexpected scan-per-file response`))
          }
        } else {
          const raw = parsed as RawResult
          if (head.kind === 'compile') {
            head.resolve(rawToOutput(raw))
          } else {
            head.reject(new Error(`galeforcecss compile-stream: unexpected compile response`))
          }
        }
      } catch (err) {
        head.reject(
          new Error(`galeforcecss compile-stream produced non-JSON line: ${(err as Error).message}`),
        )
      }
    })

    next.stderr.on('data', (c: string) => {
      stderrBuf += c
      if (process.env['GALEFORCE_BENCH_SCAN'] || process.env['GALEFORCE_BENCH_COMPILE']) {
        process.stderr.write(c)
      }
    })

    // Swallow EPIPE when the child has already exited and we're still
    // mid-write. The exit handler rejects the pending request with the
    // proper diagnostic; we don't want an unhandled stdin error to
    // crash the host process on top of that.
    next.stdin.on('error', () => {})

    next.on('exit', (code, signal) => {
      // Reject anything in-flight with a useful diagnostic. The next
      // request after this respawns transparently — callers see one
      // failed compile, not a permanently dead stream.
      const exited = next
      if (child === exited) {
        rl?.close()
        rl = null
        child = null
      }
      if (pending.length > 0) {
        const tail = stderrBuf.trim()
        const sig = signal ? ` signal=${signal}` : ''
        const reason = tail
          ? `: ${tail.length > 4000 ? '...' + tail.slice(-4000) : tail}`
          : ' (no stderr captured — set GALEFORCE_BENCH_COMPILE=1 to surface child stderr)'
        const err = new Error(
          `galeforcecss compile-stream child exited (code=${code}${sig})${reason}`,
        )
        for (const p of pending) p.reject(err)
        pending.length = 0
      }
    })
  }

  function ensureChild(): ChildProcessWithoutNullStreams {
    if (userClosed) {
      throw new Error('galeforcecss compile-stream is closed')
    }
    if (!child) spawnChild()
    return child!
  }

  spawnChild()

  return {
    compile(input: CompileInput): Promise<CompileOutput> {
      let c: ChildProcessWithoutNullStreams
      try {
        c = ensureChild()
      } catch (err) {
        return Promise.reject(err as Error)
      }
      return new Promise<CompileOutput>((resolveProm, reject) => {
        pending.push({ kind: 'compile', resolve: resolveProm, reject })
        const payload = buildPayload(input)
        c.stdin.write(payload + '\n')
      })
    },
    scan(paths: string[]): Promise<string[]> {
      let c: ChildProcessWithoutNullStreams
      try {
        c = ensureChild()
      } catch (err) {
        return Promise.reject(err as Error)
      }
      return new Promise<string[]>((resolveProm, reject) => {
        pending.push({ kind: 'scan', resolve: resolveProm, reject })
        const payload = JSON.stringify({ type: 'scan', content: paths })
        c.stdin.write(payload + '\n')
      })
    },
    scanPerFile(roots: string[]): Promise<Record<string, string[]>> {
      let c: ChildProcessWithoutNullStreams
      try {
        c = ensureChild()
      } catch (err) {
        return Promise.reject(err as Error)
      }
      return new Promise<Record<string, string[]>>((resolveProm, reject) => {
        pending.push({ kind: 'scanPerFile', resolve: resolveProm, reject })
        const payload = JSON.stringify({ type: 'scan-per-file', content: roots })
        c.stdin.write(payload + '\n')
      })
    },
    close(): Promise<void> {
      userClosed = true
      const c = child
      if (!c) return Promise.resolve()
      c.stdin.end()
      return new Promise<void>((resolveProm) => {
        if (c.exitCode !== null) {
          resolveProm()
        } else {
          c.once('exit', () => resolveProm())
        }
      })
    },
  }
}

/**
 * Reflection: every known static utility class name (plus any
 * plugin-defined utilities/components captured in
 * `config.__pluginOutput`). Used by editor extensions for
 * autocomplete. Value-bearing utilities (`mt-1`, `bg-red-500`, …)
 * are NOT enumerated — most editors prefix-match from typed input.
 */
export function listClasses(config?: Record<string, unknown>): string[] {
  return runListSubcommand('list-classes', config)
}

/** Reflection: every known variant name (built-in + plugin). */
export function listVariants(config?: Record<string, unknown>): string[] {
  return runListSubcommand('list-variants', config)
}

function runListSubcommand(
  sub: 'list-classes' | 'list-variants',
  config?: Record<string, unknown>,
): string[] {
  const bin = requireBinary()
  const args: string[] = [sub]
  let configPath: string | undefined
  // If a config is supplied we have to write it to a file because
  // the CLI subcommand reads `--config <path>` rather than stdin.
  // Tiny temp file; the cost is amortised by the editor calling
  // these once per project, not per keystroke.
  if (config) {
    const fs = require('node:fs') as typeof import('node:fs')
    const path = require('node:path') as typeof import('node:path')
    const os = require('node:os') as typeof import('node:os')
    configPath = path.join(os.tmpdir(), `galeforcecss-list-${process.pid}.json`)
    fs.writeFileSync(configPath, JSON.stringify(config))
    args.push('--config', configPath)
  }
  const { spawnSync } = require('node:child_process') as typeof import('node:child_process')
  const r = spawnSync(bin, args, { encoding: 'utf8' })
  if (configPath) {
    try {
      ;(require('node:fs') as typeof import('node:fs')).unlinkSync(configPath)
    } catch {
      // Best-effort cleanup.
    }
  }
  if (r.status !== 0) {
    throw new Error(`galeforcecss ${sub} exited with ${r.status}: ${r.stderr || '(no stderr)'}`)
  }
  return JSON.parse(r.stdout) as string[]
}

export const version = '0.0.0'

// Re-export the config loader so consumers get one entry point.
export {
  loadConfig,
  findConfigPath,
  DEFAULT_CONFIG_FILES,
  type LoadConfigOptions,
  type LoadedConfig,
} from '@cx/galeforcecss-config-loader'
