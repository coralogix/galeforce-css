// Extractor for upstream `tailwindcss@3.4.19` test files.
//
// Each test file in `vendor/tailwindcss-v3/tests/*.test.js` is a Jest
// suite of small `it(...)` / `test(...)` blocks. Most blocks share a
// canonical shape:
//
//   it('description', () => {
//     let config = { content: [{ raw: html`<div class="…">…</div>` }], … }
//     let input = css`@tailwind utilities;`
//     return run(input, config).then((result) => {
//       expect(result.css).toMatchFormattedCss(css`…`)
//     })
//   })
//
// We pull the `config` object literal, the `input` template literal,
// and the expected output template literal out of each block and feed
// them to a shared runner that compiles via the oracle AND via Galeforce
// then diffs. Tests that don't fit the canonical shape (use file
// paths, snapshot matchers, plugin imports from upstream internals,
// etc.) are flagged `unsupported` and skipped.
//
// We use simple bracket-depth-aware string scanning rather than a
// JS parser. The patterns are regular enough that this is robust for
// the >80% of tests we care about; the rest get skipped with a
// clear reason.
//
// Anything that lives inside the config OBJECT literal is evaluated
// in a tiny sandbox that provides `html`, `css`, `__dirname`, and
// `path` (best-effort). Function values inside the config (e.g.
// theme extenders, plugins) are preserved as JS source and reified
// at run time via `new Function`.

import { readFileSync } from 'node:fs'

export interface ExtractedTest {
  /** Test name from `it(NAME, ...)` / `test(NAME, ...)`. */
  name: string
  /** Source code of the `let config = …` object literal. */
  configSource: string
  /** Decoded input CSS (`css\`…\``), or the canonical default. */
  inputCss: string
  /** Decoded expected CSS for the `toMatchFormattedCss` assertion. */
  expectedCss: string | null
  /** When set, this test was skipped — string explains why. */
  skipReason?: string
}

interface RawBlock {
  name: string
  body: string
}

/**
 * Pull every top-level `it(...)` / `test(...)` body out of a JS source
 * string. Returns each as `{ name, body }`. Top-level here means at
 * file scope OR inside a top-level `describe(...)` (we recurse one
 * level). Anything deeper is left for callers to handle.
 */
function findItBlocks(source: string): RawBlock[] {
  const out: RawBlock[] = []
  const len = source.length
  let i = 0
  while (i < len) {
    // Skip strings, comments, regexes — anything that could spuriously
    // contain `it(` text.
    const skip = skipNoise(source, i)
    if (skip > i) {
      i = skip
      continue
    }
    // Match `it(`, `test(`, or `describe(` followed by a string lit.
    const start = i
    const match = matchAt(source, i, ['it', 'test', 'describe'])
    if (match) {
      const after = match.end
      // `(` may be preceded by whitespace.
      let j = after
      while (j < len && /\s/.test(source[j]!)) j++
      if (source[j] === '(') {
        // Parse argument list: a string-lit name, `,`, then a callback.
        const argsStart = j + 1
        const args = parseCallArguments(source, argsStart)
        if (args && args.length >= 2 && typeof args[0]!.value === 'string') {
          const name = args[0]!.value
          // For `it`/`test`, the callback's body is what we want. Find
          // the function body inside the callback source.
          if (match.kind === 'describe') {
            // Recurse into the describe body so nested it()s are
            // collected with their describe name prefixed.
            const cb = args[1]!.source
            const inner = findItBlocks(cb)
            for (const child of inner) {
              out.push({ name: `${name} > ${child.name}`, body: child.body })
            }
          } else {
            const cb = args[1]!.source
            const body = extractFunctionBody(cb)
            if (body !== null) {
              out.push({ name, body })
            }
          }
          i = args.endIndex
          continue
        }
      }
      // No paren — fall through.
      i = start + 1
      continue
    }
    i++
  }
  return out
}

interface ParsedArg {
  source: string
  /** Decoded JS string for primitive string-literal arguments; `null` otherwise. */
  value: string | null
}

/**
 * Parse comma-separated arguments starting at `start` (which is one
 * past the opening `(`). Returns `null` if a top-level `)` isn't
 * found. The returned `endIndex` is one past the closing `)`.
 */
