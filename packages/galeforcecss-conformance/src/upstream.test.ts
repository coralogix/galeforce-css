// Upstream Tailwind v3 comprehensive test runner.
//
// `vendor/tailwindcss-v3/tests/` has thousands of test cases written
// against Tailwind's own JIT pipeline. We import the four biggest /
// most-comprehensive ones — `basic-usage`, `arbitrary-values`,
// `variants`, `raw-content` — and replay them against BOTH Galeforce and
// the oracle, asserting byte-equivalence after PostCSS normalization.
//
// Strategy: we don't run upstream's Jest harness directly. Instead
// we extract the test content (HTML class strings), feed it as
// candidates to both compilers, normalize, diff. The expected
// `.test.css` files become a sanity check — if BOTH oracle and Galeforce
// match the expectation, our implementation is faithful.
//
// Each test lives in this file as a single `it(...)`. New tests get
// added by extracting any upstream test file's content + config and
// pasting in.
//
// **Why we only run a curated subset**: most upstream tests exercise
// Tailwind-internal features (plugin context internals, source-map
// machinery, custom transformers). The four we picked are the ones
// designed as "compile the world" smoke tests.

import { describe, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { compileWithTailwind3 } from '@cx/galeforcecss-oracle'
import { compile } from '@cx/galeforcecss'
import { normalizeCss, diffStylesheets, formatDiff } from './index.js'

const here = dirname(fileURLToPath(import.meta.url))
// here is packages/galeforcecss-conformance/src; vendor is four up.
const vendorTests = resolve(here, '../../../vendor/tailwindcss-v3/tests')

/**
 * Extract every class name appearing in `class="…"` or
 * `className={"…"}` attributes from a chunk of HTML. Mirrors what
 * Tailwind's own scanner would extract — same class strings, just
 * pulled with a regex instead of via Tailwind's defaultExtractor.
 */
function extractClasses(html: string): string[] {
  const out = new Set<string>()
  // Match class="..." and className="..."  with single OR double quotes
  const re = /class(?:Name)?\s*=\s*["']([^"']*)["']/g
  let m: RegExpExecArray | null
  while ((m = re.exec(html)) !== null) {
    for (const tok of m[1]!.split(/\s+/)) {
      if (tok.length > 0) out.add(tok)
    }
  }
  return [...out]
}

/**
 * Extract the first `html\`…\`` template literal body from a JS test
 * file. The upstream tests use this pattern for inline content:
 *   `let config = { content: [{ raw: html\`<div class="…"></div>\` }] }`
 */
function extractHtmlTemplate(jsSource: string): string | null {
  const start = jsSource.indexOf('html`')
  if (start < 0) return null
  // Find the matching backtick. Template literals can nest `${...}`
  // expressions but the upstream tests never do — pure strings only.
  const bodyStart = start + 5
  const end = jsSource.indexOf('`', bodyStart)
  if (end < 0) return null
  return jsSource.slice(bodyStart, end)
}

async function runBoth(opts: {
  candidates: string[]
  inputCss?: string
  config?: Record<string, unknown>
}): Promise<{ oracle: string; galeforce: string }> {
  const inputCss = opts.inputCss ?? '@tailwind utilities;\n'
  const config = { corePlugins: { preflight: false }, ...(opts.config ?? {}) }
  const oracleResult = await compileWithTailwind3({
    candidates: opts.candidates,
    inputCss,
    config,
  })
  const galeforceResult = await compile({
    candidates: opts.candidates,
    inputCss,
    config,
  })
  return { oracle: oracleResult.css, galeforce: galeforceResult.css }
}

function assertNoDiff(label: string, oracle: string, galeforce: string): void {
  const o = normalizeCss(oracle)
  const b = normalizeCss(galeforce)
  const diff = diffStylesheets(b, o, {})
  if (diff.length > 0) {
    throw new Error(
      `[${label}] Galeforce diverged from oracle on upstream test corpus:\n${formatDiff(diff)}`,
    )
  }
}

function loadCompanionHtml(testName: string): string {
  return readFileSync(resolve(vendorTests, `${testName}.test.html`), 'utf8')
}

function loadInlineContent(testName: string): string {
  const js = readFileSync(resolve(vendorTests, `${testName}.test.js`), 'utf8')
  const html = extractHtmlTemplate(js)
  if (html === null) {
    throw new Error(`couldn't extract inline html\`…\` block from ${testName}.test.js`)
  }
  return html
}

describe('upstream Tailwind 3 comprehensive tests', () => {
  it('arbitrary-values — companion .test.html corpus', async () => {
    const html = loadCompanionHtml('arbitrary-values')
    const candidates = extractClasses(html)
    const { oracle, galeforce } = await runBoth({ candidates })
    assertNoDiff('arbitrary-values', oracle, galeforce)
  })

  it('variants — companion .test.html corpus', async () => {
    const html = loadCompanionHtml('variants')
    const candidates = extractClasses(html)
    const { oracle, galeforce } = await runBoth({ candidates })
    assertNoDiff('variants', oracle, galeforce)
  })

  it('basic-usage — inline html\\`…\\` corpus from .test.js', async () => {
    const html = loadInlineContent('basic-usage')
    const candidates = extractClasses(html)
    const { oracle, galeforce } = await runBoth({ candidates })
    assertNoDiff('basic-usage', oracle, galeforce)
  })

  it('raw-content — inline html\\`…\\` corpus from .test.js', async () => {
    const html = loadInlineContent('raw-content')
    const candidates = extractClasses(html)
    const { oracle, galeforce } = await runBoth({ candidates })
    assertNoDiff('raw-content', oracle, galeforce)
  })
})
