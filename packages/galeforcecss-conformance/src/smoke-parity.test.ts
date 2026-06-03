import { describe, it, expect } from 'vitest'
import { compileWithTailwind3 } from '@cx/galeforcecss-oracle'
import { compile } from '@cx/galeforcecss'
import { normalizeCss, diffStylesheets, formatDiff } from './index.js'

/**
 * Real-world smoke test: verify GaleforceCSS content scanning produces CSS for the actual smoke project.
 * Uses Galeforce's native content scanning to verify it works on real files.
 */

describe('Real-world smoke test', () => {
  it('scans and compiles the smoke project', async () => {
    const inputCss = '@tailwind utilities;\n'
    const smokeRoot = new URL('../../../smoke/src', import.meta.url).pathname

    // Use Galeforce's content scanning on real project files
    const galeforceResult = await compile({
      inputCss,
      config: { corePlugins: { preflight: false } },
      content: [smokeRoot],
    })

    const normalizedGaleforce = normalizeCss(galeforceResult.css)

    // Verify Galeforce scanned the project and produced substantial output
    console.log(`\nSmoke test results:`)
    console.log(`  Candidates found: ${galeforceResult.candidateCount}`)
    console.log(`  CSS size: ${galeforceResult.css.length} bytes`)
    console.log(`  Rules generated: ${normalizedGaleforce.rules.length}`)

    // Should find candidates in real project files
    expect(galeforceResult.candidateCount).toBeGreaterThan(100)

    // Should generate substantial CSS from those candidates
    expect(galeforceResult.css.length).toBeGreaterThan(10000)

    // Should have multiple rules
    expect(normalizedGaleforce.rules.length).toBeGreaterThan(200)
  })
})
