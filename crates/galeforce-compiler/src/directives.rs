//! CSS input processor: handles `@tailwind`, `@apply`, `@layer`, `theme()`,
//! and `screen()` directives. Phase E.
//!
//! ## Strategy
//!
//! We don't build a full PostCSS-equivalent AST. The directives we care
//! about are sparse and locally identifiable in the source text, and the
//! conformance normalizer (PostCSS-backed) compares semantic structure
//! after both sides parse — so we can string-process aggressively as long
//! as we preserve every byte the user wrote verbatim outside the directive
//! sites.
//!
//! The processor is a small token-aware scanner that:
//!
//! - Emits the user's CSS to an output buffer untouched, with a tiny state
//!   machine that knows about strings, `/* */` comments, parens, and
//!   brackets so that a `;` inside `url("a;b")` doesn't terminate the
//!   surrounding declaration.
//! - When it recognizes `@tailwind <layer>;`, it emits the corresponding
//!   payload (compiled candidate rules for `utilities`, vendored preflight
//!   for `base`, empty for `components`).
//! - When it recognizes `@apply <utilities>;` inside a rule body, it
//!   compiles the listed utilities and inlines their declarations at the
//!   `@apply` position. Variant-aware `@apply` (e.g. `@apply hover:flex`)
//!   is rejected with an `unsupported-apply` diagnostic — supporting it
//!   requires rule-splitting that Phase E carries over.
//! - When it recognizes `theme(<path>)` in a value position, it replaces
//!   the call with the resolved theme value (or emits a diagnostic if the
//!   path is missing).
//! - When it recognizes `screen(<name>)` in an at-rule param position
//!   (`@media screen(md)`), it replaces with the resolved media query
//!   body.
//! - When it recognizes `@layer <name> { ... }`, it strips the wrapper
//!   and emits the contents in place. Layer-position reordering between
//!   user `@layer` blocks and `@tailwind` expansions is intentionally not
//!   modeled — Tailwind's PostCSS pipeline reorders based on the final
//!   document, but for fixtures that interleave the directives correctly
//!   in source order, our passthrough already produces matching output.
//!
//! This is decidedly Phase-E-shaped: enough for `@tailwind utilities;` +
//! simple `@apply` + `theme()` to work end-to-end. A real AST and proper
//! variant-aware `@apply` arrive when value-bearing utilities do.

#![allow(clippy::doc_markdown)]

use rustc_hash::{FxHashMap, FxHashSet};

use galeforce_core::Diagnostic;
use galeforce_css::escape_class_name;
use galeforce_parser::{parse, ParseOptions};
use serde_json::Value;

use crate::find_static;
use crate::try_resolve_value_utility;
use crate::value_utilities::find_value_utilities;
use crate::variants::{apply_variants, VariantContext};

/// Cascade-variable defaults emitted at the top of `@tailwind base;`
/// expansions. Contains the `*, ::before, ::after { --tw-*: 0; ... }`
/// and `::backdrop { ... }` blocks that the transform/filter/ring/
/// scroll-snap/touch-action/gradient corePlugins emit via `addBase`
/// in upstream. Vendored verbatim from a one-shot oracle compile of
/// `@tailwind base;` against a default config — these values are
/// fixed and don't depend on user theme overrides.
const BASE_CASCADE_VARS: &str = include_str!("../vendor/tailwind-base-cascade-vars.css");

/// Tailwind 3.4.19 preflight CSS (the version-banner comment + the
/// `*` reset + element-specific normalizers). Like the cascade-vars
/// block, this is the post-`theme()`-resolution form for a default
/// config; `theme()` calls inside `vendor/tailwindcss-v3/src/css/preflight.css`
/// (e.g. `theme('borderColor.DEFAULT', currentColor)`) have already
/// been evaluated. User configs that override `borderColor.DEFAULT`,
/// `fontFamily.sans`, `fontFamily.mono`, or the placeholder gray
/// would diverge — that's a Phase carry-over (see CLAUDE.md).
const BASE_PREFLIGHT: &str = include_str!("../vendor/tailwind-base-preflight.css");

/// Outcome of candidate-filtering an `addBase` rule's selector.
struct AddBaseEmission {
    /// The selector to emit. `None` means drop the rule entirely
    /// (no candidate matched any class in the selector).
    selector: Option<String>,
    /// True iff a matching candidate had the `!` important prefix —
    /// the emit pass marks every decl `!important`.
    force_important: bool,
}

/// Decide how to emit an `addBase` rule given the candidate set.
/// Mirrors upstream's class-bearing rule gating + `applyImportant`:
///
/// - If the selector is comma-separated, each part is checked
///   independently.
/// - For tag-only / pseudo-only parts (no `.class`), keep verbatim.
/// - For class-bearing parts, the part survives iff at least one
///   referenced class equals a candidate (post `!`/prefix
///   stripping). The matched class is rewritten to `\!class` when
///   the candidate carries `!`, and the rule's decls are marked
///   important.
fn plan_addbase_emission(selector: &str, candidate_set: &FxHashSet<&str>) -> AddBaseEmission {
    let parts = split_top_level_selectors_owned(selector);
    let mut emit_parts: Vec<String> = Vec::new();
    let mut force_important = false;
    for part in parts {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let classes = collect_top_level_classes(trimmed);
        if classes.is_empty() {
            // No class reference — emit as a "not-on-demand" rule.
            emit_parts.push(trimmed.to_string());
            continue;
        }
        // Find a class whose candidate is scanned. Try with and
        // without the leading `!` important prefix.
        let mut hit: Option<(String, bool)> = None;
        for c in &classes {
            if candidate_set.contains(c.as_str()) {
                hit = Some((c.clone(), false));
                break;
            }
            let bang = format!("!{c}");
            if candidate_set.contains(bang.as_str()) {
                hit = Some((c.clone(), true));
                break;
            }
        }
        let Some((class, important)) = hit else {
            continue;
        };
        if important {
            force_important = true;
        }
        let final_part = if important {
            replace_class_token(trimmed, &class, &format!("\\!{class}"))
        } else {
            trimmed.to_string()
        };
        emit_parts.push(final_part);
    }
    if emit_parts.is_empty() {
        return AddBaseEmission {
            selector: None,
            force_important: false,
        };
    }
    AddBaseEmission {
        selector: Some(emit_parts.join(", ")),
        force_important,
    }
}

/// Split `selector` at top-level commas (not inside parens or
/// brackets). Owned strings.
fn split_top_level_selectors_owned(selector: &str) -> Vec<String> {
    let bytes = selector.as_bytes();
    let mut parts = Vec::new();
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'(' => {
                depth_paren += 1;
                i += 1;
            }
            b')' => {
                depth_paren -= 1;
                i += 1;
            }
            b'[' => {
                depth_bracket += 1;
                i += 1;
            }
            b']' => {
                depth_bracket -= 1;
                i += 1;
            }
            b',' if depth_paren == 0 && depth_bracket == 0 => {
                parts.push(selector[start..i].to_string());
                start = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    parts.push(selector[start..].to_string());
    parts
}

/// Collect every top-level `.<class>` token's class name (without
/// the leading dot) from a single selector. Skips classes inside
/// `:not(...)` because those are exclusion clauses.
fn collect_top_level_classes(selector: &str) -> Vec<String> {
    let bytes = selector.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    let mut not_depth = 0i32;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if b == b':' && bytes.get(i + 1) != Some(&b':') {
            // Possible `:not(...)` start.
            let after = i + 1;
            let mut j = after;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'-') {
                j += 1;
            }
            if &selector[after..j] == "not" && bytes.get(j) == Some(&b'(') {
                not_depth += 1;
                i = j + 1;
                continue;
            }
            i = j;
            continue;
        }
        if b == b'(' {
            i += 1;
            continue;
        }
        if b == b')' {
            if not_depth > 0 {
                not_depth -= 1;
            }
            i += 1;
            continue;
        }
        if b == b'.' && not_depth == 0 {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() {
                let c = bytes[j];
                if c == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                    continue;
                }
                if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' {
                    j += 1;
                    continue;
                }
                break;
            }
            if j > start {
                out.push(selector[start..j].to_string());
            }
            i = j;
            continue;
        }
        i += 1;
    }
    out
}

/// Replace `.<old>` with `.<new>` in the selector. Used by the
/// `applyImportant` transform to rewrite the matched class to its
/// `\!class` form. Walks once and bails after the first hit.
fn replace_class_token(selector: &str, old: &str, new: &str) -> String {
    let needle_owned = format!(".{old}");
    let needle = needle_owned.as_str();
    if let Some(idx) = selector.find(needle) {
        let after_idx = idx + needle.len();
        // Make sure the match is at a class boundary (not part of a
        // longer class name).
        let next = selector.as_bytes().get(after_idx).copied();
        let valid_boundary = next.map_or(true, |b| {
            !(b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'\\')
        });
        if valid_boundary {
            let mut out = String::with_capacity(selector.len() + new.len() - old.len());
            out.push_str(&selector[..idx]);
            out.push('.');
            out.push_str(new);
            out.push_str(&selector[after_idx..]);
            return out;
        }
    }
    selector.to_string()
}

/// Body + sibling rules emitted by `process_rule_body`. Sibling
/// rules are top-level rules that must be inserted at the SAME
/// nesting level as the parent rule (i.e. AFTER the parent's
/// closing `}`, not inside the parent's body). Used by variant-
/// aware `@apply` to mirror upstream's rule-splitting behavior.
struct ProcessedBody {
    body: String,
    sibling_rules: Vec<SiblingRule>,
}

/// One chunk in a partitioned rule body. Mirrors upstream's
/// `partitionApplyAtRules.js`, which splits a parent rule into
/// clones — one per contiguous group of `@apply` directives vs
/// non-`@apply` decls — and emits them in source order. After the
/// split, each `@apply` clone is processed independently (sibling
/// emission and empty-parent removal), and `collapseAdjacent` merges
/// adjacent clones that share the same selector.
enum BodyChunk {
    /// A run of declarations (and nested at-rules / `theme()` calls)
    /// between `@apply` directives. Emitted wrapped in
    /// `<parent_selector> { ... }`.
    Decls(String),
    /// Output from a single `@apply` directive.
    Apply {
        in_place_decls: String,
        sibling_rules: Vec<SiblingRule>,
    },
}

struct SiblingRule {
    /// `<selector> { <body> }` is emitted at parent level. When
    /// `selector` is empty, the `body` is taken to be a complete
    /// at-rule + nested rule (e.g. `@media (min-width: 768px) {
    /// .btn:hover { display: none } }`) and rendered verbatim.
    selector: String,
    body: String,
    /// Source-rule order assigned at user-apply-cache build time.
    /// Used to sort `@apply` user-cache siblings so they emit in
    /// upstream's per-source-rule order. `0` for non-cache-derived
    /// siblings (variant-applied static utilities, etc.) which keep
    /// their natural emit order.
    source_order: u32,
}

/// Result of expanding a single `@apply` directive: any in-place
/// declarations (variant-free utilities are inlined where the
/// `@apply` directive sat), plus any sibling rules that need to be
/// emitted at the same nesting level as the parent rule.
struct ApplyExpansion {
    in_place_decls: String,
    sibling_rules: Vec<SiblingRule>,
}

/// Single rule from user-authored CSS, indexed by class name so that
/// `@apply <user-class>` can resolve to it. Mirrors upstream's
/// `buildLocalApplyCache` (see `vendor/tailwindcss-v3/src/lib/expandApplyAtRules.js`).
#[derive(Clone)]
pub struct UserApplyEntry {
    /// Full source selector (`.foo`, `span, .b`, `.foo:hover`, etc.).
    pub selector: String,
    /// Original body text — may contain its own `@apply` directives,
    /// which we expand recursively at apply time.
    pub body: String,
    /// Outermost-first chain of at-rule wrappers around this rule
    /// (e.g. `["@supports (a: b)"]` or `["@media (min-width: 768px)"]`).
    /// When `@apply` resolves through the user cache, an entry with
    /// a non-empty chain emits a sibling rule wrapped in those
    /// at-rules instead of inlining its decls into the parent.
    pub at_rules: Vec<String>,
    /// True when the rule lived inside `@layer utilities` or
    /// `@layer components`. These layers behave like
    /// `addUtilities`/`addComponents` plugin output — the class is
    /// treated as a synthetic utility that participates in the
    /// candidate-driven pipeline (variants, prefix, important
    /// modifier, etc.) on top of the existing `@apply` cache use.
    pub in_layer_utility: bool,
    /// Source-order index assigned at cache-build time. Used by
    /// `@apply` expansion to sort emitted sibling rules so they
    /// match upstream's per-source-rule emit order — crucial for
    /// candidates whose substitution produces the same selector
    /// from multiple source rules with different decls
    /// (`.foo.bar`, `.bar.foo` against `main { @apply foo bar }`).
    pub source_order: u32,
}

/// Map of class name -> list of user rules whose selector contains
/// that class. Built once via a pre-pass over the input CSS, before
/// directive expansion runs. A class can map to multiple entries
/// when the user writes multiple definitions of the same class
/// (e.g. `.a { color: red } @media (...) { .a { color: blue } }`).
#[derive(Default)]
pub struct UserApplyCache {
    /// className -> rules whose selector references that class.
    pub entries: FxHashMap<String, Vec<UserApplyEntry>>,
    /// Monotonic counter assigned to each cached source rule so
    /// `@apply` expansion can sort emitted siblings by source-rule
    /// order, matching upstream's per-rule emit pattern.
    pub next_source_order: u32,
    /// Set of class names that appear as @apply tokens anywhere in
    /// the input. Used to extend the candidate set during
    /// `@layer utilities|components` tree-shaking so that user-CSS
    /// classes referenced via `@apply` are kept in the output.
    pub applied_classes: FxHashSet<String>,
}

/// Outcome of processing user-supplied CSS input.
pub struct ProcessedCss {
    pub css: String,
    pub diagnostics: Vec<Diagnostic>,
}

/// Process user CSS, replacing supported directives with their resolved
/// equivalents. `compiled_utilities` is the CSS string emitted from the
/// candidate compile step — it gets injected wherever
/// `@tailwind utilities;` appears.
///
/// `candidate_set` is the set of post-parse candidate roots — the bare
/// class names (`flex`, `super-flex`, `hover:bg-red-500` etc.) that the
/// compiler accepted. It's used to tree-shake user-supplied
/// `@layer utilities { … }` and `@layer components { … }` rules: each
/// top-level child of those layers is kept verbatim only if at least
/// one class referenced anywhere in its body matches the set.
pub fn process<'a>(
    input: &str,
    compiled_utilities: &str,
    compiled_components: &str,
    cx: &VariantContext,
    config: Option<&Value>,
    candidate_set: &'a FxHashSet<&'a str>,
    blocklist: &'a FxHashSet<&'a str>,
) -> ProcessedCss {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let empty_user_layers = UserLayers::default();
    let mut state = ProcessState {
        compiled_utilities,
        compiled_components,
        cx,
        config,
        diagnostics: &mut diagnostics,
        preflight_enabled: preflight_enabled(config),
        candidate_set,
        blocklist,
        user_layers: empty_user_layers,
        skip_known_layers: false,
        inside_user_layer: false,
        user_cache: UserApplyCache::default(),
        apply_stack: Vec::new(),
    };
    // User-CSS @apply cache: only build it when the input actually
    // contains `@apply`. Mirrors upstream's `lazyCache` — without an
    // @apply directive, walking every rule to index by class name
    // would be pure overhead. With one, we need a complete index so
    // tokens can resolve to user-defined `.foo {…}` rules and not
    // just Tailwind utilities.
    let has_apply = memchr::memmem::find(input.as_bytes(), b"@apply").is_some();
    if has_apply {
        build_user_apply_cache(input, &mut state.user_cache);
    }
    // Fast-path: if there's no `@layer` substring at all, skip the
    // pre-collect pass entirely. The common case for the harness +
    // most user projects is `@tailwind base; @tailwind components;
    // @tailwind utilities;` with no user `@layer` blocks. Saves a
    // full byte-walk over the input.
    if memchr::memmem::find(input.as_bytes(), b"@layer").is_some() {
        state.precollect_user_layers(input);
        state.skip_known_layers = true;
    } else {
        state.skip_known_layers = true;
    }
    let css = state.process(input);
    ProcessedCss { css, diagnostics }
}

/// User-supplied content collected from `@layer base|components|utilities`
/// blocks before the main emit pass. Each is the post-resolution body
/// (with `@apply`, `theme()`, `screen()` already expanded) and — for
/// the utilities/components layers — already tree-shaken against the
/// candidate set. Concatenated, in source order, into the matching
/// `@tailwind X;` slot during emit.
#[derive(Default)]
struct UserLayers {
    base: String,
    components: String,
    utilities: String,
}

