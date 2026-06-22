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

import { describe, expect, it } from 'vitest'
import { compileWithTailwind3 } from './index.js'

describe('compileWithTailwind3', () => {
  it('emits a flex utility for the `flex` candidate', async () => {
    const { css } = await compileWithTailwind3({
      candidates: ['flex'],
      inputCss: '@tailwind utilities;',
      config: { corePlugins: { preflight: false } },
    })
    expect(css).toMatch(/\.flex\s*\{[^}]*display:\s*flex/)
  })

  it('handles arbitrary values containing brackets and underscores', async () => {
    const { css } = await compileWithTailwind3({
      candidates: ['grid-cols-[1fr_2fr]'],
      inputCss: '@tailwind utilities;',
      config: { corePlugins: { preflight: false } },
    })
    expect(css).toMatch(/grid-template-columns:\s*1fr\s+2fr/)
  })

  it('honours `darkMode: "class"`', async () => {
    const { css } = await compileWithTailwind3({
      candidates: ['dark:bg-black'],
      inputCss: '@tailwind utilities;',
      config: {
        darkMode: 'class',
        corePlugins: { preflight: false },
      },
    })
    // Tailwind 3.4 emits `.dark\:bg-black:is(.dark *)` for darkMode: 'class'.
    expect(css).toMatch(/\.dark\\:bg-black:is\(\.dark \*\)/)
    expect(css).toMatch(/background-color:\s*rgb\(0 0 0/)
  })

  it('rejects passing both `config` and `configPath`', async () => {
    await expect(
      compileWithTailwind3({
        candidates: ['flex'],
        config: {},
        configPath: 'tailwind.config.js',
      }),
    ).rejects.toThrow(/either `config` or `configPath`/)
  })
})
