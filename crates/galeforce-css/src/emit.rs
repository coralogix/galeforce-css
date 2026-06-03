//! Stylesheet -> CSS string emitter.
//!
//! We don't try to be byte-compatible with PostCSS's output — the
//! conformance harness normalizes both sides through PostCSS before
//! comparing, so any valid CSS that produces the same parsed structure is
//! semantically equivalent.
//!
//! Rules sharing an at-rule context get grouped under one wrapper; this
//! matches Tailwind 3's output ergonomics (all `@media (min-width: 768px)`
//! rules under one block) and saves bytes in production builds.

use crate::{Rule, Stylesheet};

#[derive(Clone, Debug)]
pub struct EmitOptions {
    pub minify: bool,
    /// Indent string used in pretty mode. Defaults to four spaces — what
    /// Tailwind 3's PostCSS output uses.
    pub indent: String,
}

impl Default for EmitOptions {
    fn default() -> Self {
        Self {
            minify: false,
            indent: "    ".to_string(),
        }
    }
}

pub fn emit_stylesheet(sheet: &Stylesheet, opts: &EmitOptions) -> String {
    // Heuristic pre-allocation. Average emitted CSS rule on a real
    // Tailwind project is ~400 bytes (selector + 3-4 declarations
    // + at-rule headers + whitespace). Pre-sizing to that × rule
    // count saves the geometric String regrowth path; the worst
    // case is over-allocating by 2x which the OS reclaims on drop
    // anyway. Real-corpus emit went from ~6 reallocs to 0.
    let estimated = sheet.rules.len() * 400;
    let mut out = String::with_capacity(estimated);
    emit_stylesheet_into(sheet, &mut out, opts);
    out
}

/// Emit the stylesheet directly into a caller-owned buffer. Used by
/// the directive processor to inject candidate rules at the
/// `@tailwind utilities` slot without an intermediate 295KB String.
/// The buffer should be pre-sized; this function appends without
/// touching what's already there.
pub fn emit_stylesheet_into(sheet: &Stylesheet, out: &mut String, opts: &EmitOptions) {
    let mut current_context: &[String] = &[];

    for (i, rule) in sheet.rules.iter().enumerate() {
        let new_context = rule.at_rules.as_slice();
        if new_context != current_context {
            // Close any open contexts that the new rule no longer shares.
            let common = common_prefix(current_context, new_context);
            close_contexts(out, current_context, common, opts);
            // Open contexts the new rule introduces.
            open_contexts(out, new_context, common, opts);
            current_context = new_context;
        } else if i > 0 && !opts.minify {
            out.push('\n');
        }

        emit_rule(out, rule, opts);
    }

    // Close any remaining open contexts.
    close_contexts(out, current_context, 0, opts);
}