/// True when `important: true` is set in the config — every decl
/// in `@layer utilities` (and candidate output) gets tagged with
/// `!important`. The `important: '<sel>'` form (selector prefix)
/// is handled by `apply_important_mode` on candidate rules.
fn optimize_universal_defaults_for_apply(config: Option<&Value>) -> bool {
    if let Some(Value::String(s)) = config.and_then(|c| c.get("experimental")) {
        return s == "all";
    }
    config
        .and_then(|c| c.get("experimental"))
        .and_then(|e| e.get("optimizeUniversalDefaults"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn important_is_bang(config: Option<&Value>) -> bool {
    matches!(
        config.and_then(|c| c.get("important")),
        Some(Value::Bool(true))
    )
}

/// Return the configured `important: '<selector>'` string, or
/// `None` when the user didn't set the selector form (or set the
/// `important: true` boolean instead).
fn important_selector_str(config: Option<&Value>) -> Option<&str> {
    config
        .and_then(|c| c.get("important"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// Apply `important: '<sel>'` selector-prefix mode to a stretch
/// of CSS rules. Walks rule selectors only (skipping comments,
/// strings, nested at-rule headers, declaration bodies), prefixes
/// each top-level comma-separated selector with `<sel> ` and wraps
/// it in `:is(...)` when it contains a top-level combinator.
/// Mirrors upstream's `applyImportantSelector.js` traversal at
/// the user-`@layer` level.
fn apply_important_selector_to_block(body: &str, important_sel: &str) -> String {
    let bytes = body.as_bytes();
    let mut out = String::with_capacity(body.len() + body.len() / 4);
    let mut i = 0;
    let mut depth = 0i32;
    let mut sel_start: Option<usize> = None;
    while i < bytes.len() {
        // Comments pass through verbatim.
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            let end = find_comment_end(bytes, i + 2);
            out.push_str(&body[i..end]);
            i = end;
            continue;
        }
        // Strings pass through verbatim.
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            let end = find_string_end(bytes, i);
            out.push_str(&body[i..end]);
            i = end;
            continue;
        }
        let b = bytes[i];
        if depth == 0 && sel_start.is_none() {
            // Skip whitespace before a selector start.
            if b.is_ascii_whitespace() {
                out.push(b as char);
                i += 1;
                continue;
            }
            // At-rule header — pass through until its `{`.
            if b == b'@' {
                let mut j = i;
                while j < bytes.len() && bytes[j] != b'{' && bytes[j] != b';' {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b';' {
                    out.push_str(&body[i..=j]);
                    i = j + 1;
                    continue;
                }
                if j < bytes.len() && bytes[j] == b'{' {
                    out.push_str(&body[i..=j]);
                    depth += 1;
                    i = j + 1;
                    continue;
                }
                out.push_str(&body[i..]);
                break;
            }
            // Regular rule — capture selector until `{`.
            sel_start = Some(i);
            i += 1;
            continue;
        }
        if let Some(start) = sel_start {
            if b == b'{' {
                let raw_selector = &body[start..i];
                let selector = raw_selector.trim();
                let parts: Vec<&str> = split_top_level_selectors_dir(selector);
                let mut joined = String::new();
                for (n, part) in parts.iter().enumerate() {
                    if n > 0 {
                        joined.push_str(", ");
                    }
                    joined.push_str(important_sel);
                    joined.push(' ');
                    if has_top_level_combinator_dir(part) {
                        joined.push_str(":is(");
                        joined.push_str(part);
                        joined.push(')');
                    } else {
                        joined.push_str(part);
                    }
                }
                let leading_ws_len = raw_selector.len() - raw_selector.trim_start().len();
                out.push_str(&raw_selector[..leading_ws_len]);
                out.push_str(&joined);
                out.push(' ');
                out.push('{');
                depth += 1;
                sel_start = None;
                i += 1;
                continue;
            }
            i += 1;
            continue;
        }
        // Inside a body — pass through, tracking braces.
        if b == b'{' {
            depth += 1;
        } else if b == b'}' {
            depth -= 1;
            if depth == 0 {
                // Re-enter selector-search mode.
                out.push(b as char);
                i += 1;
                continue;
            }
        }
        out.push(b as char);
        i += 1;
    }
    out
}

fn split_top_level_selectors_dir(input: &str) -> Vec<&str> {
    let bytes = input.as_bytes();
    let mut parts: Vec<&str> = Vec::new();
    let mut start = 0;
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'(' => {
                depth_paren += 1;
                i += 1;
            }
            b')' => {
                depth_paren -= 1;
                i += 1;
            }
            b'[' => {
                depth_bracket += 1;
                i += 1;
            }
            b']' => {
                depth_bracket -= 1;
                i += 1;
            }
            b',' if depth_paren == 0 && depth_bracket == 0 => {
                parts.push(input[start..i].trim());
                start = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    let tail = input[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    if parts.is_empty() {
        parts.push(input);
    }
    parts
}

fn has_top_level_combinator_dir(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    let mut prev_non_ws = 0u8;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'"' | b'\'' => {
                let q = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != q {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                if i < bytes.len() {
                    i += 1;
                }
            }
            b'(' => {
                depth_paren += 1;
                prev_non_ws = bytes[i];
                i += 1;
            }
            b')' => {
                depth_paren -= 1;
                prev_non_ws = bytes[i];
                i += 1;
            }
            b'[' => {
                depth_bracket += 1;
                prev_non_ws = bytes[i];
                i += 1;
            }
            b']' => {
                depth_bracket -= 1;
                prev_non_ws = bytes[i];
                i += 1;
            }
            b'>' | b'+' | b'~' if depth_paren == 0 && depth_bracket == 0 => return true,
            b' ' | b'\t' if depth_paren == 0 && depth_bracket == 0 => {
                if prev_non_ws == 0 {
                    i += 1;
                    continue;
                }
                let mut j = i + 1;
                while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                    j += 1;
                }
                if j < bytes.len() {
                    return true;
                }
                i = j;
            }
            c => {
                prev_non_ws = c;
                i += 1;
            }
        }
    }
    false
}

/// Tag every CSS declaration in `body` with `!important`. Walks
/// the body byte-aware (skipping comments / strings / nested
/// at-rules) and inserts `!important` before each declaration's
/// terminating `;`. Idempotent — decls that already carry the
/// flag are left alone.
fn tag_decls_important(body: &str) -> String {
    let bytes = body.as_bytes();
    let mut out = String::with_capacity(body.len() + 32);
    let mut i = 0;
    while i < bytes.len() {
        // Comments pass through verbatim.
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            let end = find_comment_end(bytes, i + 2);
            out.push_str(&body[i..end]);
            i = end;
            continue;
        }
        // Strings pass through.
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            let end = find_string_end(bytes, i);
            out.push_str(&body[i..end]);
            i = end;
            continue;
        }
        // At-rule with body or simple at-rule — skip whole.
        if bytes[i] == b'@' {
            let name_start = i + 1;
            let name_end = scan_ident(bytes, name_start);
            if let Some((after, _, _)) = read_at_rule_with_params_and_block(body, name_end) {
                out.push_str(&body[i..after]);
                i = after;
                continue;
            }
            if let Some(after) = skip_at_rule(body, i) {
                out.push_str(&body[i..after]);
                i = after;
                continue;
            }
        }
        // Nested rule { ... } — recurse.
        if is_selector_lead(bytes[i]) {
            if let Some(brace) = find_top_level_open_brace(bytes, i) {
                if let Some(close) = find_matching_close_brace(bytes, brace) {
                    let inner = &body[brace + 1..close];
                    out.push_str(&body[i..=brace]);
                    out.push_str(&tag_decls_important(inner));
                    out.push('}');
                    i = close + 1;
                    continue;
                }
            }
        }
        // Otherwise — try to match a `prop: value;` declaration.
        // Find the next `;` at depth 0; check that the slice
        // contains a `:` (else it's not a decl).
        if let Some(semi) = find_top_level_semicolon(bytes, i) {
            let chunk = &body[i..semi];
            if chunk.contains(':') {
                let trimmed = chunk.trim_end();
                if trimmed.ends_with("!important") {
                    // Already important — pass through.
                    out.push_str(&body[i..=semi]);
                } else {
                    out.push_str(trimmed);
                    out.push_str(" !important");
                    // Preserve trailing whitespace before `;`.
                    let suffix = &body[i + trimmed.len()..semi];
                    out.push_str(suffix);
                    out.push(';');
                }
                i = semi + 1;
                continue;
            }
            // No `:` — pass the chunk verbatim.
            out.push_str(&body[i..=semi]);
            i = semi + 1;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn preflight_enabled(config: Option<&Value>) -> bool {
    let Some(cfg) = config else { return true };
    let cp = cfg.get("corePlugins");
    match cp {
        Some(Value::Object(map)) => {
            // `corePlugins: { preflight: false, … }` — explicit
            // disable.
            !matches!(map.get("preflight"), Some(Value::Bool(false)))
        }
        Some(Value::Array(arr)) => {
            // `corePlugins: ['plugin-a', …]` allowlist — enabled iff
            // 'preflight' is in the list. After Tailwind's
            // `resolveConfig` runs (used by `@cx/galeforcecss-config-loader`)
            // the object form `{ preflight: false }` is normalised to
            // an allowlist that excludes `preflight`, so this branch
            // catches both the original allowlist and the post-
            // resolveConfig form.
            arr.iter().any(|v| v.as_str() == Some("preflight"))
        }
        Some(Value::Bool(false)) => false,
        _ => true,
    }
}

struct ProcessState<'a> {
    compiled_utilities: &'a str,
    compiled_components: &'a str,
    cx: &'a VariantContext,
    config: Option<&'a Value>,
    diagnostics: &'a mut Vec<Diagnostic>,
    preflight_enabled: bool,
    candidate_set: &'a FxHashSet<&'a str>,
    /// Class names the user has explicitly blocklisted in config.
    /// Used by `tree_shake_layer` to drop user `@layer
    /// utilities|components` rules whose class candidate is blocked,
    /// matching upstream's behavior of running the blocklist filter
    /// against `@layer`-derived utilities too.
    blocklist: &'a FxHashSet<&'a str>,
    user_layers: UserLayers,
    /// When true, top-level `@layer base|components|utilities` blocks
    /// are skipped during emit (they were already collected by the
    /// pre-pass). Always false during the pre-pass itself.
    skip_known_layers: bool,
    /// True while emitting the body of `@layer components|utilities`
    /// (i.e. during the pre-pass collection of those layers). Inside
    /// these blocks, `@apply`-derived sibling rules are emitted
    /// AFTER the parent so the post-pass `collapse_adjacent_rules`
    /// merges same-selector parent+sibling pairs — matching upstream's
    /// behavior where utilities cloned via @apply at the same layer
    /// as the parent end up adjacent. Outside these blocks (top-level
    /// rules in user CSS), siblings emit BEFORE the parent so cloned
    /// utility rules don't merge with the user's own decls.
    inside_user_layer: bool,
    /// User-CSS rules indexed by class name. `@apply` falls back to
    /// this cache when neither static nor value-utility resolution
    /// matches a token. Empty when the input contains no `@apply`
    /// directives (precollect skips the build).
    user_cache: UserApplyCache,
    /// Stack of user classes currently being expanded, used to
    /// detect circular `@apply` chains (`.a → .b → .a`). Mirrors
    /// upstream's `intersects` check in expandApplyAtRules.js.
    apply_stack: Vec<String>,
}

impl ProcessState<'_> {
    fn is_important_bang(&self) -> bool {
        important_is_bang(self.config)
    }

    /// Top-level pass: scan the input, copying bytes to `out` and replacing
    /// directives we recognize.
    fn process(&mut self, input: &str) -> String {
        // The base block (cascade vars + preflight) inserts ~10KB; the
        // user's @layer base/components/utilities slot in their own
        // collected content; @tailwind utilities expands to
        // `compiled_utilities`. Pre-size for the worst common case
        // so the byte loop never trips a String regrow on a typical
        // 295KB output.
        let estimated = input.len()
            + self.compiled_utilities.len()
            + self.compiled_components.len()
            + BASE_CASCADE_VARS.len()
            + if self.preflight_enabled {
                BASE_PREFLIGHT.len()
            } else {
                0
            }
            + self.user_layers.base.len()
            + self.user_layers.components.len()
            + self.user_layers.utilities.len();
        let mut out = String::with_capacity(estimated);
        let bytes = input.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            // Comments pass through verbatim.
            if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                let end = find_comment_end(bytes, i + 2);
                out.push_str(&input[i..end]);
                i = end;
                continue;
            }
            // Strings pass through verbatim.
            if bytes[i] == b'"' || bytes[i] == b'\'' {
                let end = find_string_end(bytes, i);
                out.push_str(&input[i..end]);
                i = end;
                continue;
            }
            // Top-level at-rule.
            if bytes[i] == b'@' {
                if let Some(after) = self.handle_at_rule(input, i, &mut out) {
                    i = after;
                    continue;
                }
            }
            // Generic rule: walk until we find `{` (start of body) and emit
            // the selector + body, processing `@apply` / `theme()` /
            // `screen()` inside.
            if let Some(after) = self.handle_rule(input, i, &mut out) {
                i = after;
                continue;
            }
            // Default: copy byte and advance.
            out.push(bytes[i] as char);
            i += 1;
        }
        out
    }

    /// Try to handle a top-level at-rule starting at `start` (which points
    /// at `@`). Returns `Some(after_index)` if handled, `None` otherwise
    /// so the caller falls through to generic rule scanning.
    fn handle_at_rule(&mut self, input: &str, start: usize, out: &mut String) -> Option<usize> {
        let bytes = input.as_bytes();
        let name_start = start + 1;
        let name_end = scan_ident(bytes, name_start);
        let name = &input[name_start..name_end];
        match name {
            "tailwind" => {
                let (after, layer) = read_at_rule_simple_value(input, name_end)?;
                self.emit_tailwind_layer(layer.trim(), out);
                Some(after)
            }
            "import" => {
                // Mirror upstream `normalizeTailwindDirectives.js`:
                // `@import 'tailwindcss/base'` (and the `components`,
                // `utilities`, `screens`, `variants` siblings) are aliases
                // for the corresponding `@tailwind` directive. Any other
                // `@import` passes through verbatim via the fallthrough.
                let (after, params) = read_at_rule_simple_value(input, name_end)?;
                let trimmed = params.trim();
                let unquoted = if trimmed.len() >= 2
                    && (trimmed.starts_with('"') || trimmed.starts_with('\''))
                    && (trimmed.ends_with('"') || trimmed.ends_with('\''))
                {
                    &trimmed[1..trimmed.len() - 1]
                } else {
                    trimmed
                };
                let layer = match unquoted {
                    "tailwindcss/base" => Some("base"),
                    "tailwindcss/components" => Some("components"),
                    "tailwindcss/utilities" => Some("utilities"),
                    "tailwindcss/screens" | "tailwindcss/variants" => Some("variants"),
                    _ => None,
                };
                if let Some(layer) = layer {
                    self.emit_tailwind_layer(layer, out);
                    Some(after)
                } else {
                    None
                }
            }
            "layer" => {
                let (after, params, body) = read_at_rule_with_params_and_block(input, name_end)?;
                let layer_name = params.trim();
                // Top-level `@layer base|components|utilities` blocks
                // were already pulled into `self.user_layers` by the
                // pre-pass — drop them here so they don't double-emit.
                // Other layer names (and nested blocks during recursive
                // emits where the precollect didn't run) fall through
                // to the strip-wrapper-and-inline path.
                if self.skip_known_layers
                    && matches!(layer_name, "base" | "components" | "utilities")
                {
                    return Some(after);
                }
                if matches!(layer_name, "base" | "components" | "utilities") {
                    let processed = self.process(body);
                    out.push_str(&processed);
                } else {
                    // Unknown / user-defined layer name. Tailwind's
                    // PostCSS pipeline preserves the wrapper verbatim.
                    out.push_str("@layer ");
                    out.push_str(params.trim());
                    out.push_str(" {");
                    let processed = self.process(body);
                    out.push_str(&processed);
                    out.push('}');
                }
                Some(after)
            }
            "responsive" | "variants" => {
                // Deprecated v2 directives — `@responsive` and
                // `@variants` were a way to ask Tailwind to generate
                // responsive / state-variant copies of the inner
                // rule. In v3 the JIT engine handles this on demand:
                // any rule inside `@layer utilities` already
                // participates in candidate-driven variant
                // application, so the wrapper is a no-op for our
                // purposes. Strip the wrapper and inline the body
                // at the parent context. Mirrors upstream's
                // `normalizeTailwindDirectives.js` deprecation path.
                let (after, _params, body) = read_at_rule_with_params_and_block(input, name_end)?;
                let processed = self.process(body);
                out.push_str(&processed);
                Some(after)
            }
            "media" | "supports" | "container" | "page" | "keyframes" | "-webkit-keyframes" => {
                // At-rules that wrap a block: process the params for
                // `screen()` calls, then recurse into the body.
                let (after, params, body) = read_at_rule_with_params_and_block(input, name_end)?;
                let resolved_params = self.resolve_screen_calls(params);
                out.push('@');
                out.push_str(name);
                if !resolved_params.is_empty() {
                    out.push(' ');
                    out.push_str(resolved_params.trim());
                }
                out.push_str(" {");
                let processed = self.process(body);
                out.push_str(&processed);
                out.push('}');
                Some(after)
            }
            "screen" => {
                // `@screen md { ... }` -> `@media (min-width: 768px) { ... }`
                // per Tailwind's substituteScreenAtRules.js: the at-rule is
                // renamed to `media` and `params` is replaced with the
                // built media query.
                let (after, params, body) = read_at_rule_with_params_and_block(input, name_end)?;
                let screen = params.trim();
                if let Some(s) = self.cx.screens.iter().find(|s| s.name == screen) {
                    out.push_str("@media (min-width: ");
                    out.push_str(&s.value);
                    out.push_str(") {");
                    let processed = self.process(body);
                    out.push_str(&processed);
                    out.push('}');
                } else {
                    self.diagnostics.push(Diagnostic::warning(
                        "unknown-screen",
                        format!("`@screen {screen}` does not match any configured screen"),
                    ));
                    out.push_str("@screen ");
                    out.push_str(params);
                    out.push_str(" {");
                    let processed = self.process(body);
                    out.push_str(&processed);
                    out.push('}');
                }
                Some(after)
            }
            _ => None,
        }
    }

    fn emit_tailwind_layer(&mut self, layer: &str, out: &mut String) {
        match layer {
            "utilities" => {
                // User `@layer utilities { … }` content emits FIRST,
                // then candidate-driven `compiled_utilities`. This
                // mirrors upstream's `Offsets.compare` cascade where
                // user-CSS unvarianted rules (layer = utilities,
                // parent_layer = utilities) sort before variant-
                // wrapped utility rules (layer = variants). Reversing
                // the slot order makes the @layer utilities body land
                // ahead of hover:foo / sm:foo synthesized from the
                // same source rules.
                if self.is_important_bang() {
                    out.push_str(&tag_decls_important(&self.user_layers.utilities));
                } else if let Some(sel) = important_selector_str(self.config) {
                    out.push_str(&apply_important_selector_to_block(
                        &self.user_layers.utilities,
                        sel,
                    ));
                } else {
                    out.push_str(&self.user_layers.utilities);
                }
                out.push_str(self.compiled_utilities);
            }
            "base" => {
                // If the JS-side config loader detected user theme
                // overrides for paths preflight references via embedded
                // `theme()` calls (`borderColor.DEFAULT`,
                // `fontFamily.sans/mono`, the placeholder gray), it
                // pre-resolves the base block by running Tailwind itself
                // and ships the result as `config.__resolvedBase`. We
                // prefer that when present so user overrides take
                // effect; otherwise fall back to the vendored form.
                let resolved = self
                    .config
                    .and_then(|c| c.get("__resolvedBase"))
                    .and_then(|v| v.as_str());
                let optimize_defaults = optimize_universal_defaults_for_apply(self.config);
                if let Some(s) = resolved {
                    out.push_str(s);
                } else {
                    // The cascade-var defaults block + preflight reset
                    // are vendored from a one-shot oracle compile of
                    // `@tailwind base;` with `config: {}`. Used when
                    // there's no override that would change them.
                    //
                    // Under `experimental.optimizeUniversalDefaults`,
                    // upstream's `resolveDefaultsAtRules.js` removes
                    // the universal `*, ::before, ::after { --tw-*: ... }`
                    // block — each utility-using rule gets its own
                    // shared defaults rule via the `@defaults <id>`
                    // marker pass instead. So we skip it here too.
                    if !optimize_defaults {
                        out.push_str(BASE_CASCADE_VARS);
                    }
                    if self.preflight_enabled {
                        out.push_str(BASE_PREFLIGHT);
                    }
                }
                // Plugin-supplied `addBase({ body: { margin: 0 } })`
                // rules — emit each as `<selector> { <decls> }` at
                // the `@tailwind base` slot. Mirrors upstream's
                // `addBase` behaviour where base output is grouped
                // before user `@layer base` content.
                //
                // Candidate-driven filtering: rules whose selectors
                // reference class names go through the same
                // candidate-map gating as utilities — they emit
                // only when a matching class is in the scanned set.
                // Important-prefixed candidates (`!a1`) trigger an
                // `applyImportant` transform: the selector's class
                // is rewritten to `\!<class>` and decls are marked
                // important. Class-free selectors (`body`, `html`)
                // emit unconditionally. Mirrors upstream's
                // `setupContextUtils.js` `addBase` plus
                // `generateRules.js` `applyImportant`.
                let plugin_output = crate::plugins::read_plugin_output(self.config);
                for rule in &plugin_output.base {
                    let emission = plan_addbase_emission(&rule.selector, self.candidate_set);
                    let Some(emit_selector) = emission.selector else {
                        continue;
                    };
                    let mut at_chain = String::new();
                    for at in rule.at_rules.iter().rev() {
                        at_chain.push_str(at);
                        at_chain.push_str(" { ");
                    }
                    out.push_str(&at_chain);
                    out.push_str(&emit_selector);
                    out.push_str(" { ");
                    for d in &rule.declarations {
                        out.push_str(&d.property);
                        out.push_str(": ");
                        out.push_str(&d.value);
                        if d.important || emission.force_important {
                            out.push_str(" !important");
                        }
                        out.push_str("; ");
                    }
                    out.push_str(" }");
                    for _ in 0..rule.at_rules.len() {
                        out.push_str(" } ");
                    }
                }
                out.push_str(&self.user_layers.base);
            }
            "components" => {
                // The `container` utility and any plugin-supplied
                // `addComponents` rules emit here, followed by user
                // `@layer components` content. Container is the only
                // built-in component plugin that participates in
                // candidate-driven JIT — its rules are routed into
                // `compiled_components` upstream of this slot.
                out.push_str(self.compiled_components);
                out.push_str(&self.user_layers.components);
            }
            other => {
                self.diagnostics.push(Diagnostic::warning(
                    "unknown-tailwind-layer",
                    format!("`@tailwind {other};` is not a recognized layer"),
                ));
            }
        }
    }

    /// Pre-pass: scan `input` at the top level, locate every
    /// `@layer base|components|utilities { … }` block, fully process
    /// its body (resolving `@apply`/`theme()`/`screen()`), tree-shake
    /// the utilities/components forms against the candidate set, and
    /// stash the result in `self.user_layers`. Doesn't emit anything.
    /// Walks past strings, comments, and arbitrary at-rule/rule
    /// blocks at the top level so an `@layer` reference inside a
    /// string literal can't false-match.
    fn precollect_user_layers(&mut self, input: &str) {
        let bytes = input.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                i = find_comment_end(bytes, i + 2);
                continue;
            }
            if bytes[i] == b'"' || bytes[i] == b'\'' {
                i = find_string_end(bytes, i);
                continue;
            }
            if bytes[i] == b'@' {
                let name_start = i + 1;
                let name_end = scan_ident(bytes, name_start);
                let name = &input[name_start..name_end];
                if name == "layer" {
                    if let Some((after, params, body)) =
                        read_at_rule_with_params_and_block(input, name_end)
                    {
                        let layer_name = params.trim();
                        if matches!(layer_name, "base" | "components" | "utilities") {
                            // Mark layer context so `handle_rule`'s
                            // sibling-vs-parent ordering picks the
                            // "merge with parent" form for cloned
                            // @apply rules at the same layer.
                            let prev_in_user = self.inside_user_layer;
                            if matches!(layer_name, "components" | "utilities") {
                                self.inside_user_layer = true;
                            }
                            let processed = self.process(body);
                            self.inside_user_layer = prev_in_user;
                            let final_body = match layer_name {
                                "components" | "utilities" => self.tree_shake_layer(&processed),
                                _ => processed,
                            };
                            match layer_name {
                                "base" => self.user_layers.base.push_str(&final_body),
                                "components" => self.user_layers.components.push_str(&final_body),
                                "utilities" => self.user_layers.utilities.push_str(&final_body),
                                _ => unreachable!(),
                            }
                            i = after;
                            continue;
                        }
                    }
                }
                // Non-layer at-rule: skip past its block (or `;`-terminated
                // simple form) so we don't false-match `@layer` references
                // inside the body.
                if let Some(after) = skip_at_rule(input, i) {
                    i = after;
                    continue;
                }
            }
            // Generic rule body — skip past matching braces. Falls
            // through to single-byte advance for stray characters.
            if is_selector_lead(bytes[i]) {
                if let Some(after) = skip_rule_block(input, i) {
                    i = after;
                    continue;
                }
            }
            i += 1;
        }
    }

    /// Tree-shake the body of an `@layer utilities` or `@layer components`
    /// block. Each top-level child (rule or at-rule) is kept in full only
    /// if at least one class name referenced anywhere in the child's
    /// selectors / nested selectors appears in `self.candidate_set`.
    /// Children with no class references at all are kept conservatively
    /// (they probably target element selectors).
    fn tree_shake_layer(&self, body: &str) -> String {
        let bytes = body.as_bytes();
        let mut out = String::with_capacity(body.len());
        let mut i = 0;
        while i < bytes.len() {
            // Whitespace passes through to preserve oracle-equivalent
            // formatting (post-normalize, this is a no-op anyway).
            if bytes[i].is_ascii_whitespace() {
                out.push(bytes[i] as char);
                i += 1;
                continue;
            }
            if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                let end = find_comment_end(bytes, i + 2);
                out.push_str(&body[i..end]);
                i = end;
                continue;
            }
            // Top-level child: walk to its closing `}` and decide.
            let start = i;
            let end = match find_top_level_block_end(body, start) {
                Some(e) => e,
                None => {
                    // Couldn't parse the child — keep verbatim and
                    // bail out so we don't lose content.
                    out.push_str(&body[start..]);
                    return out;
                }
            };
            let chunk = &body[start..end];
            // Mirror upstream's `extractCandidates` semantics: each
            // top-level rule child of `@layer utilities|components`
            // is treated like an `addUtilities`/`addComponents` call.
            // A rule is "always kept" (NOT_ON_DEMAND in upstream) if
            // any of its selectors has zero class candidates — e.g.
            // `span` (tag), `#main` (id), `*` (universal). Otherwise
            // it's class-driven: kept only if at least one of its
            // class candidates appears in the content candidate set.
            // `@apply <class>` does NOT keep the rule — upstream
            // treats @apply as decl-inlining, not as a candidate
            // reference. (We model this by NOT consulting
            // `user_cache.applied_classes`.)
            let classes = collect_class_names(chunk);
            // Blocklist: drop a rule whose every class candidate is
            // in the blocklist. Mirrors upstream's behaviour where
            // `blocklist: ['my-custom']` strips `.my-custom { ... }`
            // even inside `@layer utilities`. Rules with tag-only
            // members (`span, .b`) skip this filter.
            let all_blocked =
                !classes.is_empty() && classes.iter().all(|c| self.blocklist.contains(c.as_str()));
            let keep = !all_blocked
                && (rule_has_non_class_only_selector(chunk)
                    || classes
                        .iter()
                        .any(|c| self.candidate_set.contains(c.as_str())));
            if keep {
                out.push_str(chunk);
            }
            i = end;
        }
        out
    }

    /// Try to handle a rule at `start`. The first non-at-rule, non-comment
    /// chunk is treated as a selector list; we look ahead for `{` to start
    /// a body. Returns `Some(after)` on handled, `None` otherwise so the
    /// caller falls through to single-byte copy.
    fn handle_rule(&mut self, input: &str, start: usize, out: &mut String) -> Option<usize> {
        let bytes = input.as_bytes();
        // We only enter "rule" mode when the next non-whitespace char looks
        // like the start of a selector (alphanumeric, `.`, `#`, `*`, `:`,
        // `[`, `&`, `>`, `+`, `~`, `_`). For everything else we let the
        // outer loop fall through.
        if !is_selector_lead(bytes[start]) {
            return None;
        }
        let brace = find_top_level_open_brace(bytes, start)?;
        let close = find_matching_close_brace(bytes, brace)?;
        let selector = &input[start..brace];
        let body = &input[brace + 1..close];
        let inside_layer = self.inside_user_layer;
        let _ = inside_layer;
        // Partition the body into source-order chunks at `@apply`
        // boundaries — mirrors upstream's `partitionApplyAtRules.js`
        // which clones the parent rule once per non-`@apply`/`@apply`
        // group. Each chunk emits its own `<selector> { ... }` rule
        // so the source position of an `@apply` is preserved
        // relative to neighbouring decls. `collapseAdjacent` merges
        // adjacent same-selector clones afterwards (utility-only
        // `@apply foo` next to a decl run still collapses into a
        // single rule, while a variant `@apply hover:foo` keeps the
        // chunks separated by its sibling).
        let chunks = self.process_rule_body_partitioned(body, selector);
        for chunk in &chunks {
            match chunk {
                BodyChunk::Decls(decls) => {
                    if !decls.trim().is_empty() {
                        out.push_str(selector);
                        out.push('{');
                        out.push_str(decls);
                        out.push('}');
                    }
                }
                BodyChunk::Apply {
                    in_place_decls,
                    sibling_rules,
                } => {
                    // Emit the in-place clone (variant-free decls go
                    // in a `<selector> { ... }` rule at the @apply's
                    // source position; if the apply was purely
                    // variant-bearing the in_place_decls are empty
                    // and we skip the clone).
                    if !in_place_decls.trim().is_empty() {
                        out.push_str(selector);
                        out.push('{');
                        out.push_str(in_place_decls);
                        out.push('}');
                    }
                    // Variant-bearing siblings follow the in-place
                    // clone. Mirrors upstream's `parent.after(siblings)`
                    // ordering after partitioning.
                    for sib in sibling_rules {
                        if sib.selector.is_empty() {
                            out.push_str(&sib.body);
                        } else {
                            out.push_str(&sib.selector);
                            out.push_str(" { ");
                            out.push_str(&sib.body);
                            out.push_str(" }");
                        }
                    }
                }
            }
        }
        Some(close + 1)
    }

    /// Walk a rule body and split it into source-order chunks: runs
    /// of declarations between `@apply` directives, and one `Apply`
    /// chunk per `@apply` (with that directive's in-place decls and
    /// variant-bearing sibling rules). Resolves `theme()` calls and
    /// nested at-rules inside decl chunks. Mirrors the
    /// `partitionApplyAtRules` + `expandApplyAtRules` pair from
    /// `vendor/tailwindcss-v3/src/lib`.
    fn process_rule_body_partitioned(
        &mut self,
        body: &str,
        parent_selector: &str,
    ) -> Vec<BodyChunk> {
        let bytes = body.as_bytes();
        let mut chunks: Vec<BodyChunk> = Vec::new();
        let mut current_decls = String::with_capacity(body.len());
        let mut i = 0;
        let flush = |current_decls: &mut String, chunks: &mut Vec<BodyChunk>| {
            if !current_decls.is_empty() {
                chunks.push(BodyChunk::Decls(std::mem::take(current_decls)));
            }
        };
        while i < bytes.len() {
            // Comments pass through into the current decl chunk.
            if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                let end = find_comment_end(bytes, i + 2);
                current_decls.push_str(&body[i..end]);
                i = end;
                continue;
            }
            if bytes[i] == b'"' || bytes[i] == b'\'' {
                let end = find_string_end(bytes, i);
                current_decls.push_str(&body[i..end]);
                i = end;
                continue;
            }
            // @apply line — finalise the current decl chunk and
            // emit an Apply chunk.
            if bytes[i] == b'@' && body[i..].starts_with("@apply") {
                if let Some(semi) = find_top_level_semicolon(bytes, i + "@apply".len()) {
                    let utilities = &body[i + "@apply".len()..semi];
                    let expansion = self.expand_apply(utilities.trim(), parent_selector);
                    flush(&mut current_decls, &mut chunks);
                    chunks.push(BodyChunk::Apply {
                        in_place_decls: expansion.in_place_decls,
                        sibling_rules: expansion.sibling_rules,
                    });
                    i = semi + 1;
                    continue;
                }
            }
            // Nested at-rule inside the body — handled into the
            // current decl chunk.
            if bytes[i] == b'@' {
                if let Some(after) = self.handle_at_rule(body, i, &mut current_decls) {
                    i = after;
                    continue;
                }
            }
            // theme() call — resolve into the current decl chunk.
            if bytes[i].is_ascii_alphabetic() {
                let ident_end = scan_ident(bytes, i);
                let name = &body[i..ident_end];
                if name == "theme" && ident_end < bytes.len() && bytes[ident_end] == b'(' {
                    if let Some(close) = find_matching_close_paren(bytes, ident_end) {
                        let arg = &body[ident_end + 1..close];
                        let resolved = self.resolve_theme(arg.trim());
                        current_decls.push_str(&resolved);
                        i = close + 1;
                        continue;
                    }
                }
            }
            current_decls.push(bytes[i] as char);
            i += 1;
        }
        flush(&mut current_decls, &mut chunks);
        chunks
    }

    /// Walk a rule body and process `@apply`/`theme()`/nested at-rules.
    /// Returns a concatenated body + sibling rules. The
    /// `split_static_when_variant` parameter is currently always `false`
    /// (the recursive user-CSS `@apply` cache path is the only caller);
    /// the original top-level path now uses
    /// `process_rule_body_partitioned` instead which emits source-order
    /// rule clones. Kept for the recursive path where we want
    /// concatenated decls to inline into the outer parent.
    fn process_rule_body_with_options(
        &mut self,
        body: &str,
        parent_selector: &str,
        split_static_when_variant: bool,
    ) -> ProcessedBody {
        let bytes = body.as_bytes();
        let mut out = String::with_capacity(body.len());
        let mut sibling_rules: Vec<SiblingRule> = Vec::new();
        // When any @apply token uses variants, we split the rule:
        // non-variant @apply'd decls move out of the parent body
        // into their own sibling rule (matching the parent selector),
        // variant tokens get their variant-applied sibling rules,
        // and the parent rule keeps the user-authored decls. This
        // mirrors upstream's `expandApplyAtRules.js` rule-splitting,
        // and is required for diff parity with the oracle when the
        // user mixes a `@apply hover:foo` with their own decls in
        // the same rule.
        let mut split_apply_decls = String::new();
        let mut i = 0;
        while i < bytes.len() {
            // Pass comments and strings.
            if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                let end = find_comment_end(bytes, i + 2);
                out.push_str(&body[i..end]);
                i = end;
                continue;
            }
            if bytes[i] == b'"' || bytes[i] == b'\'' {
                let end = find_string_end(bytes, i);
                out.push_str(&body[i..end]);
                i = end;
                continue;
            }
            // @apply line.
            if bytes[i] == b'@' && body[i..].starts_with("@apply") {
                if let Some(semi) = find_top_level_semicolon(bytes, i + "@apply".len()) {
                    let utilities = &body[i + "@apply".len()..semi];
                    let expansion = self.expand_apply(utilities.trim(), parent_selector);
                    if split_static_when_variant && !expansion.sibling_rules.is_empty() {
                        // Variant-bearing tokens trigger the split:
                        // non-variant decls come out as their own
                        // sibling rule (built below). Without any
                        // variant tokens, in-place inlining keeps
                        // the simple case a single rule.
                        split_apply_decls.push_str(&expansion.in_place_decls);
                    } else {
                        out.push_str(&expansion.in_place_decls);
                    }
                    sibling_rules.extend(expansion.sibling_rules);
                    i = semi + 1;
                    continue;
                }
            }
            // Nested at-rule (e.g. @media inside a rule).
            if bytes[i] == b'@' {
                if let Some(after) = self.handle_at_rule(body, i, &mut out) {
                    i = after;
                    continue;
                }
            }
            // theme() call: scan a `theme(` start and replace with its
            // resolved value through the closing paren.
            if bytes[i].is_ascii_alphabetic() {
                let ident_end = scan_ident(bytes, i);
                let name = &body[i..ident_end];
                if name == "theme" && ident_end < bytes.len() && bytes[ident_end] == b'(' {
                    if let Some(close) = find_matching_close_paren(bytes, ident_end) {
                        let arg = &body[ident_end + 1..close];
                        let resolved = self.resolve_theme(arg.trim());
                        out.push_str(&resolved);
                        i = close + 1;
                        continue;
                    }
                }
            }
            out.push(bytes[i] as char);
            i += 1;
        }
        // If we accumulated split @apply decls, prepend a sibling
        // rule with the parent selector so the diff sees `.btn {
        // <apply'd decls> }` separate from `.btn { <user decls> }`.
        // Order matters: oracle emits the @apply'd rule FIRST.
        if !split_apply_decls.is_empty() {
            sibling_rules.insert(
                0,
                SiblingRule {
                    selector: parent_selector.trim().to_string(),
                    body: split_apply_decls,
                    source_order: 0,
                },
            );
        }
        ProcessedBody {
            body: out,
            sibling_rules,
        }
    }

    fn expand_apply(&mut self, utilities: &str, parent_selector: &str) -> ApplyExpansion {
        // Two accumulators: utility-class decls come first in the
        // emitted parent rule, user-CSS-class decls come last.
        // Mirrors upstream's `Offsets.compare`-based sort where
        // `layer = 'utilities'` (3) sorts BEFORE `layer = 'user'`
        // (4) — so `.foo { @apply font-bold }` followed by `.bar
        // { @apply foo text-red-500 }` produces `.bar { <text-red
        // decls>; <font-bold decls> }` (text-red being a utility,
        // font-bold reached via the user-CSS path through `.foo`).
        let mut utility_decls = String::new();
        // User-CSS inline decls tagged with the source-rule order they
        // came from. Mirrors upstream's `offsets.create('user')` per
        // source rule — the smaller `source_order`, the earlier in the
        // input CSS, the earlier the decls emit. Without the sort
        // `@apply bar bop` (where `.bop` came first in the input)
        // would emit decls in token order rather than source order.
        let mut user_decls: Vec<(u32, String)> = Vec::new();
        let mut sibling_rules: Vec<SiblingRule> = Vec::new();

        // Mirror upstream's `extractApplyCandidates`: a trailing
        // `!important` token applies `!important` to every emitted
        // declaration in this `@apply`.
        let tokens: Vec<&str> = utilities.split_whitespace().collect();
        let (tokens, apply_all_important) = match tokens.split_last() {
            Some((last, rest)) if *last == "!important" => (rest.to_vec(), true),
            _ => (tokens, false),
        };

        for token in tokens {
            // `@apply !flex` -> important on each emitted decl.
            let mut tok = token;
            let mut important = apply_all_important;
            if let Some(rest) = tok.strip_prefix('!') {
                tok = rest;
                important = true;
            }

            // Parse the token via the candidate parser so we get
            // variants/root/etc. like a normal class. Variants in
            // `@apply` are valid input; we route them to sibling
            // rules below. Honor the configured prefix so
            // `@apply tw-flex` works under `prefix: 'tw-'`.
            let parse_opts = ParseOptions {
                prefix: self
                    .config
                    .and_then(|c| c.get("prefix"))
                    .and_then(|v| v.as_str()),
                separator: self
                    .config
                    .and_then(|c| c.get("separator"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(":"),
            };
            let parsed = match parse(tok, &parse_opts) {
                Some(p) => p,
                None => {
                    self.diagnostics.push(
                        Diagnostic::warning(
                            "unknown-apply-utility",
                            format!("`@apply {token}` could not be parsed"),
                        )
                        .with_candidate(token),
                    );
                    continue;
                }
            };

            // Try the static-utility table first — same priority order
            // as the candidate compile path.
            let static_util = find_static(parsed.root);
            // Decl source: a Vec<(String, String)> built either from the
            // static-utility's literal decls or from a value-utility
            // resolver. We unify on this shape so the variant/in-place
            // routing below can be shared.
            let mut decls_pairs: Vec<(String, String)> = Vec::new();
            let mut selector_suffix: Option<&'static str> = None;
            // Cascade-var defaults groups the resolved utility
            // participates in. Used under
            // `experimental.optimizeUniversalDefaults` to prepend
            // the group's defaults to the @apply'd decls so they
            // emit at the parent rule's selector before the actual
            // utility decls. Mirrors upstream's `addDefaults`
            // behaviour passing through @apply expansion.
            let mut defaults_groups: &'static [&'static str] = &[];
            if let Some(util) = static_util {
                for (p, v) in util.declarations {
                    decls_pairs.push((p.to_string(), v.to_string()));
                }
            } else {
                // Fall through to value-bearing utilities. Multiple
                // resolvers can claim a prefix (`text-` is fontSize +
                // textColor); we walk them and keep the first that
                // resolves successfully — same precedence the candidate
                // pipeline uses.
                let mut resolved_some = false;
                for (util, value_key) in find_value_utilities(parsed.root) {
                    if let Some(resolved) =
                        try_resolve_value_utility(util, value_key, &parsed, self.config)
                    {
                        decls_pairs = resolved.decls;
                        selector_suffix = resolved.selector_suffix;
                        defaults_groups = resolved.defaults_groups;
                        // Bubble any `@keyframes` blocks (e.g.
                        // `@apply animate-spin` ships `@keyframes spin`)
                        // up as top-level sibling rules. The conformance
                        // normalizer dedupes by name; explicit dedup
                        // here is a perf concern, not correctness.
                        for (name, body) in resolved.extra_keyframes {
                            sibling_rules.push(SiblingRule {
                                selector: String::new(),
                                body: format!("@keyframes {name} {{ {body} }}"),
                                source_order: 0,
                            });
                        }
                        resolved_some = true;
                        break;
                    }
                }
                if !resolved_some {
                    // Fall back to the user-CSS apply cache. Mirrors
                    // upstream's `localCache`: `@apply <user-class>`
                    // resolves to any rule(s) in the user's CSS whose
                    // selector references that class. Each matching
                    // rule's body is recursively expanded and its decls
                    // are inlined into the parent. Variant-bearing
                    // tokens (`@apply hover:foo`) emit sibling rules.
                    // Pick the cache key. The candidate parser strips
                    // a leading `-` into `parsed.negative` and exposes
                    // the dash-less form as `parsed.root`. User-CSS
                    // classes that *literally* start with a dash
                    // (`.-foo-1 { … }`) live under the prefixed key,
                    // so we prefer the prefixed lookup when negative
                    // is set.
                    let cache_key_neg = if parsed.negative {
                        Some(format!("-{}", parsed.root))
                    } else {
                        None
                    };
                    let cache_key_owned = match cache_key_neg
                        .as_deref()
                        .filter(|k| self.user_cache.entries.contains_key(*k))
                    {
                        Some(k) => k.to_string(),
                        None => parsed.root.to_string(),
                    };
                    if self.user_cache.entries.contains_key(&cache_key_owned) {
                        if self.apply_stack.iter().any(|c| c == &cache_key_owned) {
                            // Circular @apply chain — Tailwind throws
                            // here; we degrade to a diagnostic and skip.
                            self.diagnostics.push(
                                Diagnostic::warning(
                                    "circular-apply",
                                    format!("`@apply {token}` creates a circular dependency"),
                                )
                                .with_candidate(token),
                            );
                            continue;
                        }
                        self.apply_stack.push(cache_key_owned.clone());
                        // Clone the entries to release the borrow on
                        // self.user_cache before re-entering process_rule_body.
                        let entries: Vec<(String, String, Vec<String>, u32)> = self
                            .user_cache
                            .entries
                            .get(&cache_key_owned)
                            .map(|v| {
                                v.iter()
                                    .map(|e| {
                                        (
                                            e.selector.clone(),
                                            e.body.clone(),
                                            e.at_rules.clone(),
                                            e.source_order,
                                        )
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        for (src_selector, src_body, src_at_rules, src_order) in &entries {
                            // Recursively process the user rule body
                            // with the parent selector as context.
                            // Result: `body` has the user's plain
                            // decls + any nested @apply'd decls
                            // inlined; `sibling_rules` carries any
                            // variant @apply'd siblings. We pass
                            // `split_static_when_variant: false` so
                            // the inner body's static decls inline
                            // back into the outer caller's
                            // ApplyExpansion as a single block —
                            // otherwise inner-and-outer split would
                            // emit the same .parent selector twice.
                            let processed = self.process_rule_body_with_options(
                                src_body,
                                parent_selector,
                                false,
                            );
                            let body_decls = if important {
                                apply_important_to_decls(&processed.body)
                            } else {
                                processed.body.clone()
                            };
                            // Determine whether the source rule's
                            // selector is "trivial" — a comma-list of
                            // bare `.<matched-class>` entries with
                            // nothing else attached. For those, we
                            // INLINE the body decls into the parent
                            // rule (cheap path — no extra rule).
                            // Otherwise the source has structure
                            // (`.foo:hover`, `.foo.bar`, `.foo + .foo`,
                            // multi-class lists with elements), so we
                            // CLONE the source's selector with the
                            // matched class substituted for the
                            // parent. Mirrors upstream's
                            // `replaceSelector` from
                            // `expandApplyAtRules.js`.
                            let trivial = is_trivial_class_selector(src_selector, &cache_key_owned);
                            // When the @apply token carries variants
                            // (`@apply sm:b`), the parent selector
                            // gets the variant-applied form before
                            // the source's matched class is
                            // substituted in. Apply variants to the
                            // parent class selector and capture the
                            // resulting at-rule chain. Variant-free
                            // tokens skip this branch entirely.
                            let (variant_parent, variant_at_rules): (String, Vec<String>) =
                                if parsed.variants.is_empty() {
                                    (parent_selector.trim().to_string(), Vec::new())
                                } else {
                                    let parent_trimmed = parent_selector.trim();
                                    match apply_variants(&parsed.variants, parent_trimmed, self.cx)
                                    {
                                        Ok(applied) => {
                                            // Apply only the first
                                            // selector format — multi-
                                            // format variants under
                                            // user-cache @apply are
                                            // out of scope.
                                            let p = applied
                                                .selectors
                                                .into_iter()
                                                .next()
                                                .unwrap_or_else(|| parent_trimmed.to_string());
                                            (p, applied.at_rules)
                                        }
                                        Err(reason) => {
                                            self.diagnostics.push(
                                                Diagnostic::warning("unsupported-apply", reason)
                                                    .with_candidate(token),
                                            );
                                            sibling_rules.extend(processed.sibling_rules);
                                            continue;
                                        }
                                    }
                                };
                            // For the trivial-class fast path, only
                            // inline when there are no variants and
                            // no source at-rules — variants always
                            // need to emit a sibling because they
                            // wrap in an at-rule chain.
                            if trivial
                                && src_at_rules.is_empty()
                                && variant_at_rules.is_empty()
                                && parsed.variants.is_empty()
                            {
                                // USER-CSS source — tag with the
                                // source-rule order and stash for the
                                // post-loop sort. Mirrors upstream's
                                // `offsets.create('user')` per source
                                // rule, so two `@apply` user-CSS
                                // tokens emit in the order their
                                // source classes appear in the input,
                                // not the order they were listed in
                                // the `@apply`.
                                user_decls.push((*src_order, body_decls));
                                sibling_rules.extend(processed.sibling_rules);
                                continue;
                            }
                            let new_selector = substitute_class_in_selector(
                                src_selector,
                                &cache_key_owned,
                                &variant_parent,
                            );
                            if new_selector.is_empty() {
                                sibling_rules.extend(processed.sibling_rules);
                                continue;
                            }
                            // Compose at-rule chain: variant at-rules
                            // outermost (responsive / dark), source
                            // rule's at-rules innermost (`@supports`
                            // etc.). Mirrors upstream's wrapping
                            // order in `expandApplyAtRules.js`.
                            let mut all_at_rules = variant_at_rules.clone();
                            all_at_rules.extend(src_at_rules.iter().cloned());
                            if all_at_rules.is_empty() {
                                sibling_rules.push(SiblingRule {
                                    selector: new_selector,
                                    body: body_decls.trim().to_string(),
                                    source_order: *src_order,
                                });
                                sibling_rules.extend(processed.sibling_rules);
                                continue;
                            }
                            let mut wrapped = format!("{new_selector} {{ {} }}", body_decls.trim());
                            for at in all_at_rules.iter().rev() {
                                wrapped = format!("{at} {{ {wrapped} }}");
                            }
                            sibling_rules.push(SiblingRule {
                                selector: String::new(),
                                body: wrapped,
                                source_order: *src_order,
                            });
                            sibling_rules.extend(processed.sibling_rules);
                        }
                        self.apply_stack.pop();
                        // We've routed everything for this token through
                        // the user cache — skip the rest of the per-
                        // token logic (which expects decls_pairs).
                        continue;
                    }
                    self.diagnostics.push(
                        Diagnostic::warning(
                            "unknown-apply-utility",
                            format!("`@apply {token}` references an unknown utility"),
                        )
                        .with_candidate(token),
                    );
                    continue;
                }
            }

            // Render the decls into a CSS body string. `@apply !foo`
            // tags every emitted decl as `!important`; `parsed.important`
            // (the embedded `!flex`-style token form) is not threaded
            // here — that's a pre-existing limitation.
            //
            // Under `experimental.optimizeUniversalDefaults`, prepend
            // `@defaults <id>;` markers for each cascade-var defaults
            // group the resolved utility participates in. The post-pass
            // `resolve_defaults_at_rules_pass` collects these markers
            // and emits one shared defaults rule per (group,
            // isolation-bucket, at-rule context) tuple — mirroring
            // upstream's `resolveDefaultsAtRules.js`. Markers go in
            // the same body as the apply'd decls so the parent rule's
            // selector flows through to the bucket as the using-rule.
            let mut decls = String::new();
            if optimize_universal_defaults_for_apply(self.config) && self.cx.has_tailwind_base {
                for group in defaults_groups {
                    decls.push_str("@defaults ");
                    decls.push_str(group);
                    decls.push(';');
                }
            }
            for (prop, val) in &decls_pairs {
                decls.push_str(prop);
                decls.push_str(": ");
                decls.push_str(val);
                if important {
                    decls.push_str(" !important");
                }
                decls.push(';');
            }

            // Compose the parent-selector + sibling-pair suffix once.
            // Used both for variant-free sibling rules (when only
            // selector_suffix is present) and as the synthetic class
            // we feed to apply_variants below.
            //
            // Crucially we feed the suffix-less form to apply_variants
            // and append the suffix AFTER the variant substitution, so
            // `hover:space-x-4` produces `.x:hover > :not(...) ~ :not(...)`
            // rather than `.x > :not(...) ~ :not(...):hover`. Upstream's
            // expandApplyAtRules.js does the same — the variant
            // selector wraps the class, then the plugin's selector
            // suffix lands at the tail.
            let parent_trimmed = parent_selector.trim().to_string();
            let parent_with_suffix = match selector_suffix {
                Some(s) => format!("{parent_trimmed}{s}"),
                None => parent_trimmed.clone(),
            };
            let _ = escape_class_name; // potential future use

            if parsed.variants.is_empty() {
                if selector_suffix.is_some() {
                    // Variant-free, but the utility wants a non-default
                    // selector (sibling-pair `space-x-*`, `divide-x-*`).
                    // Emit as a sibling rule with the suffixed selector.
                    sibling_rules.push(SiblingRule {
                        selector: parent_with_suffix,
                        body: decls,
                        source_order: 0,
                    });
                } else {
                    // Variant-free, plain selector: inline at the
                    // `@apply` site so the parent rule absorbs the
                    // declarations. UTILITY-source — goes in the
                    // utility accumulator (sorts before user-CSS
                    // decls). `decls_pairs` was populated by static
                    // or value-utility resolution above, never by
                    // the user-CSS cache.
                    utility_decls.push_str(&decls);
                }
            } else {
                // Variant-bearing: emit sibling rule(s) with variants
                // applied to the parent selector first; selector_suffix
                // is appended afterward so `hover:space-x-4` becomes
                // `<parent>:hover<suffix>`, not `<parent><suffix>:hover`.
                match apply_variants(&parsed.variants, &parent_trimmed, self.cx) {
                    Ok(applied) => {
                        // Variants may also bring prepended decls
                        // (`before:`, `after:` need `content:`).
                        let mut prepend = String::new();
                        for (p, v) in &applied.prepend_decls {
                            prepend.push_str(p);
                            prepend.push_str(": ");
                            prepend.push_str(v);
                            prepend.push(';');
                        }
                        // Wrap each selector with any accumulated
                        // at-rules (`md:` -> `@media (min-width: …)`).
                        // For the at-rule case we render the entire
                        // wrapped string into `body` and use an empty
                        // `selector` so the SiblingRule renderer
                        // outputs the at-rule verbatim.
                        let body = format!("{prepend}{decls}");
                        for sel in applied.selectors {
                            // Append the plugin's selector suffix
                            // (sibling-pair combinator) after the
                            // variant substitution.
                            let final_sel = match selector_suffix {
                                Some(s) => format!("{sel}{s}"),
                                None => sel,
                            };
                            // Mirror upstream's `movePseudos` final
                            // pass: pseudo-elements that ended up
                            // before non-attachable tokens
                            // (`::-webkit-scrollbar-track:is(.dark
                            // *)`) move to the tail
                            // (`:is(.dark *)::-webkit-scrollbar-track`).
                            let final_sel = move_pseudo_elements_to_end(&final_sel);
                            let composed = if applied.at_rules.is_empty() {
                                SiblingRule {
                                    selector: final_sel,
                                    body: body.clone(),
                                    source_order: 0,
                                }
                            } else {
                                let mut wrapped = format!("{final_sel} {{ {body} }}");
                                for at in applied.at_rules.iter().rev() {
                                    wrapped = format!("{at} {{ {wrapped} }}");
                                }
                                SiblingRule {
                                    selector: String::new(),
                                    body: wrapped,
                                    source_order: 0,
                                }
                            };
                            sibling_rules.push(composed);
                        }
                    }
                    Err(reason) => {
                        self.diagnostics.push(
                            Diagnostic::warning("unsupported-apply", reason).with_candidate(token),
                        );
                    }
                }
            }
        }

        // Stable sort by source-rule order so user-cache @apply
        // siblings emit in upstream's per-source-rule order. Rules
        // without a source order (`source_order: 0`) keep their
        // natural emit order at the front of the list. Mirrors
        // upstream's `context.offsets.sort` step in
        // `expandApplyAtRules.js` — we only model the source-rule
        // dimension since the per-candidate dimension within each
        // source rule is already preserved by emission order.
        sibling_rules.sort_by_key(|s| s.source_order);

        // Concat utility decls FIRST, user-CSS decls LAST. Mirrors
        // upstream's `Offsets.compare` layer order
        // (`utilities < user`) so e.g. `.bar { @apply foo
        // text-red-500 }` where `.foo` is a user-CSS class with
        // `@apply font-bold` ends up `.bar { <text-red>; font-weight:
        // 700 }` rather than `.bar { font-weight: 700; <text-red> }`.
        // Within the user-CSS group, sort by source-rule order —
        // `@apply bar bop` where `.bop` came first in the input
        // should emit bop's decls before bar's, mirroring upstream's
        // `index` field on the user-layer offset.
        user_decls.sort_by_key(|(o, _)| *o);
        let mut in_place_decls = utility_decls;
        for (_, decls) in &user_decls {
            in_place_decls.push_str(decls);
        }
        ApplyExpansion {
            in_place_decls,
            sibling_rules,
        }
    }

    fn resolve_screen_calls(&mut self, params: &str) -> String {
        // Substitute every top-level `screen(<name>)` with the screen's
        // media query body (without the leading `@media`).
        let bytes = params.as_bytes();
        let mut out = String::with_capacity(params.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i].is_ascii_alphabetic() {
                let end = scan_ident(bytes, i);
                let name = &params[i..end];
                if name == "screen" && end < bytes.len() && bytes[end] == b'(' {
                    if let Some(close) = find_matching_close_paren(bytes, end) {
                        let arg = params[end + 1..close].trim();
                        let resolved = self.resolve_screen(arg);
                        out.push_str(&resolved);
                        i = close + 1;
                        continue;
                    }
                }
            }
            out.push(bytes[i] as char);
            i += 1;
        }
        out
    }

    fn resolve_screen(&mut self, name: &str) -> String {
        // Strip surrounding quotes so `screen('sm')` works the same
        // as `screen(sm)`. Mirrors upstream's value-parser-driven
        // arg unwrapping.
        let unquoted = name.trim().trim_matches(|c| c == '"' || c == '\'');
        if let Some(s) = self.cx.screens.iter().find(|s| s.name == unquoted) {
            // Raw screens already carry the full media query body
            // (e.g. `(max-width: 600px)`, `(min-width: X) and
            // (max-width: Y)`, or `monochrome`). Emit verbatim.
            // Simple-string screens get wrapped in `(min-width: X)`.
            if s.is_raw {
                return s.value.clone();
            }
            return format!("(min-width: {})", s.value);
        }
        self.diagnostics.push(Diagnostic::warning(
            "unknown-screen",
            format!("`screen({name})` does not match any configured screen"),
        ));
        format!("screen({name})")
    }

    fn resolve_theme(&mut self, arg: &str) -> String {
        // Tailwind's `theme()` accepts `(path, default)`. The default may
        // contain commas (e.g. font-family lists), and Tailwind tokenizes
        // via postcss-value-parser. We only need to support a top-level
        // first-argument split, where the path is the first
        // comma-separated chunk and the rest (joined back) is the default.
        let (path_arg, default_arg) = split_first_arg(arg);
        let path_raw = path_arg
            .trim()
            .trim_matches(|c| c == '"' || c == '\'')
            .to_string();
        // `theme(colors.blue.500 / 50%)` — split off an optional alpha
        // suffix. Mirrors upstream's `toPaths` regex
        // (`^([^\s]+)(?![^\[]*\])(?:\s*\/\s*([^\/\s]+))$`). If the
        // path resolves WITH the alpha suffix as a literal key first,
        // upstream uses that — but for our scope we always try
        // alpha-stripping when a `/` is present at the top level.
        let (path, alpha) = split_theme_alpha(&path_raw);
        let segments = to_path_segments(path);
        let theme = self.config.and_then(|cfg| cfg.get("theme"));
        let resolved = theme.and_then(|t| walk_path(t, &segments));
        // Apply theme-section-specific transforms (`fontSize` array
        // → first element, `fontFamily` array → comma join, etc.).
        // Mirrors `vendor/tailwindcss-v3/src/util/transformThemeValue.js`.
        let section = segments.first().map(String::as_str).unwrap_or("");
        let transformed = resolved
            .and_then(|v| transform_theme_value(section, v))
            .and_then(|s| {
                // Apply alpha if present. `withAlphaValue`-equivalent:
                // parse the color, set alpha, format back.
                if let Some(a) = alpha {
                    apply_alpha_to_color(&s, a)
                } else {
                    Some(s)
                }
            });
        match transformed {
            Some(s) if !s.is_empty() => s,
            _ => {
                if let Some(default) = default_arg {
                    // Recursively resolve any nested `theme()` calls
                    // inside the default arg — `theme('colors.blue',
                    // theme('colors.yellow'))` must expand the inner
                    // call when the outer key is missing.
                    return self.resolve_theme_in_string(default.trim());
                }
                self.diagnostics.push(Diagnostic::warning(
                    "unknown-theme-path",
                    format!("`theme({arg})` does not match any theme value"),
                ));
                format!("theme({arg})")
            }
        }
    }

    /// Walk `s` looking for `theme(...)` calls and substitute each
    /// with its resolved value. Used to expand nested `theme()`
    /// calls inside the default-value position of an outer call.
    fn resolve_theme_in_string(&mut self, s: &str) -> String {
        if !s.contains("theme(") {
            return s.to_string();
        }
        let bytes = s.as_bytes();
        let mut out = String::with_capacity(s.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i].is_ascii_alphabetic() {
                let end = scan_ident(bytes, i);
                let name = &s[i..end];
                if name == "theme" && end < bytes.len() && bytes[end] == b'(' {
                    if let Some(close) = find_matching_close_paren(bytes, end) {
                        let arg = &s[end + 1..close];
                        let resolved = self.resolve_theme(arg.trim());
                        out.push_str(&resolved);
                        i = close + 1;
                        continue;
                    }
                }
            }
            out.push(bytes[i] as char);
            i += 1;
        }
        out
    }
}

/// Tokenize a `theme()` path string into segments. Supports both
/// `a.b.c` and `a[b][c]` shapes — bracket form lets a segment
/// contain a `.` literally (`colors[red.500]` → `["colors", "red.500"]`).
/// Mirrors `vendor/tailwindcss-v3/src/util/toPath.js`.
fn to_path_segments(path: &str) -> Vec<String> {
    let mut segments: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_bracket = false;
    for ch in path.chars() {
        match ch {
            '[' => {
                if !current.is_empty() {
                    segments.push(std::mem::take(&mut current));
                }
                in_bracket = true;
            }
            ']' => {
                if !current.is_empty() {
                    segments.push(std::mem::take(&mut current));
                }
                in_bracket = false;
            }
            '.' if !in_bracket => {
                if !current.is_empty() {
                    segments.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        segments.push(current);
    }
    segments
}

/// Split a `theme()` path into `(path, Some(alpha))` when the input
/// matches `<path> / <alpha>` at the top level (not inside `[...]`).
/// Returns `(path, None)` otherwise. Mirrors upstream's `toPaths`
/// regex.
fn split_theme_alpha(path: &str) -> (&str, Option<&str>) {
    let bytes = path.as_bytes();
    // Walk from the end looking for ` / ` at top level (not inside
    // brackets). The split point is the FIRST top-level `/`, with
    // surrounding whitespace allowed.
    let mut i = 0;
    let mut depth_bracket = 0i32;
    while i < bytes.len() {
        match bytes[i] {
            b'[' => depth_bracket += 1,
            b']' => depth_bracket -= 1,
            b'/' if depth_bracket == 0 => {
                // Trim whitespace around the slash to extract path / alpha.
                let mut path_end = i;
                while path_end > 0 && matches!(bytes[path_end - 1], b' ' | b'\t') {
                    path_end -= 1;
                }
                let mut alpha_start = i + 1;
                while alpha_start < bytes.len() && matches!(bytes[alpha_start], b' ' | b'\t') {
                    alpha_start += 1;
                }
                let alpha = &path[alpha_start..];
                if alpha.is_empty() || alpha.contains('/') {
                    return (path, None);
                }
                return (&path[..path_end], Some(alpha));
            }
            _ => {}
        }
        i += 1;
    }
    (path, None)
}

/// Apply an alpha value to a CSS color string, mirroring upstream's
/// `withAlphaValue` + `formatColor`. Returns the alpha-applied
/// string when the color parses; `None` otherwise (caller falls
/// back to the default arg).
fn apply_alpha_to_color(color: &str, alpha: &str) -> Option<String> {
    let parsed = parse_color(color)?;
    Some(format_color_with_alpha(&parsed, alpha))
}

#[derive(Debug, Clone)]
struct ParsedColor {
    /// `rgb`, `rgba`, `hsl`, `hsla`.
    mode: String,
    /// The R/G/B or H/S/L components as strings.
    components: Vec<String>,
}

fn parse_color(value: &str) -> Option<ParsedColor> {
    let v = value.trim();
    // Hex: `#RRGGBB` or `#RGB` (optionally with alpha).
    if let Some(parsed) = parse_hex_color(v) {
        return Some(parsed);
    }
    // Functional: `rgb(...)`, `rgba(...)`, `hsl(...)`, `hsla(...)`.
    if let Some(open) = v.find('(') {
        let mode = v[..open].trim().to_ascii_lowercase();
        if !matches!(mode.as_str(), "rgb" | "rgba" | "hsl" | "hsla") {
            return None;
        }
        if !v.ends_with(')') {
            return None;
        }
        let inner = &v[open + 1..v.len() - 1];
        // Split inner on commas or spaces (not inside parens).
        let components = split_color_components(inner);
        if components.is_empty() {
            return None;
        }
        return Some(ParsedColor { mode, components });
    }
    None
}

fn parse_hex_color(value: &str) -> Option<ParsedColor> {
    let v = value.strip_prefix('#')?;
    let bytes = v.as_bytes();
    if !bytes.iter().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let parts: [String; 3] = match bytes.len() {
        3 | 4 => {
            let pair = |i: usize| {
                let c = bytes[i] as char;
                u8::from_str_radix(&format!("{c}{c}"), 16).ok()
            };
            [
                pair(0)?.to_string(),
                pair(1)?.to_string(),
                pair(2)?.to_string(),
            ]
        }
        6 | 8 => {
            let pair = |i: usize| u8::from_str_radix(&v[i..i + 2], 16).ok();
            [
                pair(0)?.to_string(),
                pair(2)?.to_string(),
                pair(4)?.to_string(),
            ]
        }
        _ => return None,
    };
    Some(ParsedColor {
        mode: "rgb".to_string(),
        components: parts.to_vec(),
    })
}

fn split_color_components(inner: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut depth_paren = 0i32;
    let mut start = 0usize;
    let bytes = inner.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth_paren += 1,
            b')' => depth_paren -= 1,
            b',' | b' ' | b'\t' | b'/' if depth_paren == 0 => {
                let chunk = inner[start..i].trim();
                if !chunk.is_empty() {
                    parts.push(chunk.to_string());
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    let tail = inner[start..].trim();
    if !tail.is_empty() {
        parts.push(tail.to_string());
    }
    parts
}

fn format_color_with_alpha(c: &ParsedColor, alpha: &str) -> String {
    // Mirrors `formatColor`: `rgb`/`hsl` use space-separated body
    // with ` / alpha`; `rgba`/`hsla` use comma-separated with
    // `, alpha`. The first three components are the channels;
    // anything beyond is dropped (the alpha override replaces it).
    let trio: Vec<&str> = c.components.iter().take(3).map(String::as_str).collect();
    if c.mode == "rgba" || c.mode == "hsla" {
        format!("{}({}, {alpha})", c.mode, trio.join(", "))
    } else {
        format!("{}({} / {alpha})", c.mode, trio.join(" "))
    }
}

fn walk_path<'a>(root: &'a Value, segments: &[String]) -> Option<&'a Value> {
    let mut node = root;
    for seg in segments {
        // For arrays, allow numeric segments to index by position
        // (`fontFamily.sans[1].fontFeatureSettings` walks into
        // `array[1]`). Object access still uses the string key.
        if node.is_array() {
            if let Ok(idx) = seg.parse::<usize>() {
                node = node.get(idx)?;
                continue;
            }
        }
        node = node.get(seg)?;
    }
    Some(node)
}

/// Render a resolved theme value as a string, applying the
/// section-specific transform that upstream's
/// `transformThemeValue.js` would apply. Returns `None` for shapes
/// that can't render (objects without a meaningful default).
fn transform_theme_value(section: &str, value: &Value) -> Option<String> {
    match section {
        // `theme('fontSize.lg')` returns the size only (first array
        // element) when the entry is `[size, options]`.
        "fontSize" | "outline" => match value {
            Value::Array(arr) => arr.first().and_then(|v| v.as_str().map(str::to_string)),
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        },
        // `fontFamily` arrays may be `[families]` or
        // `[families, options]`. Join the families with `, `.
        "fontFamily" => match value {
            Value::Array(arr) => {
                let families = match arr.get(1) {
                    Some(Value::Object(_)) => arr.first(),
                    _ => Some(value),
                };
                match families {
                    Some(Value::Array(list)) => Some(
                        list.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                    Some(Value::String(s)) => Some(s.clone()),
                    _ => None,
                }
            }
            Value::String(s) => Some(s.clone()),
            _ => None,
        },
        // Sections that join arrays with `, ` per upstream.
        "boxShadow"
        | "transitionProperty"
        | "transitionDuration"
        | "transitionDelay"
        | "transitionTimingFunction"
        | "backgroundImage"
        | "backgroundSize"
        | "backgroundColor"
        | "cursor"
        | "animation" => match value {
            Value::Array(arr) => Some(
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        },
        _ => match value {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            // Object with DEFAULT key — return that value.
            Value::Object(map) => map
                .get("DEFAULT")
                .and_then(|v| transform_theme_value(section, v)),
            _ => None,
        },
    }
}

/// Split `arg` into (first_arg, rest) at the first top-level comma. Returns
/// `(arg, None)` if there is no top-level comma. Used to peel the path
/// argument off `theme('a.b', currentColor, 1)`-style calls.
fn split_first_arg(arg: &str) -> (&str, Option<&str>) {
    let bytes = arg.as_bytes();
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' => {
                i = find_string_end(bytes, i);
                continue;
            }
            b'(' => depth_paren += 1,
            b')' => depth_paren -= 1,
            b'[' => depth_bracket += 1,
            b']' => depth_bracket -= 1,
            b',' if depth_paren == 0 && depth_bracket == 0 => {
                return (&arg[..i], Some(&arg[i + 1..]));
            }
            _ => {}
        }
        i += 1;
    }
    (arg, None)
}

// ---- byte-level helpers ----

fn is_selector_lead(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'.' | b'#' | b'*' | b':' | b'[' | b'&' | b'>' | b'+' | b'~' | b'_' | b'-'
        )
}

fn scan_ident(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' {
            i += 1;
        } else {
            break;
        }
    }
    i
}

fn find_comment_end(bytes: &[u8], from: usize) -> usize {
    let mut i = from;
    while i + 1 < bytes.len() {
        if bytes[i] == b'*' && bytes[i + 1] == b'/' {
            return i + 2;
        }
        i += 1;
    }
    bytes.len()
}

fn find_string_end(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == quote {
            return i + 1;
        }
        i += 1;
    }
    bytes.len()
}

