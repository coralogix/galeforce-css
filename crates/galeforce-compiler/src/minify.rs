//! Simple CSS minifier: char-state-machine, no external crates.
//!
//! Removes `/* … */` comments, collapses whitespace, strips spaces around
//! `{`, `}`, `:`, `;`, `,`, and removes trailing semicolons before `}`.
//! Quoted string values (e.g. `content: " "`) are left untouched.

pub fn minify_css(input: &str) -> String {
    // Phase 1: strip comments and merge whitespace runs, preserving quoted
    // strings verbatim. Output is the "comment-free, whitespace-collapsed"
    // intermediate string.
    let stripped = strip_comments_and_collapse_ws(input);

    // Phase 2: remove spaces around structural characters.
    let compacted = compact_around_punctuation(&stripped);

    // Phase 3: remove trailing `;` before `}`.
    let final_css = remove_trailing_semicolons(&compacted);

    final_css.trim().to_owned()
}

/// Strip `/* … */` comments and collapse runs of whitespace to a single
/// space. Characters inside `'…'` / `"…"` strings are emitted verbatim.
fn strip_comments_and_collapse_ws(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_comment = false;
    let mut string_delim: Option<char> = None;
    let mut last_was_ws = false;

    while i < len {
        let c = chars[i];

        if in_comment {
            // Look for end-of-comment `*/`
            if c == '*' && i + 1 < len && chars[i + 1] == '/' {
                in_comment = false;
                i += 2;
                // A comment that replaced non-whitespace should collapse to
                // one space so adjacent tokens stay separate.
                if !last_was_ws {
                    out.push(' ');
                    last_was_ws = true;
                }
            } else {
                i += 1;
            }
            continue;
        }

        if let Some(delim) = string_delim {
            // Inside a quoted string — emit verbatim, watch for end quote
            // (non-escaped). CSS string escaping uses `\` — if the previous
            // char was `\` the closing quote is escaped and we stay in the
            // string.
            if c == delim && (i == 0 || chars[i - 1] != '\\') {
                string_delim = None;
            }
            out.push(c);
            last_was_ws = false;
            i += 1;
            continue;
        }

        // Outside comment and string.
        if c == '/' && i + 1 < len && chars[i + 1] == '*' {
            in_comment = true;
            i += 2;
            continue;
        }

        if c == '"' || c == '\'' {
            string_delim = Some(c);
            out.push(c);
            last_was_ws = false;
            i += 1;
            continue;
        }

        if c.is_ascii_whitespace() {
            if !last_was_ws {
                out.push(' ');
                last_was_ws = true;
            }
            i += 1;
            continue;
        }

        out.push(c);
        last_was_ws = false;
        i += 1;
    }

    out
}

/// Remove spaces immediately before or after `{`, `}`, `:`, `;`, `,`.
/// Characters inside `'…'` / `"…"` strings are passed through untouched.
fn compact_around_punctuation(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut out = String::with_capacity(len);
    let mut i = 0;
    let mut string_delim: Option<char> = None;

    while i < len {
        let c = chars[i];

        if let Some(delim) = string_delim {
            if c == delim && (i == 0 || chars[i - 1] != '\\') {
                string_delim = None;
            }
            out.push(c);
            i += 1;
            continue;
        }

        if c == '"' || c == '\'' {
            string_delim = Some(c);
            out.push(c);
            i += 1;
            continue;
        }

        // A space that is adjacent to a structural character gets dropped.
        if c == ' ' {
            let prev_is_structural = i > 0 && is_structural(chars[i - 1]);
            let next_is_structural = i + 1 < len && is_structural(chars[i + 1]);
            if prev_is_structural || next_is_structural {
                i += 1;
                continue;
            }
        }

        out.push(c);
        i += 1;
    }

    out
}

#[inline]
fn is_structural(c: char) -> bool {
    matches!(c, '{' | '}' | ':' | ';' | ',')
}

/// Remove `;` that appears immediately before `}` (with no intervening
/// non-whitespace). After `compact_around_punctuation` there should be no
/// spaces between `;` and `}`, so this is a simple two-char replacement.
fn remove_trailing_semicolons(input: &str) -> String {
    // After compaction the sequence is always `;}` with no space in between.
    // Do a linear scan and skip `;` when the next non-whitespace char is `}`.
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut out = String::with_capacity(len);
    let mut string_delim: Option<char> = None;
    let mut i = 0;

    while i < len {
        let c = chars[i];

        if let Some(delim) = string_delim {
            if c == delim && (i == 0 || chars[i - 1] != '\\') {
                string_delim = None;
            }
            out.push(c);
            i += 1;
            continue;
        }

        if c == '"' || c == '\'' {
            string_delim = Some(c);
            out.push(c);
            i += 1;
            continue;
        }

        if c == ';' {
            // Look ahead (skipping any spaces that survived compaction) for `}`.
            let mut j = i + 1;
            while j < len && chars[j] == ' ' {
                j += 1;
            }
            if j < len && chars[j] == '}' {
                // Skip this semicolon.
                i += 1;
                continue;
            }
        }

        out.push(c);
        i += 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::minify_css;

    #[test]
    fn removes_block_comments() {
        let input = "/* hello */ .foo { color: red; /* inline */ }";
        let result = minify_css(input);
        assert!(!result.contains("hello"), "comment should be removed");
        assert!(
            !result.contains("inline"),
            "inline comment should be removed"
        );
        assert!(result.contains(".foo"), "selector should remain");
        assert!(result.contains("color:red"), "declaration should remain");
    }

    #[test]
    fn collapses_whitespace_around_braces_and_colons() {
        let input = ".foo  {  color :  red  ;  }";
        let result = minify_css(input);
        assert_eq!(result, ".foo{color:red}");
    }

    #[test]
    fn removes_trailing_semicolon_before_closing_brace() {
        let input = ".a{color:red;}";
        let result = minify_css(input);
        assert_eq!(result, ".a{color:red}");
    }

    #[test]
    fn preserves_content_inside_quoted_strings() {
        // The space inside `" "` must survive.
        let input = r#".a { content: " "; }"#;
        let result = minify_css(input);
        // The space inside the quoted string must be preserved.
        assert!(
            result.contains("\" \"") || result.contains("' '"),
            "space inside quoted string must be preserved, got: {result}"
        );
        // The outer structure should still be minified.
        assert!(!result.ends_with(';'), "no trailing ; before }}");
    }

    #[test]
    fn full_round_trip_multi_rule() {
        let input = r#"
            /* Generated by GaleforceCSS */
            .flex {
                display: flex;
            }
            .hidden {
                display: none;
            }
            @media (min-width: 768px) {
                .md\:flex {
                    display: flex;
                }
            }
        "#;
        let result = minify_css(input);
        assert_eq!(
            result,
            r#".flex{display:flex}.hidden{display:none}@media (min-width:768px){.md\:flex{display:flex}}"#
        );
    }

    #[test]
    fn multiline_comment_removed() {
        let input = "/* line 1\n   line 2\n   line 3 */ .b { margin: 0; }";
        let result = minify_css(input);
        assert!(
            !result.contains("line"),
            "multiline comment must be removed"
        );
        assert_eq!(result, ".b{margin:0}");
    }
}
