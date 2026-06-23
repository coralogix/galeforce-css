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

//! Permissive candidate tokenizer.
//!
//! Tailwind v3's default extractor uses a broad regex that pulls out anything
//! that *could* be a class name; the parser then rejects tokens that aren't
//! valid utilities. We mirror that "extract wide, parse strict" contract so a
//! token like `bg-[url('/foo.svg')]` survives intact (brackets are opaque)
//! while string literals, file paths, and CSS values inside JS get skipped
//! later by the parser rather than here.
//!
//! Token boundaries:
//! - `[` opens a bracketed run that consumes everything up to the matching
//!   `]`, tracking nesting depth and `\`-escapes. Whitespace, quotes, colons
//!   inside brackets are part of the candidate.
//! - Outside brackets, a candidate continues across alphanumeric, `-`, `_`,
//!   `:`, `/`, `!`, `@`. It ends on any other byte.
//! - Candidates start at alphanumeric, `-`, `_`, `!`, `@`, or `[`.
//!
//! The tokenizer is byte-oriented: it does not interpret encodings beyond
//! ASCII because every Tailwind-significant character is ASCII. Multi-byte
//! UTF-8 sequences are passed through transparently when they sit inside
//! a candidate (rare, but legal in arbitrary values).

/// Append every candidate-shaped token from `source` to `out`. Duplicates are
/// preserved — deduplication happens at the cache layer where refcounts
/// matter.
pub fn extract_candidates(source: &str, out: &mut Vec<String>) {
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            // A standalone `[` can mean two things in real source:
            //
            //   1. **Arbitrary variant** — `[&>*]:mt-2`. The matching `]`
            //      is followed by `:`, so the whole run is one candidate
            //      and we hand off to `scan_candidate`.
            //   2. **JS array / TS tuple literal** — `[ 'rounded-3xl p-8' ]`.
            //      The `]` isn't followed by `:`. If we treated this as
            //      a candidate, `consume_bracketed` would swallow every
            //      class string nested inside. Descend instead so the
            //      outer loop tokenizes the inner content normally.
            //
            // The lookahead is cheap: bracket pairs in real codebases are
            // small and well-balanced. Unbalanced `[` falls into the
            // descend path so we don't lose tokens after a stray bracket.
            //
            // **Arbitrary property** (`[appearance:textfield]`) is a third
            // case: the `]` is NOT followed by `:` (no variant attaches),
            // and the inner contains a `:` (which makes it a CSS-decl
            // shape, not an array literal). Tailwind v3 produces a class
            // selector named after the entire bracketed text. We treat
            // these as single candidates so `[contain:strict]`,
            // `[grid-template-columns:repeat(auto-fill,minmax(240px,1fr))]`,
            // etc. survive intact.
            let close = find_matching_bracket_close(bytes, i);
            let is_arbitrary_variant = close.is_some_and(|c| {
                if bytes.get(c + 1) != Some(&b':') {
                    return false;
                }
                // Mirror Tailwind's `defaultExtractor` regex: a
                // bracketed segment of a candidate must not contain
                // whitespace. Without this guard, a TypeScript index
                // signature like `[key: string]:` looks variant-shaped
                // (trailing `:`) and gets swept up as one token.
                let inner_start = i + 1;
                for &b in &bytes[inner_start..c] {
                    if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
                        return false;
                    }
                }
                true
            });
            let is_arbitrary_property = close.is_some_and(|c| {
                // Skip the immediately-arbitrary-variant case.
                if bytes.get(c + 1) == Some(&b':') {
                    return false;
                }
                // Must look like `[<css-prop>:<value>]`. CSS property
                // names are `--<ident>` (custom props) or `[a-zA-Z-]+`
                // — they NEVER contain spaces, quotes, or backslashes.
                // This rules out JSX array literals like
                // `[ 'rounded p-8', 'xl:p-10' ]` whose inner starts
                // with whitespace / quote and DOES contain `:` (from
                // variants), which would otherwise false-positive.
                let inner_start = i + 1;
                if inner_start >= c {
                    return false;
                }
                let first = bytes[inner_start];
                // Allow `--` for CSS custom property names.
                let starts_like_property = first == b'-' || first.is_ascii_alphabetic();
                if !starts_like_property {
                    return false;
                }
                // Mirror Tailwind's `defaultExtractor` regex which
                // refuses whitespace (and backslash-outside-an-escape)
                // anywhere inside a bracketed candidate. This
                // rejects TypeScript index-signatures like
                // `[key: string]` where the space after `:` looks
                // valid to CSS-decl shape-checkers but isn't a
                // utility candidate at all.
                for &b in &bytes[inner_start..c] {
                    if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
                        return false;
                    }
                }
                // Walk to the FIRST top-level `:` and confirm everything
                // between `[` and it is a valid CSS property-ident shape
                // (`[a-zA-Z_-]`, `--` etc.). Anything else (space, quote,
                // digit-first) means it's not arbitrary-property syntax.
                let mut j = inner_start;
                while j < c {
                    match bytes[j] {
                        b':' => {
                            // Found the separator. Reject if we walked
                            // zero chars (empty property name).
                            return j > inner_start;
                        }
                        b if b.is_ascii_alphabetic() || matches!(b, b'-' | b'_') => j += 1,
                        _ => return false,
                    }
                }
                false
            });
            if is_arbitrary_variant || is_arbitrary_property {
                let start = i;
                i = scan_candidate(bytes, i);
                if i > start {
                    out.push(source[start..i].to_string());
                }
            } else {
                // Skip the `[` itself so the next iteration enters the
                // body of the brackets and picks up tokens inside.
                i += 1;
            }
            continue;
        }
        if is_candidate_start(bytes[i]) {
            let start = i;
            i = scan_candidate(bytes, i);
            if i > start {
                // SAFETY: we only advance by ASCII bytes or by `1` over a
                // bracketed run that is itself byte-aligned. Splitting at
                // `start` and `i` therefore lands on UTF-8 boundaries.
                let raw = &source[start..i];
                // Strip a single trailing `:` — it's never part of a
                // candidate (variants use a leading colon-prefix shape
                // like `hover:foo`, not a trailing one), and tokens that
                // end in `:` arise almost exclusively from JS/TS object
                // keys (`filter: { … }`). Tailwind's defaultExtractor
                // does the same: it yields `filter` from `filter: {`,
                // not `filter:`. Without this we'd miss a class the
                // oracle emits via the same source text.
                let trimmed = raw.strip_suffix(':').unwrap_or(raw);
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                    // When the token contains a `<digit>.<digit>`
                    // run, ALSO emit the prefix up to (but not
                    // including) the decimal — Tailwind v3's
                    // `defaultExtractor` does this by returning
                    // overlapping matches. `tw-bottom-2.5` yields
                    // both `tw-bottom-2.5` AND `tw-bottom-2` so
                    // both utility forms (if both exist in the
                    // user's project) get compiled.
                    let tb = trimmed.as_bytes();
                    for idx in 1..tb.len().saturating_sub(1) {
                        if tb[idx] == b'.'
                            && tb[idx - 1].is_ascii_digit()
                            && tb[idx + 1].is_ascii_digit()
                        {
                            out.push(trimmed[..idx].to_string());
                        }
                    }
                }
            }
        } else {
            i += 1;
        }
    }
}

