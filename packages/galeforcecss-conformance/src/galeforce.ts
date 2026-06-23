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

// Bridges the conformance harness to the Rust compiler by spawning the
// `galeforcecss compile-json` binary. This is intentionally a process-boundary
// shim, not the eventual integration: napi-rs bindings (Phase F) will
// replace this with an in-process call. The contract — JSON in, JSON out —
// stays the same so tests don't have to change.
//
// We resolve the binary at import time:
// 1. `GALEFORCE_CLI_BIN` env var, if set.
// 2. The cargo target dirs at the workspace root: prefer release if it
//    exists (CI builds it), fall back to debug for local development.
//
// If neither is available we throw `GaleforceNotImplemented` — the harness
// already understands that as "Galeforce isn't ready" and will mark the
// fixture unimplemented rather than failing.

import { spawn } from 'node:child_process'
import { existsSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

export interface GaleforceCompileInput {
  candidates: string[]
  inputCss: string
  config: Record<string, unknown>
  /** Optional feature flags forwarded to the CLI as the `features` field. */
  features?: { nesting?: boolean }
}

export interface GaleforceCompileOutput {
  css: string
}

export class GaleforceNotImplemented extends Error {
  constructor(message = 'GaleforceCSS compiler is not implemented yet (pre-alpha).') {
    super(message)
    this.name = 'GaleforceNotImplemented'
  }
}

const here = dirname(fileURLToPath(import.meta.url))
const REPO_ROOT = resolve(here, '../../..')
const RELEASE_BIN = resolve(REPO_ROOT, 'target/release/galeforcecss')
const DEBUG_BIN = resolve(REPO_ROOT, 'target/debug/galeforcecss')

function resolveBinary(): string | null {
  const fromEnv = process.env.GALEFORCE_CLI_BIN
  if (fromEnv && existsSync(fromEnv)) return fromEnv
  if (existsSync(RELEASE_BIN)) return RELEASE_BIN
  if (existsSync(DEBUG_BIN)) return DEBUG_BIN
  return null
}

interface CliResult {
  output: { css: string; map?: string | null }
  diagnostics: Array<{ severity: 'error' | 'warning' | 'info'; code: string; message: string }>
  candidateCount: number
  ruleCount: number
}

export async function compileWithGaleforce(input: GaleforceCompileInput): Promise<GaleforceCompileOutput> {
  const bin = resolveBinary()
  if (bin === null) {
    throw new GaleforceNotImplemented(
      `galeforcecss binary not found. Build it with \`cargo build -p galeforce-cli --release\` or set GALEFORCE_CLI_BIN.`,
    )
  }

  // The CLI accepts a CompileOptions object whose JSON shape comes from
  // galeforce-core. We pass `candidates` and `inputCss`/config straight through;
  // the compiler ignores fields it doesn't yet honor (Phase C: only
  // `candidates` matters).
  const payload = JSON.stringify({
    candidates: input.candidates,
    inputCss: input.inputCss,
    config: input.config,
    features: input.features ? { nesting: input.features.nesting ?? false } : undefined,
  })

  const { stdout, stderr, code } = await runChild(bin, ['compile-json'], payload)
  if (code !== 0) {
    throw new Error(`galeforcecss compile-json exited with ${code}: ${stderr || '(no stderr)'}`)
  }

  let parsed: CliResult
  try {
    parsed = JSON.parse(stdout) as CliResult
  } catch (err) {
    throw new Error(
      `galeforcecss compile-json produced non-JSON output: ${(err as Error).message}\n--- stdout ---\n${stdout}`,
    )
  }

  // Surface error-level diagnostics: they never indicate a fixture pass.
  const errors = parsed.diagnostics.filter((d) => d.severity === 'error')
  if (errors.length > 0) {
    const lines = errors.map((d) => `  [${d.code}] ${d.message}`).join('\n')
    throw new Error(`galeforcecss reported errors:\n${lines}`)
  }

  return { css: parsed.output.css }
}

interface ChildResult {
  stdout: string
  stderr: string
  code: number
}

function runChild(bin: string, args: string[], stdinPayload: string): Promise<ChildResult> {
  return new Promise((resolveProm, rejectProm) => {
    const child = spawn(bin, args, { stdio: ['pipe', 'pipe', 'pipe'] })
    let stdout = ''
    let stderr = ''
    child.stdout.setEncoding('utf8')
    child.stderr.setEncoding('utf8')
    child.stdout.on('data', (chunk: string) => {
      stdout += chunk
    })
    child.stderr.on('data', (chunk: string) => {
      stderr += chunk
    })
    child.on('error', rejectProm)
    child.on('close', (code) => resolveProm({ stdout, stderr, code: code ?? 0 }))
    child.stdin.write(stdinPayload)
    child.stdin.end()
  })
}
