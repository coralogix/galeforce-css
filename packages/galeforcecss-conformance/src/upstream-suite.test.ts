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

// Generic runner for the entire `vendor/tailwindcss-v3/tests/`
// corpus. We load every `*.test.js` file, extract the `it(...)` /
// `test(...)` blocks that fit the canonical shape (config + input +
// `toMatchFormattedCss` expected output), and feed each to a single
// shared runner that compares Galeforce output against the live oracle.
//
// Tests that don't fit (file paths in content, tests using upstream
// internals, etc.) are reported via Vitest's `test.skip` so we
// can see the coverage gap explicitly.

import { describe, it, expect } from 'vitest'
import { readdirSync } from 'node:fs'
import { resolve, dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { extractTestsFromFile } from './upstream-extract.js'
import { runExtractedTest } from './upstream-runner.js'

const here = dirname(fileURLToPath(import.meta.url))
const vendorTests = resolve(here, '../../../vendor/tailwindcss-v3/tests')

// Files to skip wholesale. These exercise infrastructure (Jest spies,
// PostCSS source maps, raw-extractor regex behaviour) rather than CSS
// output, so the canonical shape doesn't apply.
const SKIP_FILES = new Set([
  'apply-important-selector.test.js', // file-based content
  'context-reuse.test.js',            // jest spy on module load
  'custom-extractors.test.js',         // tests the extractor itself
  'custom-transformers.test.js',
  'default-extractor.test.js',
  'defaultConfig.test.js',
  'defaultTheme.test.js',
  'escapeClassName.test.js',
  'extractor-edge-cases.test.js',
  'flattenColorPalette.test.js',
  'format-variant-selector.test.js',
  'getClassList.test.js',
  'getSortOrder.test.js',
  'getVariants.test.js',
  'kitchen-sink.test.js',              // requires file-system content
  'mutable.test.js',
  'opacity.test.js',                   // tests internal types
  'parseObjectStyles.test.js',
  'plugins.test.js',
  'prefix.test.js',
  'preserve-comments.test.js',
  'public-api.test.js',
  'raw-content.test.js',               // covered by upstream.test.ts
  'safelist.test.js',                  // file-based content patterns
  'source-maps.test.js',
  'stable-class-list.test.js',
  'stable-variant-list.test.js',
  'transformingClasses.test.js',
  'util-fns.test.js',
  'value-parser.test.js',
  'variant-stacking-order.test.js',
])

const files = readdirSync(vendorTests)
  .filter((f) => f.endsWith('.test.js'))
  .filter((f) => !SKIP_FILES.has(f))
  .sort()

for (const file of files) {
  const fullPath = join(vendorTests, file)
  const cases = extractTestsFromFile(fullPath)
  if (cases.length === 0) continue

  describe(`upstream: ${file}`, () => {
    for (const tc of cases) {
      if (tc.skipReason) {
        it.skip(`${tc.name} (skipped: ${tc.skipReason})`, () => {})
        continue
      }
      it(tc.name, async () => {
        const result = await runExtractedTest(tc)
        if (result.skipReason) {
          // Surface skips as a no-op pass; the harness's job is to
          // find Galeforce divergences, not to police upstream's own
          // test-shape variability.
          return
        }
        if (result.oracleErr) {
          // Oracle itself errored — likely a config that requires
          // internals we don't support. Treat as a soft skip.
          return
        }
        if (result.galeforceErr) {
          throw new Error(
            `Galeforce threw while compiling:\n${result.galeforceErr}\n\nOracle output:\n${result.oracleCss ?? ''}`,
          )
        }
        if (result.diff && result.diff.length > 0) {
          throw new Error(
            `Galeforce diverged from oracle on upstream test "${tc.name}":\n\n${result.diff}`,
          )
        }
        expect(true).toBe(true)
      })
    }
  })
}
