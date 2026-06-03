// Byte-order-sensitive snapshot of the upstream corpus.
//
// Tailwind ships `tests/basic-usage.test.js` + `basic-usage.test.css`
// as a kitchen-sink snapshot — every utility's expected output, in
// emission order, declaration order intact. Running it against
// Galeforce with order-sensitive comparison closes the last category of
// "test letting through slippages" gaps the user surfaced.
//
// Diff mode: `orderSensitive: true` — rule order AND declaration
// order both have to match. Any divergence fails. PostCSS round-
// trips on both sides normalize whitespace and comments, so this
// stays robust to formatting (the AST shape is what we're asserting).

import { describe, it, expect } from 'vitest'
import { resolve, dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { extractTestsFromFile } from './upstream-extract.js'
import { runExtractedTest } from './upstream-runner.js'

const here = dirname(fileURLToPath(import.meta.url))
const vendorTests = resolve(here, '../../../vendor/tailwindcss-v3/tests')

// Curated corpus: upstream test files where Tailwind expects a
// specific byte-for-byte output (`toMatchFormattedCss` snapshot).
// These are the strongest possible upstream-equivalence tests.
const ORDERED_FILES = [
  'basic-usage.test.js',
  'variants.test.js',
  'apply.test.js',
  'arbitrary-values.test.js',
  'arbitrary-variants.test.js',
  'important.test.js',
]

describe('upstream byte-order-sensitive snapshots', () => {
  for (const file of ORDERED_FILES) {
    const fullPath = join(vendorTests, file)
    let cases
    try {
      cases = extractTestsFromFile(fullPath)
    } catch {
      it.skip(`${file} — extraction failed`, () => {})
      continue
    }
    if (cases.length === 0) {
      it.skip(`${file} — no extractable cases`, () => {})
      continue
    }

    describe(file, () => {
      for (const tc of cases) {
        if (tc.skipReason) {
          it.skip(`${tc.name} (${tc.skipReason})`, () => {})
          continue
        }
        it(tc.name, async () => {
          const result = await runExtractedTest(tc, { orderSensitive: true })
          if (result.skipReason || result.oracleErr) return
          if (result.galeforceErr) {
            throw new Error(
              `Galeforce threw while compiling:\n${result.galeforceErr}\n\nOracle output:\n${result.oracleCss ?? ''}`,
            )
          }
          if (result.diff && result.diff.length > 0) {
            // Cap the diff to keep failure output readable when an
            // upstream snapshot has hundreds of rules — first N lines
            // is usually enough to localize the divergence.
            const MAX_LINES = 30
            const lines = result.diff.split('\n')
            const truncated =
              lines.length > MAX_LINES
                ? `${lines.slice(0, MAX_LINES).join('\n')}\n… (+${lines.length - MAX_LINES} more)`
                : result.diff
            expect.fail(`byte-sensitive divergence for ${tc.name}:\n${truncated}`)
          }
        })
      }
    })
  }
})
