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
import { Button } from './Button'

export function ContactForm() {
  const [submitting, setSubmitting] = useState(false)
  return (
    <form className="space-y-6" onSubmit={async (e) => { e.preventDefault(); setSubmitting(true); await new Promise(r => setTimeout(r, 800)); setSubmitting(false) }}>
      <div className="grid grid-cols-1 gap-x-6 gap-y-4 sm:grid-cols-2">
        <div>
          <label htmlFor="first" className="block text-sm font-medium leading-6 text-gray-900 dark:text-gray-200">
            First name
          </label>
          <div className="mt-2">
            <input
              id="first"
              name="first"
              type="text"
              autoComplete="given-name"
              required
              className="block w-full rounded-md border-0 px-3 py-1.5 text-gray-900 shadow-sm ring-1 ring-inset ring-gray-300 placeholder:text-gray-400 focus:ring-2 focus:ring-inset focus:ring-indigo-600 invalid:ring-red-500 disabled:bg-gray-50 dark:bg-gray-800 dark:text-gray-100 dark:ring-gray-700 dark:focus:ring-indigo-500 sm:text-sm sm:leading-6"
            />
          </div>
        </div>
        <div>
          <label htmlFor="last" className="block text-sm font-medium leading-6 text-gray-900 dark:text-gray-200">
            Last name
          </label>
          <div className="mt-2">
            <input
              id="last"
              name="last"
              type="text"
              autoComplete="family-name"
              className="block w-full rounded-md border-0 px-3 py-1.5 text-gray-900 shadow-sm ring-1 ring-inset ring-gray-300 placeholder:text-gray-400 focus:ring-2 focus:ring-inset focus:ring-indigo-600 dark:bg-gray-800 dark:text-gray-100 dark:ring-gray-700 dark:focus:ring-indigo-500 sm:text-sm sm:leading-6"
            />
          </div>
        </div>
      </div>
      <div>
        <label htmlFor="message" className="block text-sm font-medium leading-6 text-gray-900 dark:text-gray-200">
          Message
        </label>
        <div className="mt-2">
          <textarea
            id="message"
            name="message"
            rows={4}
            className="block w-full rounded-md border-0 px-3 py-1.5 text-gray-900 shadow-sm ring-1 ring-inset ring-gray-300 placeholder:text-gray-400 focus:ring-2 focus:ring-inset focus:ring-indigo-600 dark:bg-gray-800 dark:text-gray-100 dark:ring-gray-700 dark:focus:ring-indigo-500 sm:text-sm sm:leading-6"
            placeholder="Tell us what's on your mind..."
          />
        </div>
        <p className="mt-2 text-xs text-gray-500 dark:text-gray-400">
          Max 500 characters. We'll never share your details.
        </p>
      </div>
      <fieldset>
        <legend className="text-sm font-medium leading-6 text-gray-900 dark:text-gray-200">Subscribe to</legend>
        <div className="mt-3 space-y-2">
          <label className="flex items-center gap-3">
            <input type="checkbox" className="size-4 rounded border-gray-300 text-indigo-600 focus:ring-indigo-600 dark:border-gray-700 dark:bg-gray-800" defaultChecked />
            <span className="text-sm text-gray-700 dark:text-gray-300">Release announcements</span>
          </label>
          <label className="flex items-center gap-3">
            <input type="checkbox" className="size-4 rounded border-gray-300 text-indigo-600 focus:ring-indigo-600 dark:border-gray-700 dark:bg-gray-800" />
            <span className="text-sm text-gray-700 dark:text-gray-300">Weekly digest</span>
          </label>
        </div>
      </fieldset>
      <div className="flex items-center justify-end gap-3 border-t border-gray-200 pt-4 dark:border-gray-800">
        <Button variant="ghost" type="button">Cancel</Button>
        <Button variant="primary" type="submit" loading={submitting}>
          {submitting ? 'Sending...' : 'Send message'}
        </Button>
      </div>
    </form>
  )
}