function parseCallArguments(
  source: string,
  start: number,
): { length: number; [k: number]: ParsedArg; endIndex: number } | null {
  const args: ParsedArg[] = []
  let depth = 1
  let i = start
  let argStart = start
  const len = source.length
  while (i < len) {
    const skip = skipNoise(source, i)
    if (skip > i) {
      i = skip
      continue
    }
    const ch = source[i]!
    if (ch === '(' || ch === '[' || ch === '{') {
      depth++
      i++
      continue
    }
    if (ch === ')' || ch === ']' || ch === '}') {
      depth--
      if (depth === 0 && ch === ')') {
        // End of arg list.
        const argSource = source.slice(argStart, i).trim()
        if (argSource.length > 0 || args.length > 0) {
          args.push({ source: argSource, value: tryDecodeStringLiteral(argSource) })
        }
        return Object.assign(args, { length: args.length, endIndex: i + 1 })
      }
      i++
      continue
    }
    if (ch === ',' && depth === 1) {
      const argSource = source.slice(argStart, i).trim()
      args.push({ source: argSource, value: tryDecodeStringLiteral(argSource) })
      argStart = i + 1
      i++
      continue
    }
    i++
  }
  return null
}

/**
 * If `source` is a single-, double-, or backtick-quoted string literal,
 * return the decoded value. Returns `null` for any other shape.
 */
