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

/**
 * Escape a Tailwind candidate so it survives placement inside an HTML
 * `class="…"` attribute without altering its byte-level identity from
 * Tailwind's perspective.
 *
 * Tailwind's default extractor matches on a permissive regex, NOT on HTML
 * parsing — so we only need to break the literal `"` that closes the
 * attribute. We deliberately do NOT entity-encode `&`, `<`, `>`, or `[`
 * because doing so would change the candidate Tailwind sees (e.g. `&` would
 * become `&amp;` and the candidate would never match). Browsers would
 * complain, but Tailwind's content scanner only cares about the raw bytes.
 *
 * The one risky character is `"`. We replace any embedded `"` with `&quot;`
 * — Tailwind's class extractor sees the entity boundary and ends the class,
 * which is what we want for any candidate that legitimately contains a `"`
 * (rare, but possible inside arbitrary values such as
 * `content-["hello"]`). For those cases, the caller MUST pre-process the
 * candidate so quote characters use the `\"`-escaped form Tailwind itself
 * generates, or use single quotes inside the bracket: `content-['hello']`.
 */
export function escapeForHtmlAttribute(candidate: string): string {
  // Replace closing-attribute `"` only. Leave everything else byte-identical.
  return candidate.replace(/"/g, '&quot;')
}
