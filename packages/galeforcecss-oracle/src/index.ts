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

// Oracle compiler: drives the official tailwindcss@3.4.19 PostCSS plugin
// against synthetic raw HTML containing the requested candidates.
//
// This is the *executable* conformance reference. Every Galeforce utility/variant
// is verified by comparing Galeforce's output against this oracle's output for
// the same candidates.

import postcss from 'postcss'
import tailwindcss from 'tailwindcss'
// @ts-expect-error - tailwindcss/src/... has no .d.ts shipped.
import * as defaultExtractorMod from 'tailwindcss/src/lib/defaultExtractor.js'
import { escapeForHtmlAttribute } from './escape.js'

/**
 * Run upstream's `defaultExtractor` over `content` and return the
 * extracted candidate strings. Mirrors the JIT behavior — including
 * sub-token extraction (`underline` falling out of
 * `aria-[…'…']:underline` when the embedded quote splits the
 * candidate). Re-exported here because the conformance harness
 * package can't reach into `tailwindcss/src/...` from its own
 * `node_modules` tree, but the oracle package depends on
 * tailwindcss directly.
 */
export function extractWithDefaultExtractor(
  content: string,
  prefix = '',
  separator = ':',
): string[] {
  type Factory = (cx: unknown) => (s: string) => string[]
  const mod = defaultExtractorMod as unknown as Record<string, unknown>
  const factory = (mod.defaultExtractor ?? mod.default ?? mod) as Factory
  if (typeof factory !== 'function') return []
  const fn = factory({ tailwindConfig: { separator, prefix } })
  const out = fn(content) ?? []
  return out.filter((t): t is string => typeof t === 'string')
}

export interface OracleCompileOptions {
  candidates: string[]
  /**
   * Custom CSS input. Defaults to:
   *   `@tailwind base; @tailwind components; @tailwind utilities;`
   */
  inputCss?: string
  /**
   * Inline Tailwind config (object form). Cannot be combined with `configPath`.
   */
  config?: Record<string, unknown>
  /**
   * Path to a Tailwind config file. Loaded by Tailwind itself.
   */
  configPath?: string
  /**
   * Synthetic content file extension. Defaults to `.html`.
   */
  contentExtension?: string
  /**
   * Include `tailwindcss/nesting` in the PostCSS chain so the
   * oracle expands `&` / nested-rule shapes the same way the
   * project would when running through PostCSS. Used by the
   * conformance harness to compare GaleforceCSS's Rust-side
   * nesting expansion against the upstream pipeline.
   */
  nesting?: boolean
}

export interface OracleCompileResult {
  /** Raw CSS as produced by tailwindcss@3.4.19. */
  css: string
  /** PostCSS warnings (e.g. invalid `@apply` usage). */
  warnings: string[]
}

const DEFAULT_INPUT_CSS = '@tailwind base;\n@tailwind components;\n@tailwind utilities;\n'

/**
 * Compile a list of candidate classes through the official Tailwind v3 oracle.
 *
 * The candidates are embedded into a synthetic HTML document so Tailwind's
 * default JIT scanner picks them up unchanged, including arbitrary values and
 * arbitrary variants.
 */
export async function compileWithTailwind3(
  options: OracleCompileOptions,
): Promise<OracleCompileResult> {
  if (options.config && options.configPath) {
    throw new Error("oracle: pass either `config` or `configPath`, not both.")
  }

  const ext = options.contentExtension ?? '.html'
  const html = synthesizeContent(options.candidates)
  const inputCss = options.inputCss ?? DEFAULT_INPUT_CSS

  const baseConfig: Record<string, unknown> = options.config
    ? { ...options.config }
    : {}
  // Inject our synthetic content. If the user supplied `content`, we prepend
  // ours so their globs still work.
  const userContent = (baseConfig.content as unknown) ?? []
  const userContentArray = Array.isArray(userContent)
    ? userContent
    : userContent && typeof userContent === 'object' && 'files' in (userContent as object)
      ? ((userContent as { files: unknown[] }).files ?? [])
      : []
  baseConfig.content = [
    { raw: html, extension: ext.replace(/^\./, '') },
    ...userContentArray,
  ]

  const plugin = options.configPath
    ? tailwindcss(options.configPath)
    : tailwindcss(baseConfig as Parameters<typeof tailwindcss>[0])

  // `tailwindcss/nesting` runs BEFORE the tailwindcss plugin in the
  // pipeline upstream recommends, so `@apply`/`@tailwind`/`@screen`
  // see flat CSS. Only included when the harness asks for it.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const plugins: any[] = options.nesting
    ? // eslint-disable-next-line @typescript-eslint/no-require-imports
      [require('tailwindcss/nesting')(), plugin]
    : [plugin]

  const result = await postcss(plugins).process(inputCss, {
    from: undefined,
  })

  return {
    css: result.css,
    warnings: result.warnings().map((w) => w.toString()),
  }
}

function synthesizeContent(candidates: string[]): string {
  // Each candidate goes into its own `class="…"` attribute so Tailwind's
  // default content extractor (a permissive regex) splits them on the
  // attribute boundary instead of on whitespace, which preserves arbitrary
  // values like `grid-cols-[1fr_2fr]` byte-for-byte.
  const attrs = candidates.map((c) => `class="${escapeForHtmlAttribute(c)}"`).join('\n')
  return `<!doctype html>\n<html><body>\n${attrs}\n</body></html>\n`
}
