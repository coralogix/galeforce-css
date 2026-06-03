// Fixture format and loader. See `todo.md` § 7.

import { readFile } from 'node:fs/promises'

export interface Fixture {
  name: string
  description?: string
  /**
   * Custom CSS input. Defaults to `@tailwind utilities;` (preflight off).
   */
  inputCss?: string
  /**
   * Inline Tailwind config. Defaults to `{ corePlugins: { preflight: false } }`
   * so fixtures focus on utility output, not the 3kB preflight reset.
   */
  config?: Record<string, unknown>
  candidates: string[]
  /**
   * `'utilities'` (default) compiles only the utility layer.
   * `'full'` compiles base + components + utilities (preflight included).
   */
  mode?: 'utilities' | 'full'
  orderSensitive?: boolean
  /** PostCSS warnings the oracle is expected to emit, by substring match. */
  expectedWarnings?: string[]
  /**
   * Enable nested-CSS expansion in BOTH sides of the diff. The oracle
   * runs `tailwindcss/nesting` ahead of `tailwindcss` in its PostCSS
   * chain; Galeforce runs its Rust port. Diff is then byte-equivalent after
   * the conformance normalizer.
   */
  nesting?: boolean
}

export async function loadFixture(path: string): Promise<Fixture> {
  const raw = await readFile(path, 'utf8')
  const parsed = JSON.parse(raw) as Fixture
  validateFixture(parsed, path)
  return parsed
}

export function validateFixture(fx: Fixture, source: string): void {
  if (typeof fx.name !== 'string' || fx.name.length === 0) {
    throw new Error(`fixture ${source}: missing string \`name\``)
  }
  if (!Array.isArray(fx.candidates) || fx.candidates.some((c) => typeof c !== 'string')) {
    throw new Error(`fixture ${source}: \`candidates\` must be string[]`)
  }
  if (fx.mode && fx.mode !== 'utilities' && fx.mode !== 'full') {
    throw new Error(`fixture ${source}: \`mode\` must be "utilities" | "full"`)
  }
}