fn find_top_level_open_brace(bytes: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i = find_comment_end(bytes, i + 2);
                continue;
            }
            b'"' | b'\'' => {
                i = find_string_end(bytes, i);
                continue;
            }
            b'(' => depth_paren += 1,
            b')' => depth_paren -= 1,
            b'[' => depth_bracket += 1,
            b']' => depth_bracket -= 1,
            b'{' if depth_paren == 0 && depth_bracket == 0 => return Some(i),
            b';' if depth_paren == 0 && depth_bracket == 0 => return None,
            _ => {}
        }
        i += 1;
    }
    None
}

fn find_matching_close_brace(bytes: &[u8], open: usize) -> Option<usize> {
    let mut i = open + 1;
    let mut depth = 1i32;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i = find_comment_end(bytes, i + 2);
                continue;
            }
            b'"' | b'\'' => {
                i = find_string_end(bytes, i);
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn find_matching_close_paren(bytes: &[u8], open_paren: usize) -> Option<usize> {
    let mut i = open_paren + 1;
    let mut depth = 1i32;
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' => {
                i = find_string_end(bytes, i);
                continue;
            }
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn find_top_level_semicolon(bytes: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i = find_comment_end(bytes, i + 2);
                continue;
            }
            b'"' | b'\'' => {
                i = find_string_end(bytes, i);
                continue;
            }
            b'(' => depth_paren += 1,
            b')' => depth_paren -= 1,
            b'[' => depth_bracket += 1,
            b']' => depth_bracket -= 1,
            b';' if depth_paren == 0 && depth_bracket == 0 => return Some(i),
            b'}' if depth_paren == 0 && depth_bracket == 0 => return None,
            _ => {}
        }
        i += 1;
    }
    None
}

