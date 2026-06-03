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
