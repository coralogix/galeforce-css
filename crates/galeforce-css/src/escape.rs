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

//! Tailwind 3 class-name escaping for selectors.
//!
//! Mirrors `tailwindcss/src/util/escapeClassName.js`, which is:
//!
//! ```js
//! escapeClassName(name) =
//!   escapeCommas(cssesc(name, { isIdentifier: true }))
//! ```
//!
//! `cssesc` with `isIdentifier: true` (https://mths.be/cssesc) does:
//!
//! 1. For each character:
//!    - If non-printable (codepoint < 0x20 or > 0x7E) or whitespace control
//!      (`\t \n \f \r \x0B`): emit `\HEX ` (uppercase hex code-point with a
//!      single trailing space).
//!    - If the character is `\`: emit `\\`.
//!    - If the character is in the "single-escape" set
//!      `[ -,] | . | / | [:-@] | [ | ] | ^ | ` | [{-~]`: emit `\X`.
//!    - Otherwise: pass through.
//! 2. After the per-character pass:
//!    - If the result starts with `-` followed by `-` or a digit, prefix the
//!      leading `-` with `\`: `--x` -> `\--x`, `-2xl` -> `\-2xl`.
//!    - If the original first character is a digit, replace it with
//!      `\3<digit> `: `1px` -> `\31 px`.
//! 3. Excessive spaces after `\HEX` runs are collapsed when the trailing
//!    space is unambiguously redundant.
//!
//! Then `escapeCommas` replaces every `\,` with `\2c ` because some
//! browsers treat `\,` differently in selectors. We bake that into the same
//! pass: when we'd emit `\,`, we emit `\2c ` directly.
//!
//! The single-escape set in cssesc's regex (`isIdentifier` form):
//! ```text
//! /[ -,\.\/:-@\[\]\^`\{-~]/
//! ```
//! which expands to:
//! ```text
//!   space ! " # $ % & ' ( ) * +    ,    .    /
//!   :     ; < = > ? @ [ ] ^ ` { | } ~
//! ```
//! Notably absent: `-` (allowed in identifiers), digits (allowed mid-ident),
//! `_`, letters.

const ASCII_IDENT_TABLE: [Action; 128] = build_ascii_table();

#[derive(Clone, Copy)]
enum Action {
    /// Emit the byte unchanged.
    Pass,
    /// Emit `\X` where X is the byte itself.
    SingleEscape,
    /// Emit `\HEX ` form. Used for control chars and whitespace.
    HexEscape,
}

const fn build_ascii_table() -> [Action; 128] {
    let mut t = [Action::HexEscape; 128];
    let mut i: usize = 0;
    while i < 128 {
        let b = i as u8;
        let action = if b == b'\\' {
            // Backslash is double-escaped (\\) which is the same shape as
            // SingleEscape: emit `\` + b. Re-using SingleEscape keeps the
            // table clean.
            Action::SingleEscape
        } else if matches!(
            b,
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'
        ) {
            Action::Pass
        } else if matches!(
            b,
            // Single-escape set per cssesc isIdentifier:
            // ` ` ! " # $ % & ' ( ) * +
            b' '..=b','
            // .
            | b'.'
            // /
            | b'/'
            // : ; < = > ? @
            | b':'..=b'@'
            // [ ] ^ ` { | } ~
            | b'[' | b']' | b'^' | b'`' | b'{'..=b'~'
        ) {
            Action::SingleEscape
        } else {
            // Remaining low-ASCII: 0x00..0x1F controls, 0x7F DEL.
            // `cssesc` emits these as `\HEX ` too.
            Action::HexEscape
        };
        t[i] = action;
        i += 1;
    }
    t
}

/// Comma-escape style for `escape_class_name_with`. The default emits
/// the numeric `\2c ` form (Tailwind v3.4); `Literal` emits the
/// shorter `\,` form (Tailwind v3.3.x byte-identical). Both target
/// the same character — this only affects selector text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CommaEscapeStyle {
    /// `\2c ` — Tailwind v3.4+ (default).
    #[default]
    Numeric,
    /// `\,` — Tailwind v3.3.x.
    Literal,
}

pub fn escape_class_name(name: &str) -> String {
    escape_class_name_with(name, CommaEscapeStyle::Numeric)
}