/// Read `@<name> <value>;` returning `(after_semicolon, value)` where
/// value is the substring between the at-rule name end and the `;`.
/// Accepts EOF as an implicit terminator — bare `@tailwind utilities`
/// at the end of a string is treated the same as `@tailwind utilities;`.
/// Mirrors PostCSS, which terminates body-less at-rules at EOF.
fn read_at_rule_simple_value(input: &str, name_end: usize) -> Option<(usize, &str)> {
    let bytes = input.as_bytes();
    if let Some(semi) = find_top_level_semicolon(bytes, name_end) {
        let value = &input[name_end..semi];
        return Some((semi + 1, value));
    }
    // No `;` found — accept EOF as implicit terminator iff the
    // remainder doesn't contain `{` (which would make this a
    // block-form at-rule the caller should NOT treat as simple).
    let rest = &input[name_end..];
    if rest.contains('{') {
        return None;
    }
    Some((bytes.len(), rest))
}

/// Read `@<name> <params> { <body> }` returning `(after, params, body)`.
fn read_at_rule_with_params_and_block(input: &str, name_end: usize) -> Option<(usize, &str, &str)> {
    let bytes = input.as_bytes();
    let open = find_top_level_open_brace(bytes, name_end)?;
    let close = find_matching_close_brace(bytes, open)?;
    let params = &input[name_end..open];
    let body = &input[open + 1..close];
    Some((close + 1, params, body))
}

