// Copyright 2026 Coralogix Ltd.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Minimal Source Map v3 emitter.
//!
//! Generates a Source Map v3 JSON for the compiled CSS output.
//! Tailwind 3's source-map support is also coarse: it maps every
//! generated line back to the line of `@tailwind utilities;` (or
//! similar) in the user's input CSS — there's no per-utility
//! attribution to where the candidate appeared in HTML/JSX. Our MVP
//! goes further than nothing but stays well below "track every
//! candidate's source location":
//!
//!   - `sources`: the user's input CSS file path (relative).
//!   - `sourcesContent`: the input CSS body so the map is
//!     self-contained.
//!   - `mappings`: every generated line maps to line 0, col 0 of
//!     the source. That's the simplest valid VLQ-encoded mappings
//!     string (`AAAA;AAAA;AAAA…`) and lets DevTools display "from
//!     input.css" when a user clicks a rule.
//!
//! Future work: track per-rule source location through the compiler
//! pipeline (would let mappings point at the actual `@tailwind`
//! directive line and at user `@apply` / `@layer` blocks line-
//! accurately).

use serde::Serialize;

/// Build a Source Map v3 JSON pointing the compiled `output_css` back
/// to `source_path` (with `source_content` inlined). Returns the
/// JSON string ready to be base64-encoded for inline embedding.
pub fn build_source_map(output_css: &str, source_path: &str, source_content: &str) -> String {
    let line_count = output_css.lines().count().max(1);
    // Each generated line emits one `AAAA` segment (genCol=0,
    // sourceIdx=0, sourceLine=0, sourceCol=0 — relative to previous
    // segment, but VLQ-encoded zeros stay zero). Line separator is
    // `;`.
    let mappings = if line_count == 1 {
        "AAAA".to_string()
    } else {
        let mut s = String::with_capacity(line_count * 5);
        s.push_str("AAAA");
        for _ in 1..line_count {
            s.push_str(";AAAA");
        }
        s
    };
    let map = SourceMapV3 {
        version: 3,
        sources: vec![source_path.to_string()],
        sources_content: vec![source_content.to_string()],
        names: Vec::new(),
        mappings,
    };
    serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string())
}

#[derive(Serialize)]
struct SourceMapV3 {
    version: u32,
    sources: Vec<String>,
    #[serde(rename = "sourcesContent")]
    sources_content: Vec<String>,
    names: Vec<String>,
    mappings: String,
}

/// Append an inline source-map comment to `css`. Format:
///
/// ```text
/// /*# sourceMappingURL=data:application/json;base64,<…> */
/// ```
///
/// This is the canonical way for browsers / DevTools / source-map
/// consumer crates to find the map. For users who want a separate
/// `.map` file the CLI can write the JSON to disk instead — that's
/// a separate path the build command implements.
pub fn append_inline_source_map(css: &mut String, map_json: &str) {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(map_json);
    css.push_str("\n/*# sourceMappingURL=data:application/json;base64,");
    css.push_str(&encoded);
    css.push_str(" */\n");
}