pub fn escape_class_name_with(name: &str, comma_style: CommaEscapeStyle) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    let first = name.chars().next();

    // Per-character pass.
    for ch in name.chars() {
        if (ch as u32) < 128 {
            let action = ASCII_IDENT_TABLE[ch as usize];
            match action {
                Action::Pass => out.push(ch),
                Action::SingleEscape => {
                    // Emit `\,` here and let the post-pass `escapeCommas`
                    // step (after the collapse logic) substitute `\,`
                    // for `\2c `. Critically, that final step must run
                    // AFTER `collapse_hex_escape_spaces` — otherwise
                    // the collapse routine sees `\2c ` and strips the
                    // trailing space when the next char isn't hex,
                    // producing `\2c\.2` instead of `\2c \.2`. Tailwind's
                    // pipeline is the same: cssesc first, then a
                    // separate `escapeCommas` regex pass.
                    out.push('\\');
                    out.push(ch);
                }
                Action::HexEscape => {
                    push_hex_escape(&mut out, ch as u32);
                }
            }
        } else {
            // Non-ASCII codepoints pass through. cssesc would emit `\HEX `
            // for codepoints > 0x7E, but Tailwind class names in practice
            // never contain non-ASCII. Stay byte-compatible with the ASCII
            // subset; punt on multibyte for now.
            out.push(ch);
        }
    }

    // Identifier-prefix fixups, applied in cssesc order. The leading-digit
    // case emits `\3<digit> ` form; the regex post-process below collapses
    // the trailing space when it isn't load-bearing.
    if let Some(c) = first {
        if c.is_ascii_digit() {
            // out currently starts with the digit verbatim. Re-emit.
            let tail = &out[c.len_utf8()..];
            let mut new_out = String::with_capacity(out.len() + 3);
            new_out.push('\\');
            new_out.push('3');
            new_out.push(c);
            new_out.push(' ');
            new_out.push_str(tail);
            out = new_out;
        } else if c == '-' {
            let mut iter = name.chars();
            iter.next();
            if let Some(second) = iter.next() {
                if second == '-' || second.is_ascii_digit() {
                    let mut new_out = String::with_capacity(out.len() + 1);
                    new_out.push('\\');
                    new_out.push_str(&out);
                    out = new_out;
                }
            }
        }
    }

    let collapsed = collapse_hex_escape_spaces(&out);
    // Final pass: substitute `\,` with `\2c ` per Tailwind's
    // `escapeCommas` (v3.4+ default). Done last so the trailing space
    // is preserved verbatim regardless of what follows. Skip this
    // when callers want v3.3.x byte-equivalence — the `\,` form is
    // still a valid selector escape, just not what 3.4 emits.
    match comma_style {
        CommaEscapeStyle::Numeric => collapsed.replace("\\,", "\\2c "),
        CommaEscapeStyle::Literal => collapsed,
    }
}

fn push_hex_escape(out: &mut String, code: u32) {
    // `\HEX ` form, uppercase.
    out.push('\\');
    let hex = format!("{code:X}");
    out.push_str(&hex);
    out.push(' ');
}