/// Skip past an at-rule starting at `start` (which points at `@`).
/// Used by the precollection pass to step over non-`@layer` at-rules
/// without descending into them. An at-rule terminates at the first
/// top-level `;` (simple form, e.g. `@import "x";`) OR at the close
/// of its first `{...}` block. Returns `Some(after)` on a clean step,
/// `None` if the at-rule's syntax doesn't terminate (caller falls
/// through to byte-by-byte advance).
fn skip_at_rule(input: &str, start: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    // Walk forward from after the `@<name>` to whichever terminator
    // arrives first.
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b';' => return Some(i + 1),
            b'{' => {
                let close = find_matching_close_brace(bytes, i)?;
                return Some(close + 1);
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => i = find_comment_end(bytes, i + 2),
            b'"' | b'\'' => i = find_string_end(bytes, i),
            _ => i += 1,
        }
    }
    None
}

/// Skip past a regular CSS rule (selector + `{ … }`) starting at `start`.
/// Returns `Some(after)` on a balanced step. Used by the precollection
/// pass to step over rules without recursing.
fn skip_rule_block(input: &str, start: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                let close = find_matching_close_brace(bytes, i)?;
                return Some(close + 1);
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => i = find_comment_end(bytes, i + 2),
            b'"' | b'\'' => i = find_string_end(bytes, i),
            b';' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// Walk forward from `start` (a top-level child within a layer body)
/// to the end of that child. Handles two shapes:
///
/// - At-rule with block: `@media (…) { … }` — closes at the matching `}`.
/// - Regular rule: `selector { … }` — closes at the matching `}`.
/// - Trailing whitespace/comments after the close are absorbed too,
///   so consecutive children don't share boundary noise.
fn find_top_level_block_end(input: &str, start: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                let close = find_matching_close_brace(bytes, i)?;
                let mut j = close + 1;
                // Absorb trailing whitespace so the kept-or-dropped
                // boundary doesn't strand stray newlines.
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                return Some(j);
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => i = find_comment_end(bytes, i + 2),
            b'"' | b'\'' => i = find_string_end(bytes, i),
            b';' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// Collect every class name referenced by selectors anywhere in `input`.
/// Used for `@layer utilities`/`@layer components` tree-shaking — each
/// child's class set is checked against the candidate set, and the
/// child is dropped wholesale if no matches are found.
///
/// "Class name" is `.<ident>` where ident matches Tailwind's class
/// alphabet (alphanum, `-`, `_`). Inside `[...]` (attribute selectors,
/// arbitrary values) we skip — `[class~="foo"]` shouldn't match `class`
/// or `foo`. Inside string literals we skip too.
/// True if the rule chunk `<selector_list> { … }` (or `@<at> { rules… }`)
/// contains at least one selector whose top-level form has no class
/// candidate. Mirrors upstream's "non-on-demandable" check: a rule
/// like `span, .b { ... }` ships with `span` (no class) AND `.b`
/// (one class), so the rule is always emitted. A rule like
/// `.foo, .bar { ... }` has classes on every selector, so it's
/// candidate-driven.
///
/// Walks each top-level rule's selector list (and recurses into
/// nested at-rules' rules). Splits each selector on top-level commas
/// and checks whether each part has at least one class.
fn rule_has_non_class_only_selector(chunk: &str) -> bool {
    // Walk top-level rules in `chunk`. For each rule, split its
    // selector list on top-level commas; if any part has no `.x`
    // class reference, return true.
    let bytes = chunk.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i = find_comment_end(bytes, i + 2);
            continue;
        }
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            i = find_string_end(bytes, i);
            continue;
        }
        if bytes[i] == b'@' {
            // Nested at-rule: recurse into its body. The at-rule
            // header itself isn't a selector, so we don't check it.
            let name_start = i + 1;
            let name_end = scan_ident(bytes, name_start);
            if let Some((after, _params, body)) =
                read_at_rule_with_params_and_block(chunk, name_end)
            {
                if rule_has_non_class_only_selector(body) {
                    return true;
                }
                i = after;
                continue;
            }
            if let Some(after) = skip_at_rule(chunk, i) {
                i = after;
                continue;
            }
            i += 1;
            continue;
        }
        if is_selector_lead(bytes[i]) {
            let start = i;
            let brace = match find_top_level_open_brace(bytes, start) {
                Some(b) => b,
                None => break,
            };
            let close = match find_matching_close_brace(bytes, brace) {
                Some(c) => c,
                None => break,
            };
            let selector_list = &chunk[start..brace];
            for sel in split_top_level_selectors(selector_list) {
                if !selector_has_class(&sel) {
                    return true;
                }
            }
            // Recurse into nested at-rules inside the body too.
            let body = &chunk[brace + 1..close];
            if rule_has_non_class_only_selector(body) {
                return true;
            }
            i = close + 1;
            continue;
        }
        i += 1;
    }
    false
}

