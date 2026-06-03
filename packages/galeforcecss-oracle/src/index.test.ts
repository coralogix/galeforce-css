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