function tryDecodeStringLiteral(source: string): string | null {
  const trimmed = source.trim()
  if (trimmed.length < 2) return null
  const first = trimmed[0]
  if (first !== "'" && first !== '"' && first !== '`') return null
  const last = trimmed[trimmed.length - 1]
  if (last !== first) return null
  const inner = trimmed.slice(1, -1)
  // Minimal escape handling — the test names don't contain anything
  // exotic.
  return inner
    .replace(/\\n/g, '\n')
    .replace(/\\t/g, '\t')
    .replace(/\\'/g, "'")
    .replace(/\\"/g, '"')
    .replace(/\\\\/g, '\\')
}

/**
 * Skip over JS noise (strings, template literals, comments) starting
 * at `i`. Returns the new position; if `i` isn't at noise, returns
 * `i` unchanged.
 */
function skipNoise(source: string, i: number): number {
  const len = source.length
  if (i >= len) return i
  const ch = source[i]!
  // Line comment
  if (ch === '/' && source[i + 1] === '/') {
    let j = i + 2
    while (j < len && source[j] !== '\n') j++
    return j
  }
  // Block comment
  if (ch === '/' && source[i + 1] === '*') {
    let j = i + 2
    while (j < len && !(source[j] === '*' && source[j + 1] === '/')) j++
    return Math.min(len, j + 2)
  }
  // Quoted strings
  if (ch === '"' || ch === "'") {
    let j = i + 1
    while (j < len) {
      if (source[j] === '\\') {
        j += 2
        continue
      }
      if (source[j] === ch) {
        return j + 1
      }
      j++
    }
    return j
  }
  // Template literal
  if (ch === '`') {
    let j = i + 1
    while (j < len) {
      if (source[j] === '\\') {
        j += 2
        continue
      }
      if (source[j] === '$' && source[j + 1] === '{') {
        // Skip ${...} expression
        let depth = 1
        j += 2
        while (j < len && depth > 0) {
          const sk = skipNoise(source, j)
          if (sk > j) {
            j = sk
            continue
          }
          if (source[j] === '{') depth++
          else if (source[j] === '}') depth--
          j++
        }
        continue
      }
      if (source[j] === '`') {
        return j + 1
      }
      j++
    }
    return j
  }
  return i
}

/**
 * Match an identifier from `idents` at position `i` of `source`. The
 * identifier must be at a token boundary (preceded by a non-identifier
 * char or start of input).
 */
function matchAt(
  source: string,
  i: number,
  idents: string[],
): { kind: string; end: number } | null {
  if (i > 0) {
    const prev = source[i - 1]!
    if (/[A-Za-z0-9_$.]/.test(prev)) return null
  }
  for (const ident of idents) {
    if (source.startsWith(ident, i)) {
      const after = source[i + ident.length]
      if (after === undefined || !/[A-Za-z0-9_$]/.test(after)) {
        return { kind: ident, end: i + ident.length }
      }
    }
  }
  return null
}

/**
 * Given a function source like `() => { … }` or `async () => …` or
 * `function foo() { … }`, return the body text (excluding the
 * enclosing braces for block forms; for expression-bodied arrow
 * functions return the expression source). `null` if the shape isn't
 * recognised.
 */
function extractFunctionBody(source: string): string | null {
  const trimmed = source.trim()
  // Find the opening `{` of the function body, accounting for arrow
  // functions and the optional async prefix.
  // Skip parameter list: walk past the first `(...)` we see.
  let i = 0
  if (trimmed.startsWith('async')) i = 5
  while (i < trimmed.length && /\s/.test(trimmed[i]!)) i++
  if (trimmed.startsWith('function', i)) {
    i += 'function'.length
    while (i < trimmed.length && /\s/.test(trimmed[i]!)) i++
    if (trimmed[i] !== '(') {
      // Skip the function name.
      while (i < trimmed.length && /[A-Za-z0-9_$]/.test(trimmed[i]!)) i++
      while (i < trimmed.length && /\s/.test(trimmed[i]!)) i++
    }
  }
  if (trimmed[i] === '(') {
    // Skip parameter list.
    let depth = 1
    i++
    while (i < trimmed.length && depth > 0) {
      const sk = skipNoise(trimmed, i)
      if (sk > i) {
        i = sk
        continue
      }
      if (trimmed[i] === '(') depth++
      else if (trimmed[i] === ')') depth--
      i++
    }
  }
  while (i < trimmed.length && /\s/.test(trimmed[i]!)) i++
  // Arrow `=>`.
  if (trimmed.startsWith('=>', i)) {
    i += 2
    while (i < trimmed.length && /\s/.test(trimmed[i]!)) i++
  }
  if (trimmed[i] !== '{') {
    // Expression-bodied arrow.
    return trimmed.slice(i).trim()
  }
  // Block body: capture interior.
  let depth = 1
  let j = i + 1
  while (j < trimmed.length && depth > 0) {
    const sk = skipNoise(trimmed, j)
    if (sk > j) {
      j = sk
      continue
    }
    if (trimmed[j] === '{') depth++
    else if (trimmed[j] === '}') depth--
    if (depth === 0) {
      return trimmed.slice(i + 1, j)
    }
    j++
  }
  return null
}

/**
 * Find the first `let X = …` / `const X = …` declaration with the
 * given identifier and return the source of the value expression.
 */
function findVarValue(body: string, name: string): string | null {
  const re = new RegExp(`(?:^|\\W)(?:let|const|var)\\s+${name}\\s*=\\s*`)
  const match = re.exec(body)
  if (!match) return null
  const start = match.index + match[0].length
  // Capture until the next top-level `;` or end-of-line that ends a
  // statement. We do this by walking with bracket-depth awareness.
  let i = start
  let depth = 0
  while (i < body.length) {
    const sk = skipNoise(body, i)
    if (sk > i) {
      i = sk
      continue
    }
    const ch = body[i]!
    if (ch === '(' || ch === '[' || ch === '{') depth++
    else if (ch === ')' || ch === ']' || ch === '}') {
      if (depth === 0) break
      depth--
    } else if ((ch === ';' || ch === '\n') && depth === 0) {
      break
    }
    i++
  }
  return body.slice(start, i).trim()
}

/**
 * Find the first call `expect(…).toMatchFormattedCss(<expectedExpr>)`
 * inside `body` and return the source of `expectedExpr`. Returns
 * `null` if no such call is present.
 */
function findExpectedCss(body: string): string | null {
  const re = /\.toMatchFormattedCss\s*\(/g
  let match: RegExpExecArray | null
  while ((match = re.exec(body)) !== null) {
    const argsStart = match.index + match[0].length
    const args = parseCallArguments(body, argsStart)
    if (args && args.length >= 1) {
      return args[0]!.source
    }
  }
  return null
}

/**
 * Find the first `run(<input>, <config>)` call in `body` and return
 * the parsed positional arguments. Returns `null` if no such call is
 * present. Used as a fallback when `let config` / `let input`
 * aren't top-level — e.g. the upstream `negative-prefix.test.js`
 * shape `run('@tailwind utilities', { ...inline... })`.
 */
function findInlineRunCall(body: string): { inputSource: string; configSource: string } | null {
  const re = /(?:^|\W)run\s*\(/g
  let match: RegExpExecArray | null
  while ((match = re.exec(body)) !== null) {
    const argsStart = match.index + match[0].length
    const args = parseCallArguments(body, argsStart)
    if (args && args.length >= 2) {
      return {
        inputSource: args[0]!.source,
        configSource: args[1]!.source,
      }
    }
  }
  return null
}

/**
 * Decode a `css\`…\`` / `html\`…\`` template literal expression to
 * its underlying string. Supports interpolations of variables we've
 * already resolved (passed via `interpolations`).
 */
export function decodeTaggedTemplate(
  expr: string,
  interpolations: Record<string, string>,
): string | null {
  const trimmed = expr.trim()
  // Match leading tag name (css/html/javascript/anything) optionally,
  // then a backtick.
  const tagMatch = /^([A-Za-z_][A-Za-z0-9_]*)?\s*`/.exec(trimmed)
  if (!tagMatch) return null
  let i = tagMatch[0].length
  let result = ''
  while (i < trimmed.length) {
    const ch = trimmed[i]!
    if (ch === '\\' && i + 1 < trimmed.length) {
      const next = trimmed[i + 1]!
      // Handle escape sequences. Double-backslashes pass through
      // (we want the literal content); the only escape we need to
      // resolve is the backtick itself.
      if (next === '`') {
        result += '`'
      } else {
        result += ch + next
      }
      i += 2
      continue
    }
    if (ch === '`') {
      return result
    }
    if (ch === '$' && trimmed[i + 1] === '{') {
      // Find matching `}`.
      let depth = 1
      let j = i + 2
      while (j < trimmed.length && depth > 0) {
        const sk = skipNoise(trimmed, j)
        if (sk > j) {
          j = sk
          continue
        }
        if (trimmed[j] === '{') depth++
        else if (trimmed[j] === '}') depth--
        if (depth === 0) break
        j++
      }
      const inner = trimmed.slice(i + 2, j).trim()
      // Resolve interpolation. Supports identifiers we've stashed.
      if (Object.prototype.hasOwnProperty.call(interpolations, inner)) {
        result += interpolations[inner]
      } else {
        // Fallback: empty string. Most upstream tests use
        // `${defaults}` which we'll capture explicitly.
        result += ''
      }
      i = j + 1
      continue
    }
    result += ch
    i++
  }
  return null
}

export function extractTestsFromFile(filePath: string): ExtractedTest[] {
  const source = readFileSync(filePath, 'utf8')
  const blocks = findItBlocks(source)
  const out: ExtractedTest[] = []
  for (const block of blocks) {
    let configSource = findVarValue(block.body, 'config')
    let inputSource = findVarValue(block.body, 'input')
    // Fallback: `run(input, { ...inline-config })` shape used by
    // ~25% of upstream tests. The first arg may be an inline string
    // / template literal OR a reference to `let input = ...` we
    // already pulled. The second arg is the inline config object.
    if (!configSource || !inputSource) {
      const inline = findInlineRunCall(block.body)
      if (inline) {
        if (!configSource) configSource = inline.configSource
        if (!inputSource) inputSource = inline.inputSource
      }
    }
    if (!configSource) {
      out.push({
        name: block.name,
        configSource: '',
        inputCss: '',
        expectedCss: null,
        skipReason: 'no config (neither `let config = …` nor `run(…, { … })`)',
      })
      continue
    }
    const interpolations: Record<string, string> = {}
    // Input source can be:
    //   - a tagged template (`css\`…\``) — decoded normally,
    //   - a bare string literal (`'@tailwind utilities'`) — decoded
    //     via the string-literal helper,
    //   - or absent — fall back to the canonical `@tailwind
    //     utilities;` shape.
    let inputCss: string
    if (inputSource === null) {
      inputCss = '@tailwind utilities;\n'
    } else {
      const tpl = decodeTaggedTemplate(inputSource, interpolations)
      if (tpl !== null) {
        inputCss = tpl
      } else {
        const str = tryDecodeStringLiteral(inputSource)
        inputCss = str ?? '@tailwind utilities;\n'
      }
    }
    // Expected CSS is no longer load-bearing — the runner compares
    // Galeforce's output against the live oracle's output, not against
    // the test's literal `toMatchFormattedCss` block. We keep the
    // field for diagnostics but tests using `.toContain` /
    // `.toMatchCss` / `.toEqual` flow through fine.
    const expectedSource = findExpectedCss(block.body)
    const expectedCss =
      expectedSource !== null ? decodeTaggedTemplate(expectedSource, interpolations) : null
    out.push({
      name: block.name,
      configSource,
      inputCss,
      expectedCss,
    })
  }
  return out
}