/// Split a selector list on top-level commas, ignoring commas inside
/// `()` or `[]` (e.g. `:is(a, b)`). Returns trimmed selector strings.
fn split_top_level_selectors(input: &str) -> Vec<String> {
    let bytes = input.as_bytes();
    let mut parts = Vec::new();
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' => {
                i = find_string_end(bytes, i);
                continue;
            }
            b'(' => depth_paren += 1,
            b')' => depth_paren -= 1,
            b'[' => depth_bracket += 1,
            b']' => depth_bracket -= 1,
            b',' if depth_paren == 0 && depth_bracket == 0 => {
                parts.push(input[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    let tail = input[start..].trim();
    if !tail.is_empty() {
        parts.push(tail.to_string());
    }
    parts
}

/// True if the selector text contains at least one `.x` class
/// reference at the top level (outside `[]`/`()`). Used to decide
/// whether a rule is candidate-driven for tree-shaking.
fn selector_has_class(selector: &str) -> bool {
    let bytes = selector.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'[' => {
                if let Some(close) = find_matching_close_bracket(bytes, i) {
                    i = close + 1;
                } else {
                    i += 1;
                }
                continue;
            }
            b'"' | b'\'' => {
                i = find_string_end(bytes, i);
                continue;
            }
            b':' => {
                // Skip past `:not(...)` — classes inside aren't
                // candidates per upstream's `ignoreNot` walk in
                // `setupContextUtils.js extractCandidates`. The
                // negated class isn't what makes the rule match,
                // so it shouldn't pin the rule to the candidate
                // set during `@layer` tree-shaking.
                if selector[i..].starts_with(":not(") {
                    let open = i + 4;
                    if let Some(close) = find_matching_close_paren(bytes, open) {
                        i = close + 1;
                        continue;
                    }
                }
                i += 1;
                continue;
            }
            b'.' => {
                let next = i + 1;
                if next < bytes.len() && is_class_ident_start(bytes[next]) {
                    return true;
                }
                i += 1;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// True if `selector` is a comma-list of bare `.<class>` entries
/// where every part is exactly `.<class>` (or whitespace-trimmed
/// equivalent) and at least one matches `class_name`. Used to fast-
/// path the user-cache `@apply` lookup: when the source rule has
/// only plain class selectors, the parent rule can absorb the decls
/// inline. When the source has structure (pseudo, combinators,
/// multi-class compound), we fall back to selector substitution.
fn is_trivial_class_selector(selector: &str, class_name: &str) -> bool {
    let mut found_match = false;
    for part in split_top_level_selectors(selector) {
        let trimmed = part.trim();
        let bare = match trimmed.strip_prefix('.') {
            Some(s) => s,
            None => return false,
        };
        if bare == class_name {
            found_match = true;
        }
        // Reject if anything else exists past the class identifier
        // (`.foo.bar`, `.foo:hover`, `.foo > *`, etc.).
        if !is_simple_class_ident(bare) {
            return false;
        }
    }
    found_match
}

/// True if `s` is a CSS-class-identifier-shaped run with no
/// pseudo/compound/combinator characters.
fn is_simple_class_ident(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || !b.is_ascii() {
            i += 1;
            continue;
        }
        if b == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        return false;
    }
    true
}

/// Public entry point for the candidate-driven user-layer utility
/// synthesis in `lib.rs`. Substitutes EVERY occurrence of
/// `.<class_name>` in the selector with `replacement` — mirrors
/// upstream's candidate path which replaces all instances. The
/// `@apply` path inside this module uses
/// `substitute_class_in_selector` which only replaces the first
/// (per upstream's "don't replace multiple instances" comment in
/// `expandApplyAtRules.js`).
pub fn substitute_class_in_selector_public(
    selector: &str,
    class_name: &str,
    replacement: &str,
) -> String {
    let mut out_parts: Vec<String> = Vec::new();
    for part in split_top_level_selectors(selector) {
        let trimmed = part.trim();
        let replaced = replace_all_classes(trimmed, class_name, replacement);
        if let Some(s) = replaced {
            out_parts.push(s);
        }
    }
    out_parts.join(", ")
}

fn replace_all_classes(selector: &str, class_name: &str, replacement: &str) -> Option<String> {
    let mut current = selector.to_string();
    let mut found_any = false;
    loop {
        match replace_first_class(&current, class_name, replacement) {
            Some(next) if next == current => break,
            Some(next) => {
                found_any = true;
                current = next;
            }
            None => break,
        }
    }
    if found_any {
        Some(current)
    } else {
        None
    }
}

/// Substitute the FIRST occurrence of `.<class_name>` in each comma-
/// separated selector part with `replacement`. Selector parts that
/// don't reference `class_name` are dropped (mirrors upstream's
/// `replaceSelector` which only keeps parts where the matched class
/// existed). Returns the joined result. Empty when no part matched.
///
/// Mirrors upstream's `replaceSelector` in `expandApplyAtRules.js`,
/// including the "don't replace multiple instances" comment in that
/// function: a single source selector like `.foo + .foo` only has
/// its FIRST `.foo` substituted with the parent.
fn substitute_class_in_selector(selector: &str, class_name: &str, replacement: &str) -> String {
    let mut out_parts: Vec<String> = Vec::new();
    for part in split_top_level_selectors(selector) {
        if let Some(replaced) = replace_first_class(part.trim(), class_name, replacement) {
            out_parts.push(replaced);
        }
    }
    out_parts.join(", ")
}

/// Find the first `.<class_name>` token in `selector` and replace
/// it with `replacement`, applying a small "sort tags first"
/// rewrite over the affected compound. Returns `None` if
/// `class_name` doesn't appear at the top level.
///
/// The compound around the matched class (the run of selector
/// characters between two combinators) is reconstructed by:
///   1. Removing the matched `.class_name` token from the compound;
///   2. Splitting the remainder into its constituent parts (`.bar`,
///      `:hover`, `::before`, `[attr]`);
///   3. Prepending `replacement` (which represents the parent
///      selector — typically a tag or class chain), so a tag
///      replacement lands BEFORE classes / pseudo-classes /
///      pseudo-elements within the same compound. Mirrors
///      upstream's per-group sort in `replaceSelector` ("tag before
///      class, class before pseudo-element").
///
/// Bracket / paren contents are preserved verbatim — we never
/// substitute inside `:has(.foo)` or `[data-foo='bar']`.
fn replace_first_class(selector: &str, class_name: &str, replacement: &str) -> Option<String> {
    let bytes = selector.as_bytes();
    let needle = class_name.as_bytes();
    let mut i = 0;
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => {
                i += 2;
                continue;
            }
            b'"' | b'\'' => {
                let q = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != q {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                if i < bytes.len() {
                    i += 1;
                }
                continue;
            }
            b':' if depth_paren == 0 && depth_bracket == 0 => {
                // Selector-list pseudo-classes — `:is(...)`,
                // `:where(...)`, `:has(...)`, `:not(...)` —
                // contain selectors inside their parens. Recurse
                // INTO the content so a class substitution like
                // `:where(.foo)` -> `:where(.hover\:foo:hover)`
                // happens. Other pseudo-classes (e.g.
                // `:nth-child(2n)`) get their content skipped
                // verbatim — those parens hold expressions,
                // not selectors.
                let prefix_len = match selector[i..].split('(').next() {
                    Some(p) => p.len(),
                    None => 0,
                };
                let head = &selector[i..i + prefix_len];
                let is_selector_list =
                    head == ":is" || head == ":where" || head == ":has" || head == ":not";
                if is_selector_list && i + prefix_len < bytes.len() && bytes[i + prefix_len] == b'('
                {
                    let open = i + prefix_len;
                    if let Some(close) = find_matching_close_paren(bytes, open) {
                        let inner = &selector[open + 1..close];
                        if let Some(replaced) = replace_first_class(inner, class_name, replacement)
                        {
                            let mut out = String::with_capacity(selector.len() + replacement.len());
                            out.push_str(&selector[..open + 1]);
                            out.push_str(&replaced);
                            out.push_str(&selector[close..]);
                            return Some(out);
                        }
                        // No match inside; skip past the whole pseudo.
                        i = close + 1;
                        continue;
                    }
                }
                i += 1;
                continue;
            }
            b'(' => {
                depth_paren += 1;
                i += 1;
                continue;
            }
            b')' => {
                depth_paren -= 1;
                i += 1;
                continue;
            }
            b'[' => {
                depth_bracket += 1;
                i += 1;
                continue;
            }
            b']' => {
                depth_bracket -= 1;
                i += 1;
                continue;
            }
            b'.' if depth_paren == 0 && depth_bracket == 0 => {
                let cls_start = i;
                let id_start = i + 1;
                if id_start + needle.len() <= bytes.len()
                    && &bytes[id_start..id_start + needle.len()] == needle
                {
                    let after_idx = id_start + needle.len();
                    let next_ok = after_idx == bytes.len() || {
                        let c = bytes[after_idx];
                        !(c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b'\\')
                    };
                    if next_ok {
                        // Find compound boundaries (run between combinators).
                        let compound_start = compound_left_boundary(bytes, cls_start);
                        let compound_end = compound_right_boundary(bytes, after_idx);
                        let pre = &selector[..compound_start];
                        let post = &selector[compound_end..];
                        // Tokenize the source compound so we can
                        // splice the replacement's tokens in place
                        // of the matched class. Avoids string-level
                        // collisions like `.bar` + `header` →
                        // `.barheader` (interpreted as one class)
                        // by working on parsed tokens throughout.
                        let compound = &selector[compound_start..compound_end];
                        let src_tokens = tokenize_compound(compound);
                        let class_token_idx = src_tokens.iter().position(|t| {
                            t.kind == TokenKind::Class
                                && t.text
                                    .strip_prefix('.')
                                    .map(|s| s == class_name)
                                    .unwrap_or(false)
                        });
                        let replacement_tokens = tokenize_compound(replacement);
                        // Two empirically-distinct upstream paths:
                        //   * Tag-bearing replacement (e.g. parent
                        //     selector `header:nth-of-type(odd)`):
                        //     replace AT POSITION, then sort lets
                        //     the tag float ahead of remaining
                        //     classes — so `.bar.foo` substituting
                        //     `.foo` emits
                        //     `header.bar:nth-of-type(odd)`.
                        //   * Class-only replacement (variant chain
                        //     applied to a multi-class compound,
                        //     e.g. `.hover\:bar:hover` substituting
                        //     `.bar` in `.foo.bar.baz`): hoist the
                        //     replacement to compound start so the
                        //     substituted class leads —
                        //     `.hover\:bar:hover.foo.baz`.
                        let replacement_has_tag =
                            replacement_tokens.iter().any(|t| t.kind == TokenKind::Tag);
                        let merged: Vec<CompoundToken> = match class_token_idx {
                            Some(idx) if replacement_has_tag => {
                                let mut v: Vec<CompoundToken> = Vec::with_capacity(
                                    src_tokens.len() + replacement_tokens.len() - 1,
                                );
                                v.extend(src_tokens.iter().take(idx).cloned());
                                v.extend(replacement_tokens.iter().cloned());
                                v.extend(src_tokens.iter().skip(idx + 1).cloned());
                                v
                            }
                            Some(idx) => {
                                let mut v: Vec<CompoundToken> = Vec::with_capacity(
                                    src_tokens.len() + replacement_tokens.len() - 1,
                                );
                                v.extend(replacement_tokens.iter().cloned());
                                v.extend(src_tokens.iter().take(idx).cloned());
                                v.extend(src_tokens.iter().skip(idx + 1).cloned());
                                v
                            }
                            None => src_tokens.clone(),
                        };
                        let sorted = sort_token_list(&merged);
                        let mut out = String::with_capacity(pre.len() + sorted.len() + post.len());
                        out.push_str(pre);
                        out.push_str(&sorted);
                        out.push_str(post);
                        // Pseudo-elements like `::after` must sit at
                        // the END of the *full* selector (CSS
                        // pseudo-elements are terminal). When the
                        // substituted compound is followed by a
                        // descendant chain (`.bar1 .baz1`), the
                        // `::after` token should move past the
                        // descendants. Mirrors upstream's
                        // `movePseudos` post-pass.
                        let final_selector = move_pseudo_elements_to_end(&out);
                        return Some(final_selector);
                    }
                }
                i += 1;
                continue;
            }
            _ => {
                i += 1;
            }
        }
    }
    None
}

/// Hoist pseudo-elements (and their attached pseudo-classes) to the
/// tail of each top-level selector. Direct port of upstream's
/// `movePseudos` post-pass in
/// `vendor/tailwindcss-v3/src/util/pseudoElements.js` — see
/// `selector_ast` for the AST + algorithm. Comma-separated
/// selectors are processed independently.
fn move_pseudo_elements_to_end(selector: &str) -> String {
    let parsed = crate::selector_ast::parse_selector_list(selector);
    let moved: Vec<_> = parsed
        .into_iter()
        .map(crate::selector_ast::move_pseudos_in_selector)
        .collect();
    crate::selector_ast::serialize_selector_list(&moved)
}

/// Sort an already-tokenized compound's tokens and re-emit. Mirrors
/// upstream's per-group sort in `replaceSelector` from
/// `expandApplyAtRules.js`:
///   * tag before class/id;
///   * class/id before pseudo-element (`::xxx`);
///   * pseudo-classes (`:xxx`) keep their relative position.
fn sort_token_list(tokens: &[CompoundToken]) -> String {
    if tokens.is_empty() {
        return String::new();
    }
    // Bucket: 0 = tag, 1 = class/id/attr, 2 = pseudo-class, 3 = pseudo-element.
    // The pseudo-class bucket is sandwiched between classes and
    // pseudo-elements but the comparator returns 0 against classes,
    // so we KEEP source-order for adjacent pairs of class/pseudo-
    // class. To reproduce that, we emit in three passes and rely
    // on stable iteration for pseudo-class ordering.
    let mut indexed: Vec<(usize, &CompoundToken)> = tokens.iter().enumerate().collect();
    // Mirror upstream's pairwise comparator. Stable sort with this.
    indexed.sort_by(|(ai, a), (bi, b)| {
        use TokenKind::*;
        match (a.kind, b.kind) {
            (Tag, Class) | (Tag, Id) | (Tag, Attr) => std::cmp::Ordering::Less,
            (Class, Tag) | (Id, Tag) | (Attr, Tag) => std::cmp::Ordering::Greater,
            (Class, PseudoElement) | (Id, PseudoElement) | (Attr, PseudoElement) => {
                std::cmp::Ordering::Less
            }
            (PseudoElement, Class) | (PseudoElement, Id) | (PseudoElement, Attr) => {
                std::cmp::Ordering::Greater
            }
            _ => ai.cmp(bi),
        }
    });
    let total: usize = tokens.iter().map(|t| t.text.len()).sum();
    let mut out = String::with_capacity(total);
    for (_, tok) in indexed {
        out.push_str(tok.text);
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TokenKind {
    Tag,
    Class,
    Id,
    Attr,
    PseudoClass,
    PseudoElement,
    Other,
}

#[derive(Clone, Debug)]
struct CompoundToken<'a> {
    kind: TokenKind,
    text: &'a str,
}

/// Tokenize a compound selector (no top-level combinators) into
/// classified pieces. Handles balanced parens (`:not(...)`) and
/// brackets (`[attr]`). Anything we don't recognise is bucketed as
/// `Other` and preserved in source order.
fn tokenize_compound(compound: &str) -> Vec<CompoundToken<'_>> {
    let bytes = compound.as_bytes();
    let mut out: Vec<CompoundToken> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        match bytes[i] {
            b'.' => {
                i += 1;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric()
                        || bytes[i] == b'-'
                        || bytes[i] == b'_'
                        || (bytes[i] == b'\\' && i + 1 < bytes.len()))
                {
                    if bytes[i] == b'\\' {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                out.push(CompoundToken {
                    kind: TokenKind::Class,
                    text: &compound[start..i],
                });
            }
            b'#' => {
                i += 1;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b'_')
                {
                    i += 1;
                }
                out.push(CompoundToken {
                    kind: TokenKind::Id,
                    text: &compound[start..i],
                });
            }
            b'[' => {
                let mut depth = 1i32;
                i += 1;
                while i < bytes.len() && depth > 0 {
                    match bytes[i] {
                        b'[' => depth += 1,
                        b']' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
                out.push(CompoundToken {
                    kind: TokenKind::Attr,
                    text: &compound[start..i],
                });
            }
            b':' => {
                let is_element = i + 1 < bytes.len() && bytes[i + 1] == b':';
                if is_element {
                    i += 2;
                } else {
                    i += 1;
                }
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b'_')
                {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b'(' {
                    let mut depth = 1i32;
                    i += 1;
                    while i < bytes.len() && depth > 0 {
                        match bytes[i] {
                            b'(' => depth += 1,
                            b')' => depth -= 1,
                            _ => {}
                        }
                        i += 1;
                    }
                }
                out.push(CompoundToken {
                    kind: if is_element {
                        TokenKind::PseudoElement
                    } else {
                        TokenKind::PseudoClass
                    },
                    text: &compound[start..i],
                });
            }
            b'*' => {
                i += 1;
                out.push(CompoundToken {
                    kind: TokenKind::Tag,
                    text: &compound[start..i],
                });
            }
            b if b.is_ascii_alphabetic() => {
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b'_')
                {
                    i += 1;
                }
                out.push(CompoundToken {
                    kind: TokenKind::Tag,
                    text: &compound[start..i],
                });
            }
            _ => {
                i += 1;
                out.push(CompoundToken {
                    kind: TokenKind::Other,
                    text: &compound[start..i],
                });
            }
        }
    }
    out
}

/// Walk left from `pos` (inclusive of selector content but
/// exclusive of the byte AT `pos` — `pos` is itself part of the
/// compound) until we hit a combinator (`,`, ` `, `>`, `+`, `~`),
/// the start of the string, or an at-rule edge. Returns the index
/// where the compound starts.
fn compound_left_boundary(bytes: &[u8], pos: usize) -> usize {
    let mut i = pos;
    while i > 0 {
        let prev = bytes[i - 1];
        if matches!(prev, b' ' | b'\t' | b',' | b'>' | b'+' | b'~') {
            break;
        }
        i -= 1;
    }
    i
}

/// Walk right from `pos` (inclusive) until we hit a combinator or
/// the end of the string. Skips over balanced `(...)`, `[...]`, and
/// strings so combinators inside them don't end the compound.
fn compound_right_boundary(bytes: &[u8], pos: usize) -> usize {
    let mut i = pos;
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b',' | b'>' | b'+' | b'~' => return i,
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'"' | b'\'' => {
                let q = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != q {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                if i < bytes.len() {
                    i += 1;
                }
            }
            b'(' => {
                let mut depth = 1i32;
                i += 1;
                while i < bytes.len() && depth > 0 {
                    match bytes[i] {
                        b'(' => depth += 1,
                        b')' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
            }
            b'[' => {
                let mut depth = 1i32;
                i += 1;
                while i < bytes.len() && depth > 0 {
                    match bytes[i] {
                        b'[' => depth += 1,
                        b']' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    bytes.len()
}

/// Tag each declaration in `body` (a `prop: value;`-separated string)
/// with `!important`. Used when `@apply !foo` resolves through the
/// user-CSS cache and we need to propagate the bang to every decl
/// produced by the recursive expansion. Skips decls that already
/// carry `!important`.
fn apply_important_to_decls(body: &str) -> String {
    let mut out = String::with_capacity(body.len() + 16);
    for raw in body.split(';') {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        out.push_str(trimmed);
        if !trimmed.ends_with("!important") {
            out.push_str(" !important");
        }
        out.push(';');
    }
    out
}

/// Walk the input CSS and populate `cache` with every class-bearing
/// rule, indexed by class name in the selector. Also collects every
/// token that appears in an `@apply` directive into
/// `cache.applied_classes` so tree-shaking knows to keep user CSS
/// rules whose only "use" is via `@apply`.
///
/// Recurses through `@media`, `@supports`, `@layer`, and similar
/// at-rule blocks so rules nested inside them participate in the
/// cache. Top-level `@keyframes` is skipped (nested rules there
/// aren't apply-able).
pub fn build_user_apply_cache_public(input: &str) -> UserApplyCache {
    let mut cache = UserApplyCache::default();
    walk_for_user_cache(input, &mut cache, &[], false);
    cache
}

fn build_user_apply_cache(input: &str, cache: &mut UserApplyCache) {
    walk_for_user_cache(input, cache, &[], false);
}

fn walk_for_user_cache(
    input: &str,
    cache: &mut UserApplyCache,
    at_chain: &[String],
    in_layer_utility: bool,
) {
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Skip whitespace, comments, strings.
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i = find_comment_end(bytes, i + 2);
            continue;
        }
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            i = find_string_end(bytes, i);
            continue;
        }
        // At-rule: descend if it has a body, else skip.
        if bytes[i] == b'@' {
            let name_start = i + 1;
            let name_end = scan_ident(bytes, name_start);
            let name = &input[name_start..name_end];
            // `@keyframes`/`@-webkit-keyframes` interiors can't be
            // @apply targets — their bodies are percentage-keyed
            // frames, not class rules. Skip outright.
            if name == "keyframes" || name == "-webkit-keyframes" {
                if let Some(after) = skip_at_rule(input, i) {
                    i = after;
                    continue;
                }
            }
            // `@apply` directive: collect each token as an applied class.
            if name == "apply" {
                if let Some(semi) = find_top_level_semicolon(bytes, name_end) {
                    let params = &input[name_end..semi];
                    for tok in params.split_whitespace() {
                        if tok == "!important" {
                            continue;
                        }
                        let trimmed = tok.trim_start_matches('!');
                        // Strip variants — `hover:foo` only contributes
                        // `foo` to the applied set; the variant is
                        // handled at expand time.
                        let class = trimmed.rsplit(':').next().unwrap_or(trimmed);
                        if !class.is_empty() {
                            cache.applied_classes.insert(class.to_string());
                        }
                    }
                    i = semi + 1;
                    continue;
                }
            }
            // Generic at-rule with a `{ }` body — recurse into the
            // body so nested rules participate in the cache. Skip
            // `@layer` wrappers in the at-chain since their effect
            // on selector context is "inline at the layer position",
            // not "wrap in a CSS at-rule" — preserving them in the
            // chain would make `@apply` from inside `@layer
            // components { .foo {…} }` emit `@layer components { .x
            // {…} }` siblings, which isn't how upstream behaves.
            if let Some((after, params, body)) = read_at_rule_with_params_and_block(input, name_end)
            {
                if name == "layer" {
                    let layer_name = params.trim();
                    let nested_in_layer =
                        in_layer_utility || matches!(layer_name, "utilities" | "components");
                    walk_for_user_cache(body, cache, at_chain, nested_in_layer);
                } else if name == "responsive" || name == "variants" {
                    // Deprecated v2 directives. Tailwind treats
                    // them as a no-op wrapper at the directive
                    // level — same as `@layer`, the inside is
                    // inlined at the parent context. Don't add the
                    // wrapper to the at-rule chain. Treat the body
                    // as `in_layer_utility` so variant-applied
                    // candidates (`md:focus:.responsive-at-root`)
                    // resolve through the user-cache fallback in
                    // `compile_candidate`.
                    walk_for_user_cache(body, cache, at_chain, true);
                } else {
                    let header = if params.trim().is_empty() {
                        format!("@{name}")
                    } else {
                        format!("@{} {}", name, params.trim())
                    };
                    let mut next = at_chain.to_vec();
                    next.push(header);
                    walk_for_user_cache(body, cache, &next, in_layer_utility);
                }
                i = after;
                continue;
            }
            // Simple `@x ...;` — skip past the semicolon.
            if let Some(after) = skip_at_rule(input, i) {
                i = after;
                continue;
            }
            i += 1;
            continue;
        }
        // Plain rule: <selector> { <body> }.
        if is_selector_lead(bytes[i]) {
            let start = i;
            let brace = match find_top_level_open_brace(bytes, start) {
                Some(b) => b,
                None => {
                    i += 1;
                    continue;
                }
            };
            let close = match find_matching_close_brace(bytes, brace) {
                Some(c) => c,
                None => {
                    i += 1;
                    continue;
                }
            };
            let selector = input[start..brace].trim();
            let body = &input[brace + 1..close];
            // Index by class name. `selector` may be `span, .b` —
            // collect_class_names returns `["b"]`, which we use as
            // keys. `span` is kept implicitly (the entry's full
            // selector text is preserved on the entry).
            let classes = collect_class_names(selector);
            // Assign a single source-order index per RULE (not per
            // class), so multiple cache entries from the same rule
            // share the same ordering tag.
            cache.next_source_order += 1;
            let source_order = cache.next_source_order;
            for class in &classes {
                let entry = UserApplyEntry {
                    selector: selector.to_string(),
                    body: body.to_string(),
                    at_rules: at_chain.to_vec(),
                    in_layer_utility,
                    source_order,
                };
                cache.entries.entry(class.clone()).or_default().push(entry);
            }
            // Recurse into the body so nested rules (e.g. inside a
            // media query that the user nested via a CSS preprocessor)
            // also participate. With our current pipeline this is
            // mostly a no-op — but it keeps the walk consistent and
            // future-proof.
            walk_for_user_cache(body, cache, at_chain, in_layer_utility);
            i = close + 1;
            continue;
        }
        i += 1;
    }
}

fn collect_class_names(input: &str) -> Vec<String> {
    let bytes = input.as_bytes();
    let mut classes: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'*') => i = find_comment_end(bytes, i + 2),
            b'"' | b'\'' => i = find_string_end(bytes, i),
            b'[' => {
                if let Some(close) = find_matching_close_bracket(bytes, i) {
                    i = close + 1;
                } else {
                    i += 1;
                }
            }
            b':' => {
                // Skip past `:not(...)` — classes inside aren't
                // candidates for `@layer` tree-shaking. Mirrors
                // upstream's `ignoreNot` walk in
                // `setupContextUtils.js extractCandidates`.
                if input[i..].starts_with(":not(") {
                    let open = i + 4;
                    if let Some(close) = find_matching_close_paren(bytes, open) {
                        i = close + 1;
                        continue;
                    }
                }
                i += 1;
            }
            b'{' => {
                // Descend into the body — nested rules' selectors
                // count toward the class set so that
                // `@media (...) { .foo { … } }` keeps the parent
                // child once `foo` is in the candidate set.
                i += 1;
            }
            b'.' => {
                // Reject `.5rem` / `.5em` numeric leads (those are
                // values, not classes). A class identifier starts
                // with a letter, `_`, a non-ASCII char, or a `-`
                // followed by one of those (CSS allows `-foo-1`,
                // disallows `-1foo`/`--var`).
                let next = i + 1;
                if next < bytes.len() && is_class_ident_start(bytes[next]) {
                    let end = scan_class_ident(bytes, next);
                    classes.push(input[next..end].to_string());
                    i = end;
                    continue;
                }
                if next + 1 < bytes.len()
                    && bytes[next] == b'-'
                    && is_class_ident_start(bytes[next + 1])
                {
                    let end = scan_class_ident(bytes, next + 1);
                    classes.push(input[next..end].to_string());
                    i = end;
                    continue;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    classes
}

fn find_matching_close_bracket(bytes: &[u8], open: usize) -> Option<usize> {
    debug_assert_eq!(bytes[open], b'[');
    let mut depth = 1usize;
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            b'"' | b'\'' => i = find_string_end(bytes, i) - 1,
            _ => {}
        }
        i += 1;
    }
    None
}

fn is_class_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || !b.is_ascii()
}

fn scan_class_ident(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || !b.is_ascii() {
            i += 1;
        } else if b == b'\\' && i + 1 < bytes.len() {
            // Escaped character (`.\!flex` -> class `!flex` after the
            // selector escape). Consume the backslash + next byte so
            // the class identifier captures escaped specials.
            i += 2;
        } else {
            break;
        }
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::variants::VariantContext;

    fn cx() -> VariantContext {
        VariantContext::default()
    }

    fn empty_candidates() -> FxHashSet<&'static str> {
        FxHashSet::default()
    }

    #[test]
    fn at_tailwind_utilities_inserts_compiled_css() {
        let input = "body { color: red }\n@tailwind utilities;\n.custom { padding: 1rem }";
        let compiled = ".flex { display: flex }\n";
        let r = process(
            input,
            compiled,
            "",
            &cx(),
            None,
            &empty_candidates(),
            &empty_candidates(),
        );
        assert!(r.css.contains("body { color: red }"));
        assert!(r.css.contains(".flex { display: flex }"));
        assert!(r.css.contains(".custom { padding: 1rem }"));
        assert!(r.diagnostics.is_empty());
    }

    #[test]
    fn at_apply_inlines_static_utility_decls() {
        let input = ".btn { @apply flex hidden; padding: 1rem }";
        let r = process(
            input,
            "",
            "",
            &cx(),
            None,
            &empty_candidates(),
            &empty_candidates(),
        );
        assert!(r.css.contains("display: flex"), "css: {}", r.css);
        assert!(r.css.contains("display: none"), "css: {}", r.css);
        assert!(r.css.contains("padding: 1rem"));
    }

    #[test]
    fn at_apply_with_bang_emits_important() {
        let input = ".btn { @apply !flex; }";
        let r = process(
            input,
            "",
            "",
            &cx(),
            None,
            &empty_candidates(),
            &empty_candidates(),
        );
        assert!(r.css.contains("display: flex !important"), "css: {}", r.css);
    }

    #[test]
    fn at_apply_with_variant_emits_sibling_rule() {
        // Variant-aware `@apply` now emits a sibling rule —
        // `expandApplyAtRules.js` parity, no longer a Phase E
        // carry-over.
        let input = ".btn { @apply hover:flex; }";
        let r = process(
            input,
            "",
            "",
            &cx(),
            None,
            &empty_candidates(),
            &empty_candidates(),
        );
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        assert!(
            r.css.contains(".btn:hover"),
            "expected sibling .btn:hover rule, got: {}",
            r.css
        );
        assert!(
            r.css.contains("display: flex"),
            "expected display: flex in sibling, got: {}",
            r.css
        );
    }

    #[test]
    fn at_apply_with_responsive_variant_wraps_in_media() {
        let input = ".btn { @apply md:flex; }";
        let r = process(
            input,
            "",
            "",
            &cx(),
            None,
            &empty_candidates(),
            &empty_candidates(),
        );
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        assert!(
            r.css.contains("@media (min-width: 768px)"),
            "expected @media wrap, got: {}",
            r.css
        );
        assert!(r.css.contains(".btn"));
        assert!(r.css.contains("display: flex"));
    }

    #[test]
    fn at_apply_unknown_utility_diagnostic() {
        let input = ".btn { @apply not-real; }";
        let r = process(
            input,
            "",
            "",
            &cx(),
            None,
            &empty_candidates(),
            &empty_candidates(),
        );
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].code, "unknown-apply-utility");
    }

    #[test]
    fn at_layer_base_lands_in_tailwind_base_slot() {
        // User `@layer base { … }` is collected and emitted at the
        // matching `@tailwind base;` slot, regardless of source order.
        let cfg = serde_json::json!({ "corePlugins": { "preflight": false } });
        let input = "@layer base { html { color: red } }\n@tailwind base;";
        let r = process(
            input,
            "",
            "",
            &cx(),
            Some(&cfg),
            &empty_candidates(),
            &empty_candidates(),
        );
        assert!(r.css.contains("html"), "css: {}", r.css);
        assert!(r.css.contains("color: red"));
        assert!(!r.css.contains("@layer"));
        // Source order reverse — same result.
        let input2 = "@tailwind base;\n@layer base { html { color: red } }";
        let r2 = process(
            input2,
            "",
            "",
            &cx(),
            Some(&cfg),
            &empty_candidates(),
            &empty_candidates(),
        );
        assert!(r2.css.contains("html"));
        // The user content lands AFTER the cascade-vars block.
        let html_pos = r2.css.find("html").unwrap();
        let cascade_pos = r2.css.find("--tw-translate-x").unwrap();
        assert!(
            cascade_pos < html_pos,
            "user @layer base should land after cascade vars, got: {}",
            r2.css
        );
    }

    #[test]
    fn at_layer_unknown_layer_passes_through() {
        let input = "@layer custom { .x { color: red } }";
        let r = process(
            input,
            "",
            "",
            &cx(),
            None,
            &empty_candidates(),
            &empty_candidates(),
        );
        assert!(r.css.contains("@layer custom"), "css: {}", r.css);
        assert!(r.css.contains(".x"));
    }

    #[test]
    fn at_layer_utilities_tree_shakes_against_candidates() {
        // `super-flex` is in candidates → kept. `unused` is not → dropped.
        let input = "@tailwind utilities;\n@layer utilities { .super-flex { display: flex } .unused { color: red } }";
        let mut cands: FxHashSet<&str> = FxHashSet::default();
        cands.insert("super-flex");
        let r = process(input, "", "", &cx(), None, &cands, &empty_candidates());
        assert!(r.css.contains("super-flex"), "css: {}", r.css);
        assert!(
            !r.css.contains("unused"),
            "should be tree-shaken: {}",
            r.css
        );
    }

    #[test]
    fn screen_call_resolves_inside_at_media() {
        let input = "@media screen(md) { .x { color: red } }";
        let r = process(
            input,
            "",
            "",
            &cx(),
            None,
            &empty_candidates(),
            &empty_candidates(),
        );
        assert!(
            r.css.contains("@media (min-width: 768px)"),
            "css: {}",
            r.css
        );
    }

    #[test]
    fn theme_call_resolves_via_config() {
        let input = ".x { color: theme(\"colors.red.500\"); }";
        let cfg = serde_json::json!({
            "theme": { "colors": { "red": { "500": "#ef4444" } } }
        });
        let r = process(
            input,
            "",
            "",
            &cx(),
            Some(&cfg),
            &empty_candidates(),
            &empty_candidates(),
        );
        assert!(r.css.contains("color: #ef4444"), "css: {}", r.css);
    }

    #[test]
    fn theme_call_unknown_path_emits_diagnostic() {
        let input = ".x { color: theme(\"colors.does.not.exist\"); }";
        let cfg = serde_json::json!({ "theme": {} });
        let r = process(
            input,
            "",
            "",
            &cx(),
            Some(&cfg),
            &empty_candidates(),
            &empty_candidates(),
        );
        assert!(!r.diagnostics.is_empty());
        assert_eq!(r.diagnostics[0].code, "unknown-theme-path");
    }

    #[test]
    fn at_tailwind_base_emits_cascade_vars_and_preflight() {
        // `@tailwind base;` now emits both the `--tw-*` cascade
        // defaults AND the preflight reset (vendored from a
        // one-shot oracle compile against the default config).
        let r = process(
            "@tailwind base;",
            "",
            "",
            &cx(),
            None,
            &empty_candidates(),
            &empty_candidates(),
        );
        assert!(
            r.css.contains("--tw-translate-x: 0"),
            "expected cascade vars, got: {}",
            &r.css[..r.css.len().min(200)]
        );
        assert!(r.css.contains("box-sizing: border-box"));
        assert!(r.css.contains("! tailwindcss v3.4.19"));
    }

    #[test]
    fn at_tailwind_base_skips_preflight_when_disabled() {
        let cfg = serde_json::json!({ "corePlugins": { "preflight": false } });
        let r = process(
            "@tailwind base;",
            "",
            "",
            &cx(),
            Some(&cfg),
            &empty_candidates(),
            &empty_candidates(),
        );
        // Cascade vars still emit (they come from non-preflight
        // corePlugins like transform / filter / ring).
        assert!(r.css.contains("--tw-translate-x: 0"));
        // Preflight banner / reset DO NOT.
        assert!(!r.css.contains("! tailwindcss"));
        assert!(!r.css.contains("box-sizing: border-box"));
    }

    #[test]
    fn theme_call_falls_back_to_second_arg() {
        let input = ".x { border-color: theme(\"borderColor.DEFAULT\", currentColor); }";
        let cfg = serde_json::json!({ "theme": {} });
        let r = process(
            input,
            "",
            "",
            &cx(),
            Some(&cfg),
            &empty_candidates(),
            &empty_candidates(),
        );
        assert!(
            r.css.contains("border-color: currentColor"),
            "css: {}",
            r.css
        );
        // No diagnostic — fallback consumed the lookup miss.
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
    }

    #[test]
    fn at_screen_at_rule_resolves_to_media() {
        let input = "@screen md { .x { color: red } }";
        let r = process(
            input,
            "",
            "",
            &cx(),
            None,
            &empty_candidates(),
            &empty_candidates(),
        );
        assert!(
            r.css.contains("@media (min-width: 768px)"),
            "css: {}",
            r.css
        );
    }
}
