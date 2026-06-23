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

import { useState } from 'react'

export function App() {
  const [count, setCount] = useState(0)

  return (
    <main className="min-h-screen flex items-center justify-center p-8">
      <div className="max-w-md w-full bg-white dark:bg-slate-800 rounded-2xl shadow-xl p-8 space-y-6">
        <header className="space-y-2">
          <h1 className="text-3xl font-bold text-slate-900 dark:text-white">
            GaleforceCSS + Vite + React
          </h1>
          <p className="text-slate-600 dark:text-slate-300">
            Drop-in replacement for Tailwind v3. Same config, same output,{' '}
            <span className="font-semibold text-brand-500">23x faster</span> builds.
          </p>
        </header>

        <div className="flex items-center justify-between">
          <span className="text-sm text-slate-500 dark:text-slate-400">Counter</span>
          <button
            onClick={() => setCount((c) => c + 1)}
            className="px-4 py-2 bg-brand-500 hover:bg-brand-900 active:scale-95 transition text-white font-medium rounded-lg shadow-md"
          >
            {count}
          </button>
        </div>

        <p className="text-xs text-slate-400 dark:text-slate-500">
          Edit <code className="px-1 py-0.5 rounded bg-slate-100 dark:bg-slate-700 text-brand-500">src/App.tsx</code>{' '}
          and save to test HMR.
        </p>
      </div>
    </main>
  )
}
