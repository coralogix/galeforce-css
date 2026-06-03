// Semantic CSS normalization for conformance comparisons.
//
// Goal: collapse two CSS strings into a canonical structure so that
// "Tailwind output" and "Galeforce output" can be compared by *meaning*,
// not by whitespace or comment differences.
//
// We deliberately do NOT use Lightning CSS or any minifier here. Minifiers
// can rewrite values (e.g. shorten colors, drop fallbacks) and that would
// hide real diffs. We use PostCSS's loss-free parser and walk the AST.

import postcss, { type AtRule, type Container, type Declaration, type Rule } from 'postcss'

export interface NormalizedDecl {
  property: string
  value: string
  important: boolean
}

export interface NormalizedRule {
  /** Outermost-first chain of at-rule contexts, e.g. `["@media (min-width: 768px)"]`. */
  context: string[]
  /** Selector list, comma-separated and trimmed. */
  selector: string
  declarations: NormalizedDecl[]
}

export interface NormalizedStylesheet {
  rules: NormalizedRule[]
}

/**
 * Parse a CSS string and produce a normalized rule list.
 *
 * Normalization steps:
 *   - drop all comments
 *   - drop empty rules
 *   - trim whitespace inside selectors and declaration values
 *   - collapse internal runs of whitespace
 *   - keep declarations in source order (some properties — e.g. font shorthand,
 *     custom property cascade — are order-sensitive, so reordering would be
 *     unsafe)
 *   - keep at-rule context as a string chain so nesting is preserved
 */
export function normalizeCss(css: string): NormalizedStylesheet {
  const root = postcss.parse(css)
  const out: NormalizedRule[] = []
  walk(root, [], out)
  return { rules: out }
}

function walk(container: Container, context: string[], out: NormalizedRule[]): void {
  container.each((node) => {
    if (node.type === 'comment') return
    if (node.type === 'atrule') {
      const at = node as AtRule
      const header = formatAtRule(at)
      // Some at-rules are leaves (e.g. @charset, @import, @namespace).
      if (!at.nodes) {
        // Treat as a synthetic rule with empty selector so it appears in the
        // diff but doesn't get lost.
        out.push({ context: [...context], selector: header, declarations: [] })
        return
      }
      walk(at, [...context, header], out)
      return
    }
    if (node.type === 'rule') {
      const rule = node as Rule
      const decls: NormalizedDecl[] = []
      rule.walkDecls((decl: Declaration) => {
        decls.push({
          property: decl.prop.trim(),
          value: collapseWhitespace(decl.value),
          important: decl.important === true,
        })
      })
      if (decls.length === 0) return
      out.push({
        context: [...context],
        selector: normalizeSelector(rule.selector),
        declarations: decls,
      })
    }
  })
}

function formatAtRule(at: AtRule): string {
  const params = collapseWhitespace(at.params).trim()
  return params.length > 0 ? `@${at.name} ${params}` : `@${at.name}`
}

function normalizeSelector(selector: string): string {
  return selector
    .split(',')
    .map((s) => collapseWhitespace(s).trim())
    .filter((s) => s.length > 0)
    .join(', ')
}

function collapseWhitespace(s: string): string {
  return s.replace(/\s+/g, ' ').trim()
}

/** Stable string form of a normalized stylesheet — useful for snapshots and golden tests. */
export function stringifyNormalized(sheet: NormalizedStylesheet): string {
  const lines: string[] = []
  for (const rule of sheet.rules) {
    const indent = '  '.repeat(rule.context.length)
    for (let i = 0; i < rule.context.length; i++) {
      lines.push('  '.repeat(i) + rule.context[i] + ' {')
    }
    lines.push(indent + rule.selector + ' {')
    for (const decl of rule.declarations) {
      const bang = decl.important ? ' !important' : ''
      lines.push(indent + '  ' + decl.property + ': ' + decl.value + bang + ';')
    }
    lines.push(indent + '}')
    for (let i = rule.context.length - 1; i >= 0; i--) {
      lines.push('  '.repeat(i) + '}')
    }
  }
  return lines.join('\n')
}
