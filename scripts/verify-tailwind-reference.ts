// Verifies that the Tailwind oracle is the version GaleforceCSS targets.
//
// Always checks the installed `tailwindcss` package version against the
// pinned target. If the optional submodule at `vendor/tailwindcss-v3` exists,
// also checks its `package.json` version matches.
//
// Run with: `pnpm oracle:version`

import { existsSync, readFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'

const TARGET = '3.4.19'

const here = dirname(fileURLToPath(import.meta.url))
const repoRoot = resolve(here, '..')

interface Result {
  ok: boolean
  installedVersion: string | null
  submodulePresent: boolean
  submoduleVersion: string | null
  submoduleCommit: string | null
  submoduleDirty: boolean
  errors: string[]
}

function readJson(path: string): Record<string, unknown> {
  return JSON.parse(readFileSync(path, 'utf8')) as Record<string, unknown>
}

function getInstalledVersion(): string | null {
  try {
    const require = createRequire(resolve(repoRoot, 'package.json'))
    const pkg = require('tailwindcss/package.json') as { version: string }
    return pkg.version
  } catch {
    return null
  }
}

function getSubmoduleVersion(): { path: string; version: string | null } {
  const path = resolve(repoRoot, 'vendor/tailwindcss-v3/package.json')
  if (!existsSync(path)) return { path, version: null }
  try {
    const pkg = readJson(path) as { version: string }
    return { path, version: pkg.version }
  } catch {
    return { path, version: null }
  }
}

function getSubmoduleCommit(): string | null {
  try {
    const out = execFileSync('git', ['-C', resolve(repoRoot, 'vendor/tailwindcss-v3'), 'rev-parse', 'HEAD'], {
      stdio: ['ignore', 'pipe', 'ignore'],
    })
    return out.toString().trim()
  } catch {
    return null
  }
}

function check(): Result {
  const errors: string[] = []
  const installedVersion = getInstalledVersion()
  if (installedVersion === null) {
    errors.push(
      'tailwindcss is not installed. Run `pnpm install` at the repo root before checking the oracle version.',
    )
  } else if (installedVersion !== TARGET) {
    errors.push(
      `tailwindcss version drift: installed ${installedVersion}, expected ${TARGET}.\n` +
        `  Pin in package.json or update galeforcecss to track the new release.`,
    )
  }

  const submodulePath = resolve(repoRoot, 'vendor/tailwindcss-v3/package.json')
  const submodulePresent = existsSync(submodulePath)
  let submoduleVersion: string | null = null
  let submoduleCommit: string | null = null
  let submoduleDirty = false

  // CI gates the submodule harder than local: missing or dirty is a fail
  // there, but only a warning locally so a contributor on a fresh clone
  // who hasn't run `git submodule update --init` doesn't get blocked.
  const isCI = process.env.CI === 'true' || process.env.CI === '1'

  if (submodulePresent) {
    submoduleVersion = getSubmoduleVersion().version
    submoduleCommit = getSubmoduleCommit()
    submoduleDirty = isSubmoduleDirty()
    if (submoduleVersion === null) {
      errors.push(`vendor/tailwindcss-v3 is present but its package.json could not be parsed.`)
    } else if (submoduleVersion !== TARGET) {
      errors.push(
        `vendor/tailwindcss-v3 version drift: submodule ${submoduleVersion}, expected ${TARGET}.\n` +
          `  cd vendor/tailwindcss-v3 && git checkout <commit-for-v${TARGET}>`,
      )
    } else if (installedVersion !== null && submoduleVersion !== installedVersion) {
      errors.push(
        `submodule (${submoduleVersion}) and npm (${installedVersion}) disagree — they must match.`,
      )
    }
    if (submoduleDirty && isCI) {
      errors.push(
        `vendor/tailwindcss-v3 has uncommitted changes. The submodule is upstream Tailwind — ` +
          `local edits should never land in CI.`,
      )
    }
  } else if (isCI) {
    errors.push(
      `vendor/tailwindcss-v3 is missing in CI. Did the workflow forget ` +
        `\`actions/checkout@v4\` with \`submodules: recursive\`?`,
    )
  }

  return {
    ok: errors.length === 0,
    installedVersion,
    submodulePresent,
    submoduleVersion,
    submoduleCommit,
    submoduleDirty,
    errors,
  }
}

function isSubmoduleDirty(): boolean {
  try {
    const out = execFileSync(
      'git',
      ['-C', resolve(repoRoot, 'vendor/tailwindcss-v3'), 'status', '--porcelain'],
      { stdio: ['ignore', 'pipe', 'ignore'] },
    )
    return out.toString().trim().length > 0
  } catch {
    // Not a git repo, or git unavailable — treat as clean rather than
    // blocking the verify pass on environment errors.
    return false
  }
}

const result = check()

console.log(`GaleforceCSS oracle target: tailwindcss@${TARGET}`)
console.log(`  npm  installed:   ${result.installedVersion ?? '(not installed)'}`)
if (result.submodulePresent) {
  console.log(`  submodule:        ${result.submoduleVersion ?? '(unreadable)'}`)
  console.log(`  submodule commit: ${result.submoduleCommit ?? '(not a git repo)'}`)
  if (result.submoduleDirty) {
    console.log(`  submodule state:  dirty (uncommitted changes)`)
  }
} else {
  console.log(`  submodule:        (not present — run \`git submodule update --init --recursive\`)`)
}

if (!result.ok) {
  console.error('')
  for (const e of result.errors) console.error(`✗ ${e}`)
  process.exit(1)
}

console.log('\n✓ oracle version OK')