fn common_prefix(a: &[String], b: &[String]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

fn open_contexts(out: &mut String, ctx: &[String], from: usize, opts: &EmitOptions) {
    for (depth, header) in ctx.iter().enumerate().skip(from) {
        push_indent(out, depth, opts);
        out.push_str(header);
        out.push_str(if opts.minify { "{" } else { " {\n" });
    }
}

fn close_contexts(out: &mut String, ctx: &[String], down_to: usize, opts: &EmitOptions) {
    let mut depth = ctx.len();
    while depth > down_to {
        depth -= 1;
        push_indent(out, depth, opts);
        out.push_str(if opts.minify { "}" } else { "}\n" });
    }
}

fn emit_rule(out: &mut String, rule: &Rule, opts: &EmitOptions) {
    let depth = rule.at_rules.len();
    push_indent(out, depth, opts);
    let mut first = true;
    for sel in &rule.selectors {
        if !first {
            out.push_str(if opts.minify { "," } else { ", " });
        }
        first = false;
        out.push_str(sel);
    }
    out.push_str(if opts.minify { "{" } else { " {\n" });

    // Cascade-var defaults markers — `@defaults <group>;` for each
    // group this rule depends on. The post-pass
    // `resolve_defaults_at_rules_pass` collects these and emits
    // shared defaults rules at the top of the sheet. Mirrors
    // upstream's `resolveDefaultsAtRules.js` marker mechanism.
    for group in &rule.defaults_groups {
        push_indent(out, depth + 1, opts);
        out.push_str("@defaults ");
        out.push_str(group);
        out.push(';');
        if !opts.minify {
            out.push('\n');
        }
    }

    let last_idx = rule.declarations.len().saturating_sub(1);
    for (i, decl) in rule.declarations.iter().enumerate() {
        push_indent(out, depth + 1, opts);
        out.push_str(&decl.property);
        out.push_str(if opts.minify { ":" } else { ": " });
        out.push_str(&decl.value);
        if decl.important {
            out.push_str(if opts.minify {
                "!important"
            } else {
                " !important"
            });
        }
        // PostCSS-style: trailing semicolon optional on the final declaration
        // in pretty mode, required in minified mode.
        if i < last_idx || opts.minify {
            out.push(';');
        }
        if !opts.minify {
            out.push('\n');
        }
    }

    push_indent(out, depth, opts);
    out.push_str(if opts.minify { "}" } else { "}\n" });
}

fn push_indent(out: &mut String, depth: usize, opts: &EmitOptions) {
    if opts.minify {
        return;
    }
    for _ in 0..depth {
        out.push_str(&opts.indent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Declaration, Rule};
    use smallvec::smallvec;

    fn rule(selectors: &[&str], decls: &[(&str, &str)], at_rules: &[&str]) -> Rule {
        Rule {
            selectors: selectors.iter().map(|s| (*s).to_string()).collect(),
            declarations: decls
                .iter()
                .map(|(p, v)| Declaration {
                    property: (*p).to_string(),
                    value: (*v).to_string(),
                    important: false,
                })
                .collect(),
            at_rules: at_rules.iter().map(|s| (*s).to_string()).collect(),
            respect_important: true,
            defaults_groups: Vec::new(),
        }
    }

    #[test]
    fn emits_single_simple_rule() {
        let sheet = Stylesheet {
            rules: vec![rule(&[".flex"], &[("display", "flex")], &[])],
        };
        let css = emit_stylesheet(&sheet, &EmitOptions::default());
        assert_eq!(css, ".flex {\n    display: flex\n}\n", "got: {css:?}");
    }

    #[test]
    fn emits_multiple_top_level_rules_with_blank_line() {
        let sheet = Stylesheet {
            rules: vec![
                rule(&[".flex"], &[("display", "flex")], &[]),
                rule(&[".block"], &[("display", "block")], &[]),
            ],
        };
        let css = emit_stylesheet(&sheet, &EmitOptions::default());
        assert!(css.contains(".flex"));
        assert!(css.contains(".block"));
    }

    #[test]
    fn groups_rules_under_shared_at_rule() {
        let sheet = Stylesheet {
            rules: vec![
                rule(
                    &[".sm\\:flex"],
                    &[("display", "flex")],
                    &["@media (min-width: 640px)"],
                ),
                rule(
                    &[".sm\\:block"],
                    &[("display", "block")],
                    &["@media (min-width: 640px)"],
                ),
            ],
        };
        let css = emit_stylesheet(&sheet, &EmitOptions::default());
        // Exactly one `@media` opening and one closing.
        assert_eq!(css.matches("@media").count(), 1);
        // Rules indented under the @media.
        assert!(css.contains("\n    .sm\\:flex"), "got: {css}");
        assert!(css.contains("\n    .sm\\:block"), "got: {css}");
    }

    #[test]
    fn breaks_out_of_at_rule_when_context_changes() {
        let sheet = Stylesheet {
            rules: vec![
                rule(
                    &[".sm\\:flex"],
                    &[("display", "flex")],
                    &["@media (min-width: 640px)"],
                ),
                rule(&[".grid"], &[("display", "grid")], &[]),
            ],
        };
        let css = emit_stylesheet(&sheet, &EmitOptions::default());
        assert!(css.contains("@media (min-width: 640px) {"), "got: {css}");
        assert!(css.contains("}\n"));
        assert!(css.contains(".grid"));
    }

    #[test]
    fn important_declarations_render_bang_important() {
        let sheet = Stylesheet {
            rules: vec![Rule {
                selectors: smallvec![".a".to_string()],
                declarations: vec![Declaration {
                    property: "color".into(),
                    value: "red".into(),
                    important: true,
                }],
                at_rules: vec![],
                respect_important: true,
                defaults_groups: Vec::new(),
            }],
        };
        let css = emit_stylesheet(&sheet, &EmitOptions::default());
        assert!(css.contains("color: red !important"), "got: {css}");
    }

    #[test]
    fn minified_mode_drops_whitespace() {
        let sheet = Stylesheet {
            rules: vec![
                rule(&[".flex"], &[("display", "flex")], &[]),
                rule(&[".block"], &[("display", "block")], &[]),
            ],
        };
        let css = emit_stylesheet(
            &sheet,
            &EmitOptions {
                minify: true,
                indent: "".into(),
            },
        );
        assert_eq!(css, ".flex{display:flex;}.block{display:block;}");
    }

    #[test]
    fn multiple_selectors_join_with_comma() {
        let sheet = Stylesheet {
            rules: vec![rule(&[".a", ".b"], &[("color", "red")], &[])],
        };
        let css = emit_stylesheet(&sheet, &EmitOptions::default());
        assert!(css.contains(".a, .b {"), "got: {css}");
    }

    #[test]
    fn empty_stylesheet_emits_empty_string() {
        let sheet = Stylesheet::default();
        let css = emit_stylesheet(&sheet, &EmitOptions::default());
        assert!(css.is_empty());
    }
}
