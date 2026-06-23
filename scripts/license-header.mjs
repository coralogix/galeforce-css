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

// Checks (and with `fix`, inserts) the Apache 2.0 license header on every
// first-party source file. Zero dependencies: plain Node, so it "just works"
// after a checkout without installing a separate tool.
//
//   node scripts/license-header.mjs check   # verify (CI gate; exits 1 if any missing)
//   node scripts/license-header.mjs fix     # insert the header where missing
//
// The header text is read from license-header.txt. Comment style is chosen by
// extension (// for Rust, block /* */ for JS/TS). A leading `#!` shebang is
// kept on line 1 and the header inserted beneath it, so CLI entrypoints stay
// executable.

import { execSync } from 'node:child_process'
import { readFileSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, resolve, extname } from 'node:path'

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')

// Substring that marks a header as already present. Kept loose on purpose so a
// pre-existing header (whatever the exact comment rendering) is recognised.
const MARKER = 'Licensed under the Apache License, Version 2.0'

const EXTENSIONS = ['rs', 'ts', 'tsx', 'js', 'mjs', 'cjs']
const EXCLUDE = [/^vendor\//, /\/node_modules\//, /\.d\.ts$/]
const BLOCK_EXTS = new Set(['ts', 'tsx', 'js', 'mjs', 'cjs'])

const headerLines = readFileSync(resolve(repoRoot, 'license-header.txt'), 'utf8')
  .replace(/\n+$/, '')
  .split('\n')

function render(ext) {
  if (BLOCK_EXTS.has(ext)) {
    const body = headerLines.map((l) => (l ? ` * ${l}` : ' *')).join('\n')
    return `/*\n${body}\n */`
  }
  // line-comment style (Rust)
  return headerLines.map((l) => (l ? `// ${l}` : '//')).join('\n')
}

function sourceFiles() {
  const globs = EXTENSIONS.map((e) => `*.${e}`).join(' ')
  const out = execSync(`git ls-files -- ${globs}`, { cwd: repoRoot, encoding: 'utf8' })
  return out
    .split('\n')
    .filter(Boolean)
    .filter((f) => !EXCLUDE.some((re) => re.test(f)))
}

function hasHeader(content) {
  return content.split('\n', 30).join('\n').includes(MARKER)
}

function insert(content, ext) {
  const lines = content.split('\n')
  let shebang = null
  if (lines[0]?.startsWith('#!')) shebang = lines.shift()
  const body = lines.join('\n').replace(/^\n+/, '')
  const head = render(ext)
  const prefix = shebang ? `${shebang}\n${head}` : head
  return `${prefix}\n\n${body}`
}

const mode = process.argv[2]
if (mode !== 'check' && mode !== 'fix') {
  console.error('usage: node scripts/license-header.mjs <check|fix>')
  process.exit(2)
}

const missing = []
for (const file of sourceFiles()) {
  const abs = resolve(repoRoot, file)
  const content = readFileSync(abs, 'utf8')
  if (hasHeader(content)) continue
  if (mode === 'fix') {
    writeFileSync(abs, insert(content, extname(file).slice(1)))
  }
  missing.push(file)
}

if (mode === 'fix') {
  console.log(missing.length ? `Added header to ${missing.length} file(s):` : 'All files already have a header.')
  for (const f of missing) console.log(`  + ${f}`)
  process.exit(0)
}

if (missing.length) {
  console.error(`Missing Apache license header in ${missing.length} file(s):`)
  for (const f of missing) console.error(`  - ${f}`)
  console.error('\nRun `pnpm license:fix` (or `node scripts/license-header.mjs fix`) to add them.')
  process.exit(1)
}
console.log('All first-party source files carry the Apache license header.')
