// Locate the user's installed `tailwindcss` package so we can use
// THEIR `resolveConfig` (and main entrypoint) instead of our bundled
// fallback. This matters because Tailwind's `defaultTheme` differs
// between minor versions (`size-*`, the spacing-derived
// `minHeight`/`minWidth`/`maxWidth` scales, `opacity-45`, etc. were
// added in v3.4) and our config-loader sits on a pinned `^3.4.19`
// fallback that would otherwise leak v3.4-only theme defaults into a
// project pinned at v3.3.x.
//
// Resolution order:
//   1. `tailwindcss/resolveConfig.js` resolved from the user's project
//      root (`createRequire` walks up `node_modules`). If found, we
//      `require` both that and the package main and read the version
//      from its `package.json`.
//   2. Fallback to our bundled imports.
//
// Both v3.3.x and v3.4.x ship as CommonJS, so `createRequire` works
// for the whole supported range.

import { createRequire } from 'node:module'
import { dirname, resolve as resolvePath } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import bundledResolveConfig from 'tailwindcss/resolveConfig.js'
import bundledTailwind from 'tailwindcss'
// Read the bundled version straight off package.json so the
// `version` field on the returned module is honest even on the
// fallback path.
const here = dirname(fileURLToPath(import.meta.url))
const bundledRequire = createRequire(import.meta.url)
const bundledPkg = bundledRequire('tailwindcss/package.json') as { version: string }

export interface TailwindModule {
  /** `resolveConfig` from the located install — used to merge the
   *  user's tailwind.config.* against `defaultTheme`. */
  resolveConfig: (config: unknown) => Record<string, unknown>
  /** Main tailwindcss entrypoint — used to build the stub PostCSS
   *  pipeline that produces `@tailwind base;` output for projects
   *  with overridden preflight inputs. */
  tailwindcss: (...args: unknown[]) => unknown
  /** Installed version (`x.y.z`) for diagnostics. */
  version: string
  /** Where the module came from. `'user'` means the user's project
   *  `node_modules` resolved it; `'bundled'` means we fell back to
   *  the galeforcecss-pinned copy. */
  source: 'user' | 'bundled'
  /** Absolute path to the resolved `tailwindcss/package.json` (or
   *  `null` for the bundled fallback if the bundle's location is
   *  unknown). */
  packageJsonPath: string | null
}

const cache = new Map<string, TailwindModule>()

/**
 * Resolve `tailwindcss` from `searchFrom`. Returns the user's install
 * when found, otherwise the bundled fallback. Result cached by
 * `searchFrom` to avoid repeated filesystem walks.
 *
 * @param searchFrom Absolute path used as the resolution origin. Pass
 *   the directory containing the user's `tailwind.config.*` so node's
 *   resolution walks up from there to find their `node_modules`.
 */
export function resolveTailwind(searchFrom: string | null | undefined): TailwindModule {
  const key = searchFrom ?? '<null>'
  const cached = cache.get(key)
  if (cached) return cached

  const fallback: TailwindModule = {
    resolveConfig: bundledResolveConfig as TailwindModule['resolveConfig'],
    tailwindcss: bundledTailwind as TailwindModule['tailwindcss'],
    version: bundledPkg.version,
    source: 'bundled',
    packageJsonPath: null,
  }

  if (!searchFrom) {
    cache.set(key, fallback)
    return fallback
  }

  // Anchor the require at a file inside `searchFrom`. `createRequire`
  // doesn't accept a bare directory, but a `file:` URL pointing at a
  // synthetic path inside the directory makes node walk `node_modules`
  // from there — exactly what we want.
  const anchor = pathToFileURL(resolvePath(searchFrom, 'noop.js')).href
  const userRequire = createRequire(anchor)
  try {
    const pkgPath = userRequire.resolve('tailwindcss/package.json')
    const pkg = userRequire(pkgPath) as { version?: string }
    const version = pkg.version ?? '0.0.0'
    const userResolveConfig = userRequire('tailwindcss/resolveConfig.js') as
      | TailwindModule['resolveConfig']
      | { default: TailwindModule['resolveConfig'] }
    const userTailwind = userRequire('tailwindcss') as
      | TailwindModule['tailwindcss']
      | { default: TailwindModule['tailwindcss'] }
    // CJS and ESM both reach us as either the function itself or an
    // `{ default: fn }` shape — unwrap either.
    const resolveConfig = (
      typeof userResolveConfig === 'function'
        ? userResolveConfig
        : userResolveConfig.default
    ) as TailwindModule['resolveConfig']
    const tailwindcss = (
      typeof userTailwind === 'function' ? userTailwind : userTailwind.default
    ) as TailwindModule['tailwindcss']
    if (typeof resolveConfig !== 'function' || typeof tailwindcss !== 'function') {
      throw new Error('unexpected tailwindcss module shape')
    }
    const mod: TailwindModule = {
      resolveConfig,
      tailwindcss,
      version,
      source: 'user',
      packageJsonPath: pkgPath,
    }
    cache.set(key, mod)
    return mod
  } catch {
    // Resolution failed (no install, ESM-only future version, etc.)
    // — fall through to the bundled copy. Silent because callers can
    // surface a diagnostic if they care; we don't want a noisy log
    // every config load.
    cache.set(key, fallback)
    return fallback
  }
}

/** Test hook — drop the resolver cache. */
export function _resetResolveTailwindCache(): void {
  cache.clear()
}

// Re-export `here` so callers can build relative paths consistently
// against the loader's own module location.
export const _loaderModuleDir = here
