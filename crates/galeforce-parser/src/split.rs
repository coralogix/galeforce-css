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

//! Bracket-aware splitting helpers.
//!
//! Tailwind candidates use `:`, `/`, and `[...]` in overlapping ways. These
//! helpers walk a candidate byte-by-byte and return only the positions
//! relevant *outside* any `[...]` run.

/// Byte offsets of every `:` that is not inside a `[...]` run, in source
/// order. Each offset points at the colon itself.
pub fn find_top_level_colons(s: &str) -> Vec<usize> {
    find_top_level_byte(s, b':')
}

/// Like `find_top_level_colons` but with a configurable separator
/// string. Tailwind allows `separator: '__'` (or any identifier-shaped
/// string) for templating systems that can't put `:` inside class
/// attributes. Returns `(byte_offset, separator_len)` pairs so callers
/// can split correctly across multi-byte separators.
pub fn find_top_level_separator_runs(s: &str, sep: &str) -> Vec<(usize, usize)> {
    if sep.len() == 1 {
        return find_top_level_byte(s, sep.as_bytes()[0])
            .into_iter()
            .map(|i| (i, 1))
            .collect();
    }
    let bytes = s.as_bytes();
    let sep_bytes = sep.as_bytes();
    let mut out = Vec::new();
    let mut depth: usize = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => {
                i += 2;
                continue;
            }
            b'[' => {
                depth += 1;
                i += 1;
                continue;
            }
            b']' if depth > 0 => {
                depth -= 1;
                i += 1;
                continue;
            }
            _ => {}
        }
        if depth == 0 && bytes[i..].starts_with(sep_bytes) {
            out.push((i, sep_bytes.len()));
            i += sep_bytes.len();
            continue;
        }
        i += 1;
    }
    out
}

/// Byte offset of the first top-level `/` that is not at position 0
/// (Tailwind never starts a candidate with a modifier slash). Returns
/// `None` if the candidate has no modifier.
pub fn find_top_level_modifier_slash(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth: usize = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => {
                i += 2;
                continue;
            }
            b'[' => depth += 1,
            b']' if depth > 0 => depth -= 1,
            b'/' if depth == 0 && i > 0 => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

fn find_top_level_byte(s: &str, target: u8) -> Vec<usize> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut depth: usize = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => {
                i += 2;
                continue;
            }
            b'[' => depth += 1,
            b']' if depth > 0 => depth -= 1,
            b if b == target && depth == 0 => out.push(i),
            _ => {}
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_colons() {
        assert!(find_top_level_colons("flex").is_empty());
    }

    #[test]
    fn one_top_level_colon() {
        assert_eq!(find_top_level_colons("hover:bg-red-500"), vec![5]);
    }

    #[test]
    fn colons_inside_bracket_are_ignored() {
        // `supports-[display:grid]:grid`
        //                        ^ index 23
        assert_eq!(
            find_top_level_colons("supports-[display:grid]:grid"),
            vec![23]
        );
    }

    #[test]
    fn nested_brackets_balance_correctly() {
        // `[a:b[c:d]e]:x` — outer brackets cover everything, only the final
        // `:` is top-level.
        let s = "[a:b[c:d]e]:x";
        assert_eq!(find_top_level_colons(s), vec![s.find("]:x").unwrap() + 1]);
    }

    #[test]
    fn modifier_slash_at_top_level() {
        assert_eq!(find_top_level_modifier_slash("bg-red-500/50"), Some(10));
    }

    #[test]
    fn modifier_slash_skipped_inside_brackets() {
        assert_eq!(find_top_level_modifier_slash("bg-[url('/foo.svg')]"), None);
    }

    #[test]
    fn modifier_slash_in_arbitrary_modifier() {
        // `border-blue-500/[.31]` — / is at index 15, top-level.
        assert_eq!(
            find_top_level_modifier_slash("border-blue-500/[.31]"),
            Some(15)
        );
    }

    #[test]
    fn modifier_slash_only_returns_first() {
        // `text-sm/6/extra` — Tailwind doesn't allow chained slash modifiers,
        // but if one appears we still take only the first split. Anything
        // after that is the modifier's responsibility to interpret/reject.
        assert_eq!(find_top_level_modifier_slash("text-sm/6/extra"), Some(7));
    }

    #[test]
    fn leading_slash_is_not_a_modifier() {
        // A candidate can't start with `/`. Even if the byte appears at 0,
        // we don't treat it as a modifier split.
        assert_eq!(find_top_level_modifier_slash("/foo"), None);
    }
}
