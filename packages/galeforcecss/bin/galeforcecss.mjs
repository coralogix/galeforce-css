#!/usr/bin/env node
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

// Launcher for the `galeforcecss` CLI. The binary itself lives in a
// platform-specific optional dependency (npm won't link a bin out of a
// transitive optional dep reliably), so this thin wrapper resolves it with
// the same search order the Node API uses and execs it with our argv.

import { spawnSync } from 'node:child_process'

import { binaryPath } from '../dist/index.js'

const bin = binaryPath()

if (!bin) {
  process.stderr.write(
    'galeforcecss: no prebuilt binary found for ' +
      `${process.platform}-${process.arch}.\n` +
      'Supported: darwin-arm64, darwin-x64, linux-x64, linux-arm64, win32-x64.\n' +
      'Set GALEFORCE_CLI_BIN to a locally built binary to override.\n',
  )
  process.exit(1)
}

const { status, signal, error } = spawnSync(bin, process.argv.slice(2), { stdio: 'inherit' })

if (error) {
  process.stderr.write(`galeforcecss: failed to run ${bin}: ${error.message}\n`)
  process.exit(1)
}

// Mirror the child's termination: a signalled child should not look like a
// clean exit to whatever invoked us (make, CI, npm scripts).
if (signal) process.kill(process.pid, signal)
process.exit(status ?? 1)