/// Equivalent of cssesc's post-pass:
/// `(^|\+)?(\[A-F0-9]{1,6})\x20(?![a-fA-F0-9\x20])` — the trailing space
/// after a `\HEX` run is redundant when the next char isn't a hex digit or
/// another space. We strip it; otherwise the space is load-bearing and we
/// keep it.
///
/// The cssesc regex also bails out when the run is preceded by an odd
/// number of backslashes (the `\` would be escaping the `\HEX` itself).
/// Tailwind class names never produce that shape, but we mirror the rule
/// for byte-compatibility.
fn collapse_hex_escape_spaces(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            // Count consecutive backslashes ending at i (after copying ones
            // before, we're now standing on the last one). For "odd-leading"
            // detection we look at the chunk we've already emitted into out.
            let preceding_slashes = trailing_backslashes(&out);

            // Look ahead: is this the start of a \HEX run? cssesc only
            // collapses 1..=6 hex digits.
            let hex_start = i + 1;
            let mut hex_end = hex_start;
            while hex_end < bytes.len() && hex_end - hex_start < 6 && is_hex_digit(bytes[hex_end]) {
                hex_end += 1;
            }
            if hex_end > hex_start && hex_end < bytes.len() && bytes[hex_end] == b' ' {
                let after_space = hex_end + 1;
                let next_is_hex_or_space = after_space < bytes.len()
                    && (is_hex_digit(bytes[after_space]) || bytes[after_space] == b' ');
                let safe_to_strip = !next_is_hex_or_space && preceding_slashes % 2 == 0;

                out.extend_from_slice(&bytes[i..hex_end]);
                if !safe_to_strip {
                    out.push(b' ');
                }
                i = hex_end + 1; // skip the space
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    // We only operated on ASCII bytes; non-ASCII passed through.
    String::from_utf8(out).expect("post-process kept UTF-8 boundaries")
}

fn trailing_backslashes(out: &[u8]) -> usize {
    let mut n = 0;
    for &b in out.iter().rev() {
        if b == b'\\' {
            n += 1;
        } else {
            break;
        }
    }
    n
}

fn is_hex_digit(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alphanumeric_passes_through() {
        assert_eq!(escape_class_name("flex"), "flex");
        assert_eq!(escape_class_name("sr-only"), "sr-only");
        assert_eq!(escape_class_name("text_lg"), "text_lg");
        assert_eq!(escape_class_name("p2"), "p2");
    }

    #[test]
    fn variant_colon_is_escaped() {
        // Used inside a selector: `.hover\:flex`.
        assert_eq!(escape_class_name("hover:flex"), "hover\\:flex");
    }

    #[test]
    fn slash_modifier_is_escaped() {
        assert_eq!(escape_class_name("bg-red-500/50"), "bg-red-500\\/50");
    }

    #[test]
    fn brackets_parens_percent_hash() {
        // `bg-[#123456]` -> `bg-\[\#123456\]`. Note `#` is in the single-
        // escape set (`:-@` covers `0x3A..0x40`, so 0x23 `#` is in `[ -,]`).
        assert_eq!(escape_class_name("bg-[#123456]"), "bg-\\[\\#123456\\]");
        assert_eq!(
            escape_class_name("w-[calc(100%-1rem)]"),
            "w-\\[calc\\(100\\%-1rem\\)\\]"
        );
    }

    #[test]
    fn arbitrary_variant_is_escaped() {
        assert_eq!(escape_class_name("[&>*]:mt-2"), "\\[\\&\\>\\*\\]\\:mt-2");
    }

    #[test]
    fn important_bang_is_escaped() {
        assert_eq!(escape_class_name("!mt-4"), "\\!mt-4");
    }

    #[test]
    fn empty_input() {
        assert_eq!(escape_class_name(""), "");
    }

    #[test]
    fn comma_uses_hex_form() {
        // Tailwind's escapeCommas turns `\,` into `\2c `.
        // The resulting CSS character class is `\2c `.
        assert_eq!(escape_class_name("a,b"), "a\\2c b");
    }

    #[test]
    fn comma_literal_style_skips_numeric_escape() {
        // With `CommaEscapeStyle::Literal`, the post-pass that turns
        // `\,` into `\2c ` is skipped — matches Tailwind v3.3.x
        // byte-for-byte. The runtime selector is equivalent.
        assert_eq!(
            escape_class_name_with("a,b", CommaEscapeStyle::Literal),
            "a\\,b",
        );
        assert_eq!(
            escape_class_name_with("min(420px,50vh)", CommaEscapeStyle::Literal),
            "min\\(420px\\,50vh\\)",
        );
    }

    #[test]
    fn comma_followed_by_hex_keeps_space() {
        // `a,0` -> `a\2c 0`. The `0` is a hex digit, so the trailing
        // space after `\2c` is REQUIRED to terminate the escape (without
        // it, `\2c0` parses as U+02C0).
        assert_eq!(escape_class_name("a,0"), "a\\2c 0");
        // Stacked: `a,0,0,.2` (real-world rgba shape inside an
        // arbitrary value, e.g. `before:shadow-[0_2px_5px_rgba(0,0,0,.2)]`).
        assert_eq!(escape_class_name("a,0,0,.2"), "a\\2c 0\\2c 0\\2c \\.2");
    }

    #[test]
    fn leading_digit_uses_hex_form() {
        // `1px` -> `\31px` — the trailing space after \31 is redundant
        // because `p` isn't a hex digit, and cssesc strips it.
        assert_eq!(escape_class_name("1px"), "\\31px");
        // `x` likewise isn't a hex digit (only a-f are).
        assert_eq!(escape_class_name("2xl"), "\\32xl");
    }

    #[test]
    fn leading_digit_followed_by_hex_keeps_space() {
        // `1ax` — `a` IS a hex digit, so the space after `\31` is required
        // to terminate the escape sequence.
        assert_eq!(escape_class_name("1ax"), "\\31 ax");
        // Uppercase hex too.
        assert_eq!(escape_class_name("1Ax"), "\\31 Ax");
    }

    #[test]
    fn leading_digit_followed_by_escaped_space_collapses() {
        // `1 x` -> per-char `1\ x` -> digit prefix `\31 \ x` ->
        // post-process: `\31` is followed by space then `\` (not hex, not
        // space), so the space is redundant -> `\31\ x`.
        assert_eq!(escape_class_name("1 x"), "\\31\\ x");
    }

    #[test]
    fn leading_dash_dash_gets_extra_backslash() {
        // `--var` -> `\--var`.
        assert_eq!(escape_class_name("--var"), "\\--var");
    }

    #[test]
    fn leading_dash_then_digit_gets_extra_backslash() {
        // `-2xl` -> `\-2xl` (per cssesc post-pass rule).
        assert_eq!(escape_class_name("-2xl"), "\\-2xl");
    }

    #[test]
    fn leading_dash_then_letter_unchanged() {
        // `-mt-4` -> `-mt-4` (single leading `-` followed by a letter is
        // valid identifier syntax).
        assert_eq!(escape_class_name("-mt-4"), "-mt-4");
    }

    #[test]
    fn backslash_doubled() {
        assert_eq!(escape_class_name("a\\b"), "a\\\\b");
    }

    #[test]
    fn space_is_single_escape() {
        assert_eq!(escape_class_name("a b"), "a\\ b");
    }

    #[test]
    fn quotes_in_single_escape_set() {
        assert_eq!(escape_class_name("'\""), "\\'\\\"");
    }

    #[test]
    fn parens_dot_at() {
        assert_eq!(escape_class_name("(.@)"), "\\(\\.\\@\\)");
    }

    #[test]
    fn control_char_uses_hex() {
        // Tab -> `\9 ` (uppercase hex).
        assert_eq!(escape_class_name("a\tb"), "a\\9 b");
    }
}
