// Conformance runner: oracle (Tailwind 3) vs. Galeforce, on a single fixture.

import { compileWithTailwind3 } from '@cx/galeforcecss-oracle'
import { compileWithGaleforce, GaleforceNotImplemented } from './galeforce.js'
import { diffStylesheets, formatDiff, type DiffEntry } from './diff.js'
import { normalizeCss } from './normalize.js'
import type { Fixture } from './fixture.js'

export interface RunResult {
  fixture: string
  /** Empty array => fixture passed. */
  diffs: DiffEntry[]
  /** Pretty-printed diff suitable for assertion messages. */
  report: string
  /** True if the Galeforce compiler hasn't implemented this surface yet. */
  unimplemented: boolean
  oracleCss: string
  galeforceCss: string | null
}

export async function runFixture(fx: Fixture): Promise<RunResult> {
  const inputCss = fx.inputCss ?? defaultInputCss(fx.mode ?? 'utilities')
  const config = fx.config ?? defaultConfig(fx.mode ?? 'utilities')

  const oracle = await compileWithTailwind3({
    candidates: fx.candidates,
    inputCss,
    config,
    nesting: fx.nesting === true,
  })

  let galeforceCss: string | null = null
  let unimplemented = false
  try {
    const galeforce = await compileWithGaleforce({
      candidates: fx.candidates,
      inputCss,
      config,
      features: { nesting: fx.nesting === true },
    })
    galeforceCss = galeforce.css
  } catch (err) {
    if (err instanceof GaleforceNotImplemented) {
      unimplemented = true
    } else {
      throw err
    }
  }

  if (unimplemented || galeforceCss === null) {
    return {
      fixture: fx.name,
      unimplemented: true,
      oracleCss: oracle.css,
      galeforceCss: null,
      diffs: [
        {
          kind: 'missing',
          context: [],
          selector: '*',
          detail: 'Galeforce compiler not implemented yet (placeholder)',
        },
      ],
      report: 'GaleforceCSS compiler is a placeholder — fixture intentionally fails.',
    }
  }

  const oracleNorm = normalizeCss(oracle.css)
  const galeforceNorm = normalizeCss(galeforceCss)
  const diffs = diffStylesheets(galeforceNorm, oracleNorm, {
    orderSensitive: fx.orderSensitive === true,
  })

  return {
    fixture: fx.name,
    unimplemented: false,
    oracleCss: oracle.css,
    galeforceCss,
    diffs,
    report: formatDiff(diffs),
  }
}

function defaultInputCss(mode: 'utilities' | 'full'): string {
  return mode === 'full'
    ? '@tailwind base;\n@tailwind components;\n@tailwind utilities;\n'
    : '@tailwind utilities;\n'
}

function defaultConfig(mode: 'utilities' | 'full'): Record<string, unknown> {
  return mode === 'full' ? {} : { corePlugins: { preflight: false } }
}