fn is_candidate_start(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'!' | b'@')
}

/// Walk forward from a `[` and return the index of the matching `]`,
/// honoring `\`-escapes and nested `[ … ]`. Returns `None` for
/// unbalanced input.
fn find_matching_bracket_close(bytes: &[u8], open: usize) -> Option<usize> {
    debug_assert_eq!(bytes[open], b'[');
    let mut i = open + 1;
    let mut depth = 1usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'[' => {
                depth += 1;
                i += 1;
            }
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn is_candidate_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'/' | b'!' | b'@')
}

fn scan_candidate(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    let mut had_content = false;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'[' {
            let after = consume_bracketed(bytes, i);
            if after == i {
                // `consume_bracketed` bailed: the bracketed run contained
                // whitespace/quote/newline or never closed before EOF, so
                // it's not a valid Tailwind arbitrary segment (mirrors
                // upstream's `defaultExtractor` regex). End the candidate
                // before the `[` so the outer loop can resume tokenizing
                // from there. Without this guard, an embedded shell escape
                // like `\033[0;31m'` (inside a template-literal string)
                // could swallow the rest of the file.
                break;
            }
            i = after;
            had_content = true;
        } else if b == b'.'
            && i > start
            && bytes[i - 1].is_ascii_digit()
            && i + 1 < bytes.len()
            && bytes[i + 1].is_ascii_digit()
        {
            // Decimal-fraction inside a candidate run: keep the dot
            // only when it sits between two digits (`tw-top-0.5`,
            // `border-2.5`). This avoids eating JS property access
            // (`obj.method`) which would produce noisy tokens —
            // Tailwind v3's tokenizer has the same shape.
            i += 1;
            had_content = true;
        } else if is_candidate_continue(b) {
            i += 1;
            had_content = true;
        } else if b >= 0x80 {
            // Non-ASCII byte: only legal inside an already-bracketed value.
            // Outside brackets, treat as a delimiter — most files don't put
            // U+00A0 etc. inside class names.
            break;
        } else {
            break;
        }
    }
    if had_content {
        i
    } else {
        start
    }
}

