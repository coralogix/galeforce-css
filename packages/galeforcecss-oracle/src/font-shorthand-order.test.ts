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

// Ground-truth check for the cx-web-workspace dialog-title bug.
//
// The user's tailwind config defines a custom `addUtilities` plugin that
// emits `.font-<name> { font: <shorthand> }` utilities. When markup
// applies BOTH a custom `font-section-title` AND a core `font-bold` to
// the same element, the cascade depends on emission order:
//
//   - If `font-bold` (font-weight: 700) is emitted FIRST and the custom
//     `font-section-title` (font shorthand) is emitted LATER, the
//     shorthand wins (font-weight from the shorthand: 600).
//
//   - If reversed, `font-bold` wins (700).
//
// This test pins what upstream tailwindcss@3.4.19 actually does — the
// emission order MUST be `font-bold` first, then `font-section-title`.
// The Rust compiler's matching unit test then asserts the same shape.

import { describe, expect, it } from 'vitest'
import { compileWithTailwind3 } from './index.js'

describe('upstream Tailwind v3 — user-plugin utility vs core (shared prefix)', () => {
  it('emits core font-bold BEFORE a user-plugin .font-* utility', async () => {
    // Mirrors cx-web-workspace's tailwind.config.ts plugin shape.
    const config = {
      corePlugins: { preflight: false },
      plugins: [
        function ({ addUtilities }: { addUtilities: (utils: Record<string, Record<string, string>>) => void }) {
          addUtilities({
            '.font-section-title': {
              font: 'normal normal 600 16px/1.5 sans-serif',
            },
          })
        },
      ],
    }
    const { css } = await compileWithTailwind3({
      candidates: ['font-section-title', 'font-bold'],
      inputCss: '@tailwind utilities;',
      config,
    })
    const boldIdx = css.indexOf('.font-bold')
    const sectionIdx = css.indexOf('.font-section-title')
    expect(boldIdx).toBeGreaterThanOrEqual(0)
    expect(sectionIdx).toBeGreaterThanOrEqual(0)
    expect(boldIdx).toBeLessThan(sectionIdx)
  })
})
