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

import { describe, it, expect, afterAll } from 'vitest'
import { compile, createCompileStream } from './index.js'

describe('compile (one-shot)', () => {
  it('compiles a single static utility', async () => {
    const r = await compile({ candidates: ['flex'] })
    expect(r.css).toMatch(/\.flex\s*\{[^}]*display:\s*flex/)
    expect(r.candidateCount).toBe(1)
    expect(r.ruleCount).toBe(1)
    expect(r.diagnostics).toEqual([])
  })

  it('respects inputCss passthrough', async () => {
    const r = await compile({
      candidates: ['flex'],
      inputCss: 'body { color: red }\n@tailwind utilities;\n.custom { padding: 1rem }',
    })
    expect(r.css).toMatch(/body\s*\{\s*color:\s*red\s*;?\s*\}/)
    expect(r.css).toContain('.flex')
    expect(r.css).toMatch(/\.custom\s*\{\s*padding:\s*1rem\s*;?\s*\}/)
  })

  it('surfaces unknown-utility diagnostics', async () => {
    const r = await compile({ candidates: ['definitely-not-real'] })
    expect(r.candidateCount).toBe(0)
    expect(r.diagnostics).toHaveLength(1)
    const first = r.diagnostics[0]
    expect(first?.code).toBe('unknown-utility')
  })
})

describe('createCompileStream (long-running)', () => {
  const stream = createCompileStream()
  afterAll(() => stream.close())

  it('returns one response per request, in order', async () => {
    const a = await stream.compile({ candidates: ['flex'] })
    const b = await stream.compile({ candidates: ['hidden'] })
    expect(a.css).toMatch(/\.flex/)
    expect(a.css).not.toMatch(/\.hidden/)
    expect(b.css).toMatch(/\.hidden/)
    expect(b.css).not.toMatch(/\.flex/)
  })

  it('preserves order under concurrent submission', async () => {
    const results = await Promise.all([
      stream.compile({ candidates: ['flex'] }),
      stream.compile({ candidates: ['block'] }),
      stream.compile({ candidates: ['hidden'] }),
    ])
    expect(results[0]?.css).toMatch(/\.flex/)
    expect(results[1]?.css).toMatch(/\.block/)
    expect(results[2]?.css).toMatch(/\.hidden/)
  })
})

describe('createCompileStream child recovery', () => {
  // The Rust child can die mid-session for environment reasons we don't
  // control — panic, OOM-kill, SIGPIPE when stdout backpressure hits a
  // killed reader. Until 0.1.2 that put the stream into a permanent
  // "compile-stream is closed" state for the rest of the dev session
  // (every subsequent compile rejected with no diagnostic context),
  // which we saw take down Playwright integration suites running
  // against a single Vite dev server. The stream now respawns on child
  // exit so callers see one bad compile, not a dead stream.

  async function killStreamChild(): Promise<boolean> {
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const { exec } = require('node:child_process') as typeof import('node:child_process')
    const out = await new Promise<string>((resolve) => {
      exec(`pgrep -P ${process.pid} -f compile-stream`, (_e: unknown, stdout: string) =>
        // pgrep exits 1 when no match — treat as empty rather than failure
        resolve(stdout ?? ''),
      )
    })
    const pid = parseInt(out.trim().split('\n')[0] ?? '', 10)
    if (!Number.isFinite(pid)) return false
    process.kill(pid, 'SIGKILL')
    // Let Node's exit handler run before the next request submits.
    await new Promise((r) => setTimeout(r, 50))
    return true
  }

  it('respawns the child transparently after exit', async () => {
    const stream = createCompileStream()
    try {
      // First compile — establishes the initial child.
      const a = await stream.compile({ candidates: ['flex'] })
      expect(a.css).toMatch(/\.flex/)

      const killed = await killStreamChild()
      // If pgrep couldn't find the child the test environment can't
      // simulate the failure mode — skip rather than false-positive.
      if (!killed) return

      // The next request must succeed — the stream should have
      // spawned a fresh child rather than reporting "is closed".
      const b = await stream.compile({ candidates: ['hidden'] })
      expect(b.css).toMatch(/\.hidden/)
    } finally {
      await stream.close()
    }
  })

  it('reports "is closed" only after user-initiated close()', async () => {
    const stream = createCompileStream()
    await stream.compile({ candidates: ['flex'] })
    await stream.close()
    await expect(stream.compile({ candidates: ['block'] })).rejects.toThrow(
      /compile-stream is closed/,
    )
  })
})