fn consume_bracketed(bytes: &[u8], start: usize) -> usize {
    debug_assert_eq!(bytes[start], b'[');
    let mut i = start + 1;
    let mut depth = 1usize;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            // A newline inside a bracketed run is a strong signal we're
            // not inside a Tailwind arbitrary segment — upstream's
            // `defaultExtractor` regex rejects newlines, and class
            // strings in templates are single-line. Without this guard,
            // a shell ANSI escape like `\\033[0;31m'\n...` (inside a
            // TS template literal) walks `consume_bracketed` for KB
            // looking for a matching `]`, potentially to EOF, eating
            // every candidate after it.
            b'\n' | b'\r' => {
                return start;
            }
            b'\\' if i + 1 < bytes.len() => {
                // Skip the next byte regardless of what it is. This preserves
                // `\]` as a literal `]` inside the bracketed run, matching
                // Tailwind's behavior for selectors like `[&>\\:nth-child(3)]`.
                i += 2;
            }
            b'[' => {
                depth += 1;
                i += 1;
            }
            b']' => {
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    // Unbalanced bracket — `[` never closed before EOF. Treat as "not a
    // candidate" so the outer loop can resume from after the `[`. Without
    // this, an unmatched `[` near the top of a file would swallow every
    // candidate that follows it.
    if depth > 0 {
        return start;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::extract_candidates;

    fn extract(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        extract_candidates(s, &mut out);
        out
    }

    #[test]
    fn simple_class_attribute() {
        let html = r#"<div class="flex items-center hover:bg-red-500"></div>"#;
        let got = extract(html);
        assert!(got.contains(&"flex".to_string()));
        assert!(got.contains(&"items-center".to_string()));
        assert!(got.contains(&"hover:bg-red-500".to_string()));
    }

    #[test]
    fn slash_modifier_survives() {
        let got = extract(r#"<div class="bg-red-500/50 text-sm/6"></div>"#);
        assert!(got.contains(&"bg-red-500/50".to_string()));
        assert!(got.contains(&"text-sm/6".to_string()));
    }

    #[test]
    fn arbitrary_property_bracket_extracted_intact() {
        // Tailwind v3's arbitrary-property syntax: the entire
        // candidate is `[<css-decl>]`. The `]` is NOT followed by
        // `:` (no variant attaches), and the inner contains a
        // top-level `:`. Previously the scanner saw the bracket as
        // an array literal and descended into the contents, dropping
        // the whole-bracket form on the floor.
        let got = extract(r#"<div class="[appearance:textfield] [contain:strict]"></div>"#);
        assert!(
            got.contains(&"[appearance:textfield]".to_string()),
            "expected `[appearance:textfield]` as a token, got: {got:?}",
        );
        assert!(
            got.contains(&"[contain:strict]".to_string()),
            "expected `[contain:strict]` as a token, got: {got:?}",
        );
    }

    #[test]
    fn arbitrary_property_with_nested_function_commas() {
        // Real-world: `[grid-template-columns:repeat(auto-fill,minmax(240px,1fr))]`.
        // The commas live inside `repeat(...)`, balanced parens — the
        // bracket scanner must walk the whole structure as one token.
        let got = extract(
            r#"<div class="[grid-template-columns:repeat(auto-fill,minmax(240px,1fr))]"></div>"#,
        );
        assert!(
            got.contains(
                &"[grid-template-columns:repeat(auto-fill,minmax(240px,1fr))]".to_string()
            ),
            "expected the entire bracketed property as one token, got: {got:?}",
        );
    }

    #[test]
    fn typescript_index_signature_is_not_arbitrary_property() {
        // `{ [key: string]: T }` is a TypeScript index signature.
        // The inner `[key: string]` looks bracket-property-ish but
        // contains a SPACE after the `:`, which Tailwind v3's
        // `defaultExtractor` regex `[^\s\\]+` doesn't allow inside
        // a candidate. Our scanner must mirror that: never emit
        // `[key: string]` as a single candidate, otherwise it leaks
        // into the output as `.[key:\ string]`.
        let got = extract(r#"private excludedClasses: { [key: string]: boolean } = {};"#);
        assert!(
            !got.iter().any(|s| s.contains(' ') || s.contains("[key:")),
            "TS index signature must not produce a bracket-shaped token: {got:?}",
        );
    }

    #[test]
    fn arbitrary_property_with_internal_space_rejected() {
        // Belt-and-braces: any bracketed token whose inner contains
        // whitespace is rejected by Tailwind's scanner regex. We
        // mirror that.
        let got = extract(r#"<div class="[font: var(--x)] [color: red]"></div>"#);
        assert!(
            !got.iter().any(|s| s.starts_with('[') && s.ends_with(']')),
            "spaced bracket content must not produce a candidate: {got:?}",
        );
    }

    #[test]
    fn js_array_literal_still_descends() {
        // A bare `[ 'rounded-3xl p-8' ]` (no `:` inside the brackets)
        // is a JS array literal in source. We must NOT treat it as
        // a candidate — descend so the inner class strings get
        // tokenized.
        let got = extract(r#"const cls = [ 'rounded-3xl p-8' ];"#);
        assert!(got.contains(&"rounded-3xl".to_string()));
        assert!(got.contains(&"p-8".to_string()));
        assert!(
            !got.iter().any(|s| s.starts_with("[") && s.ends_with("]")),
            "JS array literal must not produce a bracket-shaped token: {got:?}",
        );
    }

    #[test]
    fn decimal_fraction_after_digit_survives() {
        // Tailwind's spacing scale includes 0.5, 1.5, 2.5, etc.
        // The tokenizer must keep the dot when it's between digits,
        // so `tw-top-0.5` arrives intact at the parser.
        let got = extract(r#"<div class="tw-top-0.5 tw-mt-1.5 tw-bottom-2.5"></div>"#);
        assert!(
            got.contains(&"tw-top-0.5".to_string()),
            "expected `tw-top-0.5` in tokens, got: {got:?}",
        );
        assert!(
            got.contains(&"tw-mt-1.5".to_string()),
            "expected `tw-mt-1.5` in tokens, got: {got:?}",
        );
        assert!(
            got.contains(&"tw-bottom-2.5".to_string()),
            "expected `tw-bottom-2.5` in tokens, got: {got:?}",
        );
    }

    #[test]
    fn decimal_token_also_emits_pre_dot_prefix() {
        // Tailwind v3's defaultExtractor returns overlapping matches:
        // for `tw-bottom-2.5` it emits BOTH `tw-bottom-2.5` AND
        // `tw-bottom-2`. Both are valid utilities (different theme
        // keys) and the user's content might rely on either. Without
        // this, source containing `class="tw-bottom-2.5"` wouldn't
        // produce `.tw-bottom-2` even though Tailwind 3 would.
        let got = extract(r#"<div class="tw-bottom-2.5 -tw-mr-1.5"></div>"#);
        assert!(got.contains(&"tw-bottom-2.5".to_string()));
        assert!(got.contains(&"tw-bottom-2".to_string()));
        assert!(got.contains(&"-tw-mr-1.5".to_string()));
        assert!(got.contains(&"-tw-mr-1".to_string()));
    }

    #[test]
    fn dot_not_followed_by_digit_terminates_token() {
        // `.` MUST NOT eat property access in JS source —
        // `obj.method` should produce `obj` and `method` as two
        // separate tokens, not one `obj.method`. Same for trailing
        // dots like `done.`. Only `digit-.-digit` survives.
        let got = extract("obj.method foo. bar");
        assert!(got.contains(&"obj".to_string()));
        assert!(got.contains(&"method".to_string()));
        assert!(got.contains(&"foo".to_string()));
        assert!(
            !got.iter().any(|s| s.contains('.')),
            "no token should contain a dot: {got:?}"
        );
    }

    #[test]
    fn arbitrary_value_with_brackets() {
        let got = extract(r#"<div class="grid-cols-[1fr_2fr] w-[calc(100%-1rem)]"></div>"#);
        assert!(got.contains(&"grid-cols-[1fr_2fr]".to_string()));
        assert!(got.contains(&"w-[calc(100%-1rem)]".to_string()));
    }

    #[test]
    fn arbitrary_value_with_quoted_url() {
        let got = extract(r#"<div class="bg-[url('/foo.svg')]"></div>"#);
        assert!(got.contains(&"bg-[url('/foo.svg')]".to_string()));
    }

    #[test]
    fn arbitrary_variant() {
        let got = extract(r#"<div class="[&>*]:mt-2"></div>"#);
        assert!(got.contains(&"[&>*]:mt-2".to_string()));
    }

    #[test]
    fn supports_arbitrary_variant() {
        let got = extract(r#"<div class="supports-[display:grid]:grid"></div>"#);
        assert!(got.contains(&"supports-[display:grid]:grid".to_string()));
    }

    #[test]
    fn data_attribute_variant() {
        let got = extract(r#"<div class="data-[state=open]:block"></div>"#);
        assert!(got.contains(&"data-[state=open]:block".to_string()));
    }

    #[test]
    fn group_named_variant() {
        let got = extract(r#"<div class="group-hover/sidebar:block"></div>"#);
        assert!(got.contains(&"group-hover/sidebar:block".to_string()));
    }

    #[test]
    fn important_and_negative() {
        let got = extract(r#"<div class="!mt-4 -mt-4 hover:!tw-bg-red-500"></div>"#);
        assert!(got.contains(&"!mt-4".to_string()));
        assert!(got.contains(&"-mt-4".to_string()));
        assert!(got.contains(&"hover:!tw-bg-red-500".to_string()));
    }

    #[test]
    fn template_literal_interpolation() {
        // The interpolation breaks the candidate; both sides come out as
        // separate tokens, which is what the parser expects.
        let got = extract(r#"const x = `${foo} bg-red-500`"#);
        assert!(got.contains(&"bg-red-500".to_string()));
    }

    #[test]
    fn backslash_escape_inside_bracket() {
        let got = extract(r#"<div class="[&>\]>span]:block"></div>"#);
        // The escaped `]` does not close the run; the second `]` does.
        assert!(got.iter().any(|c| c == "[&>\\]>span]:block"));
    }

    #[test]
    fn unbalanced_bracket_does_not_panic() {
        let got = extract(r#"<div class="bg-["></div>"#);
        // No assertion on content — we just must not crash.
        assert!(!got.is_empty());
    }

    #[test]
    fn empty_input() {
        let got = extract("");
        assert!(got.is_empty());
    }

    #[test]
    fn jsx_classname() {
        let got = extract(r#"<div className={`flex ${cond ? 'block' : 'hidden'}`}>"#);
        assert!(got.contains(&"flex".to_string()));
        assert!(got.contains(&"block".to_string()));
        assert!(got.contains(&"hidden".to_string()));
    }

    #[test]
    fn jsx_array_of_class_strings() {
        // Common pattern in real codebases: an array of class string
        // segments joined later. The tokenizer must extract every
        // utility from each string segment.
        let src = r#"const x = [
            'relative rounded-3xl p-8 ring-1 xl:p-10',
            tier.featured ? 'bg-gray-900 ring-gray-900' : 'bg-white ring-gray-200 dark:ring-gray-800',
        ].join(' ')"#;
        let got = extract(src);
        for needed in [
            "rounded-3xl",
            "p-8",
            "ring-1",
            "xl:p-10",
            "bg-gray-900",
            "ring-gray-900",
            "bg-white",
            "ring-gray-200",
            "dark:ring-gray-800",
        ] {
            assert!(
                got.contains(&needed.to_string()),
                "missing {needed}; got: {got:?}"
            );
        }
    }

    /// Regression: an unmatched `[` inside a template-literal string
    /// (e.g. a shell ANSI escape `\033[0;31m`) used to make
    /// `consume_bracketed` walk forward looking for a matching `]`,
    /// often to EOF, swallowing every candidate that followed. The
    /// fix bails out on newline-inside-bracket or EOF-with-unbalanced
    /// so the outer loop can resume tokenizing after the stray `[`.
    /// Realistic shape: a TS Storybook file whose template literal
    /// embeds an inline shell example with ANSI escapes — without
    /// this guard the scanner drops every utility class after the
    /// first `\033[` in the file.
    #[test]
    fn unmatched_bracket_in_template_literal_does_not_swallow_followers() {
        let src = r#"
        const SHELL = `
        RED='\033[0;31m'
        GREEN='\033[0;32m'
        YELLOW='\033[1;33m'
        echo "done"
        `;
        const meta = {
            render: () => ({
                template: `<div class="tw-max-h-[500px] tw-overflow-auto"></div>`,
            }),
        };
        "#;
        let got = extract(src);
        assert!(
            got.contains(&"tw-max-h-[500px]".to_string()),
            "tw-max-h-[500px] swallowed by unmatched `[` in shell escape; got: {got:?}",
        );
        assert!(
            got.contains(&"tw-overflow-auto".to_string()),
            "tw-overflow-auto swallowed; got: {got:?}",
        );
    }

    /// Regression: a newline INSIDE a bracketed run is a strong signal
    /// the `[` doesn't belong to a Tailwind arbitrary segment (class
    /// strings in templates are single-line). `consume_bracketed`
    /// should bail rather than walk across the newline.
    #[test]
    fn newline_inside_brackets_terminates_consume() {
        let src = "before \n[unclosed\nstuff\n] tw-flex tw-w-1\n";
        let got = extract(src);
        assert!(
            got.contains(&"tw-flex".to_string()),
            "tw-flex swallowed by newline-inside-bracket; got: {got:?}",
        );
        assert!(
            got.contains(&"tw-w-1".to_string()),
            "tw-w-1 swallowed; got: {got:?}",
        );
    }

    /// Sanity: balanced single-line arbitrary values still resolve
    /// correctly after the `consume_bracketed` guards landed. The
    /// classic `bg-[url('/foo.svg')]` case has quotes and brackets
    /// but no newline — must continue to extract as a single token.
    #[test]
    fn quoted_url_arbitrary_value_unaffected_by_consume_guards() {
        let got = extract(r#"<div class="bg-[url('/foo.svg')]"></div>"#);
        assert!(got.contains(&"bg-[url('/foo.svg')]".to_string()));
    }

    /// Sanity: arbitrary properties with embedded newlines in their
    /// declaration body (rare but legal in some hand-rolled CSS)
    /// also bail correctly — the test guards that we don't suddenly
    /// over-extract once newline handling was added.
    #[test]
    fn arbitrary_property_with_newline_inside_bails() {
        let src = "[grid-template-columns:\n  repeat(3,1fr)\n] tw-block\n";
        let got = extract(src);
        // The unclosed bracketed run no longer captures the inner
        // text as a single candidate; the outer loop resumes after
        // the `[` and still finds the trailing `tw-block`.
        assert!(
            got.contains(&"tw-block".to_string()),
            "tw-block lost after newline-inside-bracket; got: {got:?}",
        );
    }
}
