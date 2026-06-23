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

//! CSS nesting expansion — Rust port of `postcss-nested@6.2.0` semantics,
//! matching the algorithm Tailwind's `tailwindcss/nesting` plugin chains
//! into the PostCSS pipeline.
//!
//! Pipeline placement: run BEFORE the directive processor so `@apply`,
//! `@tailwind`, `@layer`, `@screen`, and `theme()` see a flat CSS tree.
//! This way the existing directive machinery never has to learn about
//! selector-nested rules. `@apply` inside a nested rule is passed
//! through as opaque at-rule content here, then resolved downstream
//! against the already-flattened parent selector.
//!
//! Algorithm summary (mirrors
//! `node_modules/postcss-nested/index.js:289-358`):
//!
//! 1. **`&` substitution** — replace `&` literally with the parent
//!    selector (postcss-nested does NOT wrap parent lists in `:is(…)`
//!    — it expands the cross product).
//! 2. **Implicit `&` prefix** — a nested selector with no `&` gets a
//!    descendant join from the parent (`parent + " " + child`).
//!    Leading combinators (`> .b`, `+ .b`, `~ .b`) join the same way.
//! 3. **Suffix concatenation** — `&-foo` substitutes literally so
//!    `.btn { &-primary }` becomes `.btn-primary`. The CSS Nesting
//!    spec rejects this; postcss-nested invented it for Sass-compat.
//! 4. **At-rule bubbling** — `@media`, `@supports`, `@container`,
//!    `@layer`, `@starting-style` bodies move OUT of the nested rule.
//!    The original rule selectors wrap any decl-shaped children
//!    inside the at-rule.
//! 5. **At-rule unwrapping** — `@font-face`, `@keyframes`,
//!    `@document` and prefixed `@keyframes` move out wholesale.
//! 6. **Decl partition** — declarations BEFORE the first nested
//!    rule stay in the original rule. Declarations AFTER nested
//!    rules emit as additional clones of the parent (one clone per
//!    run of decls between nested rules).
//! 7. **Cross-product on lists** — `.a, .b { .x, .y { … } }` expands
//!    to `.a .x, .a .y, .b .x, .b .y { … }`. No `:is()` wrapping.

/// Options for `expand_nesting`. Defaults mirror postcss-nested's
/// out-of-the-box behaviour.
#[derive(Clone, Debug)]
pub struct NestingOptions {
    /// At-rule names whose body bubbles OUT and wraps the parent
    /// rule. Default mirrors postcss-nested 6.2.0 plus `scope` for
    /// upstream parity (`media`, `supports`, `layer`, `container`,
    /// `starting-style`).
    pub bubble: Vec<String>,
    /// At-rule names whose body unwraps — moved out wholesale,
    /// parent dropped. Default: `document`, `font-face`,
    /// `keyframes`, `-webkit-keyframes`, `-moz-keyframes`.
    pub unwrap: Vec<String>,
    /// Preserve empty rules that survive the unwrap. Default false:
    /// rules that consume all their children into siblings get
    /// removed.
    pub preserve_empty: bool,
}

impl Default for NestingOptions {
    fn default() -> Self {
        Self {
            bubble: ["media", "supports", "layer", "container", "starting-style"]
                .into_iter()
                .map(String::from)
                .collect(),
            unwrap: [
                "document",
                "font-face",
                "keyframes",
                "-webkit-keyframes",
                "-moz-keyframes",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            preserve_empty: false,
        }
    }
}

/// Public entry point. Parse `input`, expand nested rules, emit flat
/// CSS. Comments, unrecognized at-rules, and `@apply` directives pass
/// through verbatim. Returns the input unchanged when no nesting is
/// present (fast path keeps cost zero for projects that don't author
/// nested CSS).
pub fn expand_nesting(input: &str, opts: &NestingOptions) -> String {
    if !has_nesting(input) {
        return input.to_string();
    }
    let mut nodes = parse(input);
    let mut out = Vec::with_capacity(nodes.len() * 2);
    for node in nodes.drain(..) {
        expand_node(node, &mut out, opts);
    }
    let mut buf = String::with_capacity(input.len() + input.len() / 8);
    for n in &out {
        emit(n, &mut buf);
    }
    buf
}

/// Quick check: does this CSS source even contain nesting that the
/// expander needs to handle? Cheap byte-scan for either a nested
/// rule (top-level `{` opening followed by a non-decl-like character
/// run that contains another `{` before the closing `}`) or an `&`
/// anywhere. Conservative: when in doubt, parse.
fn has_nesting(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut depth = 0i32;
    let mut at_top_level_in_rule = false;
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
            b'{' => {
                depth += 1;
                at_top_level_in_rule = depth == 1;
                i += 1;
                continue;
            }
            b'}' => {
                depth -= 1;
                at_top_level_in_rule = false;
                i += 1;
                continue;
            }
            b'&' if depth >= 1 => return true,
            b'@' if depth >= 1 => {
                // Nested at-rule — likely needs expansion (bubble/unwrap)
                // unless it's `@apply`/`@tailwind`/`@screen` which we
                // pass through. The cheapest way to know: continue, the
                // full parse will only fire if there really is nesting.
                // We just need to NOT bail here.
                i += 1;
                continue;
            }
            // A `{` inside a rule body at depth 1 means there's a
            // nested rule. (Strings + comments already handled.)
            b => {
                // Detect a possible second `{` while still inside an
                // outer rule body.
                if at_top_level_in_rule && b != b';' && b != b'}' {
                    // Scan ahead briefly for a non-`;` `{` before
                    // hitting the closing `}` at depth 1.
                    let mut j = i;
                    while j < bytes.len() && depth >= 1 {
                        match bytes[j] {
                            b'/' if j + 1 < bytes.len() && bytes[j + 1] == b'*' => {
                                j = find_comment_end(bytes, j + 2);
                            }
                            b'"' | b'\'' => {
                                j = find_string_end(bytes, j);
                            }
                            b'{' => return true,
                            b'}' => {
                                depth -= 1;
                                j += 1;
                                if depth <= 0 {
                                    break;
                                }
                            }
                            _ => j += 1,
                        }
                    }
                    i = j;
                    at_top_level_in_rule = false;
                    continue;
                }
                i += 1;
            }
        }
    }
    false
}

// ---------- AST ----------

#[derive(Clone, Debug)]
enum Node {
    Comment(String),
    Decl {
        property: String,
        value: String,
        important: bool,
    },
    Rule {
        selector: String,
        children: Vec<Node>,
    },
    AtRule {
        name: String,
        params: String,
        children: Option<Vec<Node>>,
    },
    /// Whitespace / passthrough chunks we want to preserve verbatim
    /// (e.g. between top-level rules). Not used for nested content.
    Raw(String),
}

// ---------- Parser ----------

fn parse(input: &str) -> Vec<Node> {
    let bytes = input.as_bytes();
    let mut i = 0;
    parse_block(bytes, &mut i, input, /*top_level=*/ true)
}

fn parse_block(bytes: &[u8], i: &mut usize, src: &str, top_level: bool) -> Vec<Node> {
    let mut out = Vec::new();
    while *i < bytes.len() {
        // Skip whitespace.
        if bytes[*i].is_ascii_whitespace() {
            *i += 1;
            continue;
        }
        // Comment.
        if bytes[*i] == b'/' && *i + 1 < bytes.len() && bytes[*i + 1] == b'*' {
            let end = find_comment_end(bytes, *i + 2);
            out.push(Node::Comment(src[*i..end].to_string()));
            *i = end;
            continue;
        }
        // Closing brace ends a nested block.
        if !top_level && bytes[*i] == b'}' {
            *i += 1;
            return out;
        }
        // At-rule.
        if bytes[*i] == b'@' {
            let node = parse_at_rule(bytes, i, src);
            out.push(node);
            continue;
        }
        // Decl or rule. Scan to the next top-level `{`, `;`, or `}`.
        let start = *i;
        let mut k = *i;
        let mut depth_p = 0i32;
        let mut depth_b = 0i32;
        let mut hit = b'\0';
        while k < bytes.len() {
            match bytes[k] {
                b'/' if k + 1 < bytes.len() && bytes[k + 1] == b'*' => {
                    k = find_comment_end(bytes, k + 2);
                }
                b'"' | b'\'' => {
                    k = find_string_end(bytes, k);
                }
                b'(' => {
                    depth_p += 1;
                    k += 1;
                }
                b')' if depth_p > 0 => {
                    depth_p -= 1;
                    k += 1;
                }
                b'[' => {
                    depth_b += 1;
                    k += 1;
                }
                b']' if depth_b > 0 => {
                    depth_b -= 1;
                    k += 1;
                }
                b'{' | b';' | b'}' if depth_p == 0 && depth_b == 0 => {
                    hit = bytes[k];
                    break;
                }
                _ => k += 1,
            }
        }
        if hit == b'{' {
            // Rule.
            let selector = src[start..k].trim().to_string();
            *i = k + 1;
            let children = parse_block(bytes, i, src, false);
            out.push(Node::Rule { selector, children });
        } else if hit == b';' {
            // Decl.
            let raw = src[start..k].trim();
            if let Some((p, v, important)) = parse_decl(raw) {
                out.push(Node::Decl {
                    property: p,
                    value: v,
                    important,
                });
            } else if !raw.is_empty() {
                // Unparseable — pass through.
                out.push(Node::Raw(raw.to_string()));
            }
            *i = k + 1;
        } else if hit == b'}' {
            // Decl at end of block without trailing semicolon.
            let raw = src[start..k].trim();
            if let Some((p, v, important)) = parse_decl(raw) {
                out.push(Node::Decl {
                    property: p,
                    value: v,
                    important,
                });
            } else if !raw.is_empty() {
                out.push(Node::Raw(raw.to_string()));
            }
            // Don't consume the `}` — outer caller does.
            *i = k;
            if !top_level {
                *i += 1;
                return out;
            }
        } else {
            // EOF reached.
            *i = bytes.len();
            let raw = src[start..k].trim();
            if !raw.is_empty() {
                out.push(Node::Raw(raw.to_string()));
            }
        }
    }
    out
}

fn parse_at_rule(bytes: &[u8], i: &mut usize, src: &str) -> Node {
    debug_assert_eq!(bytes[*i], b'@');
    // Name: `@` then word chars (including `-`).
    let name_start = *i + 1;
    let mut name_end = name_start;
    while name_end < bytes.len()
        && (bytes[name_end].is_ascii_alphanumeric() || bytes[name_end] == b'-')
    {
        name_end += 1;
    }
    let name = src[name_start..name_end].to_string();
    // Params: everything up to the next top-level `;` or `{` (or `}`).
    let mut k = name_end;
    let mut depth_p = 0i32;
    let mut depth_b = 0i32;
    let mut hit = b'\0';
    while k < bytes.len() {
        match bytes[k] {
            b'/' if k + 1 < bytes.len() && bytes[k + 1] == b'*' => {
                k = find_comment_end(bytes, k + 2);
            }
            b'"' | b'\'' => {
                k = find_string_end(bytes, k);
            }
            b'(' => {
                depth_p += 1;
                k += 1;
            }
            b')' if depth_p > 0 => {
                depth_p -= 1;
                k += 1;
            }
            b'[' => {
                depth_b += 1;
                k += 1;
            }
            b']' if depth_b > 0 => {
                depth_b -= 1;
                k += 1;
            }
            b'{' | b';' | b'}' if depth_p == 0 && depth_b == 0 => {
                hit = bytes[k];
                break;
            }
            _ => k += 1,
        }
    }
    let params = src[name_end..k].trim().to_string();
    if hit == b'{' {
        *i = k + 1;
        let children = parse_block(bytes, i, src, false);
        Node::AtRule {
            name,
            params,
            children: Some(children),
        }
    } else if hit == b';' {
        *i = k + 1;
        Node::AtRule {
            name,
            params,
            children: None,
        }
    } else {
        // `}` or EOF: simple at-rule, no body.
        *i = k;
        Node::AtRule {
            name,
            params,
            children: None,
        }
    }
}

fn parse_decl(raw: &str) -> Option<(String, String, bool)> {
    let colon = find_decl_colon(raw)?;
    let property = raw[..colon].trim().to_string();
    if property.is_empty() {
        return None;
    }
    let mut value = raw[colon + 1..].trim().to_string();
    let important = if let Some(stripped) = strip_important(&value) {
        value = stripped.trim_end().to_string();
        true
    } else {
        false
    };
    Some((property, value, important))
}

fn find_decl_colon(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut depth_p = 0i32;
    let mut depth_b = 0i32;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth_p += 1,
            b')' if depth_p > 0 => depth_p -= 1,
            b'[' => depth_b += 1,
            b']' if depth_b > 0 => depth_b -= 1,
            b':' if depth_p == 0 && depth_b == 0 => {
                // Skip CSS pseudo-class double colons like `::before`
                // — but those only appear in selectors, not decls.
                // Inside a decl shape we accept the first colon.
                return Some(i);
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn strip_important(value: &str) -> Option<&str> {
    let trimmed = value.trim_end();
    let lower_tail = trimmed.to_ascii_lowercase();
    if lower_tail.ends_with("!important") {
        Some(&trimmed[..trimmed.len() - "!important".len()])
    } else {
        None
    }
}

// ---------- Expansion ----------

fn expand_node(node: Node, out: &mut Vec<Node>, opts: &NestingOptions) {
    match node {
        Node::Rule { selector, children } => {
            expand_rule(&selector, children, out, opts);
        }
        Node::AtRule {
            name,
            params,
            children,
        } => {
            // Top-level at-rule: recurse into its body (if any) so
            // nested rules inside `@layer foo { … }` etc. also get
            // expanded. The at-rule itself stays put.
            let children = children.map(|kids| {
                let mut nested_out = Vec::new();
                for k in kids {
                    expand_node(k, &mut nested_out, opts);
                }
                nested_out
            });
            out.push(Node::AtRule {
                name,
                params,
                children,
            });
        }
        other => out.push(other),
    }
}

/// The core algorithm. Mirrors postcss-nested's `Rule()` visitor:
/// walk children, emit decl runs into the parent (or clones of the
/// parent), and break out nested rules / bubble at-rules as siblings.
fn expand_rule(
    parent_selector: &str,
    children: Vec<Node>,
    out: &mut Vec<Node>,
    opts: &NestingOptions,
) {
    let parent_list: Vec<String> = split_top_level_commas(parent_selector)
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if parent_list.is_empty() {
        // No selector — degrade to passthrough.
        out.push(Node::Rule {
            selector: parent_selector.to_string(),
            children,
        });
        return;
    }
    // Accumulate "front" decls (before any nested child) into the
    // emitted parent rule. After we hit a nested child, subsequent
    // decls go into a new clone of the parent appended as a sibling.
    let mut front_decls: Vec<Node> = Vec::new();
    // Buffered decls accumulated AFTER a nested child — flushed to a
    // new parent-rule clone the next time we hit another nested item
    // or at the end of children.
    let mut between_decls: Vec<Node> = Vec::new();
    let mut seen_nested = false;
    let mut after_siblings: Vec<Node> = Vec::new();

    for child in children {
        match child {
            Node::Decl { .. } | Node::Comment(_) | Node::Raw(_) => {
                if seen_nested {
                    between_decls.push(child);
                } else {
                    front_decls.push(child);
                }
            }
            Node::Rule {
                selector: child_sel,
                children: child_kids,
            } => {
                // Flush any pending between_decls as a parent-rule clone.
                flush_between_decls(parent_selector, &mut between_decls, &mut after_siblings);
                seen_nested = true;
                let merged = merge_selectors(&parent_list, &child_sel);
                let new_sel = merged.join(", ");
                // Recurse to expand the child rule with the merged
                // selector as the new parent context.
                expand_rule(&new_sel, child_kids, &mut after_siblings, opts);
            }
            Node::AtRule {
                name,
                params,
                children: at_kids,
            } => {
                let is_bubble = opts.bubble.iter().any(|n| n == &name);
                let is_unwrap = opts.unwrap.iter().any(|n| n == &name);
                if (is_bubble || is_unwrap) && at_kids.is_some() {
                    flush_between_decls(parent_selector, &mut between_decls, &mut after_siblings);
                    seen_nested = true;
                    let kids = at_kids.unwrap();
                    if is_bubble {
                        // Wrap the at-rule's decl-shaped children
                        // in a clone of the parent rule, then run
                        // the same nested-rule expansion on the
                        // at-rule's child rules with the parent
                        // selector as context.
                        let bubbled = bubble_at_rule(
                            parent_selector,
                            &parent_list,
                            &name,
                            &params,
                            kids,
                            opts,
                        );
                        after_siblings.push(bubbled);
                    } else {
                        // Unwrap: at-rule body emits at sibling
                        // level, with its decl-shaped children
                        // moved OUT of the at-rule entirely.
                        let unwrapped = Node::AtRule {
                            name,
                            params,
                            children: Some(kids),
                        };
                        after_siblings.push(unwrapped);
                    }
                } else if seen_nested {
                    // A non-bubble/unwrap at-rule (e.g. `@apply`,
                    // `@screen`, custom) sitting after nested
                    // content: treat like a decl — goes into the
                    // next parent clone.
                    between_decls.push(Node::AtRule {
                        name,
                        params,
                        children: at_kids,
                    });
                } else {
                    front_decls.push(Node::AtRule {
                        name,
                        params,
                        children: at_kids,
                    });
                }
            }
        }
    }
    // Emit:
    //   1. parent rule with front_decls (if non-empty OR preserve_empty)
    //   2. all siblings (nested rules and bubbled at-rules) in order
    //   3. between_decls flushed as a final parent clone
    let drop_empty_parent = !opts.preserve_empty && seen_nested && front_decls.is_empty();
    if !drop_empty_parent {
        out.push(Node::Rule {
            selector: parent_selector.to_string(),
            children: front_decls,
        });
    }
    for sib in after_siblings.drain(..) {
        out.push(sib);
    }
    flush_between_decls(parent_selector, &mut between_decls, out);
}

fn flush_between_decls(parent_selector: &str, decls: &mut Vec<Node>, out: &mut Vec<Node>) {
    if decls.is_empty() {
        return;
    }
    out.push(Node::Rule {
        selector: parent_selector.to_string(),
        children: std::mem::take(decls),
    });
}

/// Merge a parent selector list with a child selector. Returns the
/// cross product. Mirrors postcss-nested's `mergeSelectors`.
fn merge_selectors(parent_list: &[String], child_sel: &str) -> Vec<String> {
    let child_list: Vec<&str> = split_top_level_commas(child_sel)
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    let mut out: Vec<String> = Vec::with_capacity(parent_list.len() * child_list.len());
    for p in parent_list {
        for c in &child_list {
            out.push(merge_one(p, c));
        }
    }
    out
}

/// Combine a single parent selector with a single child selector.
/// Three cases mirror postcss-nested:
///   - child contains `&` → replace each `&` with parent
///   - child starts with combinator (`>`, `+`, `~`) → `<parent> <child>`
///   - otherwise (descendant) → `<parent> <child>`
fn merge_one(parent: &str, child: &str) -> String {
    if child.contains('&') {
        // Literal substitution. We don't wrap parent in `:is()` —
        // postcss-nested 6.2.0 doesn't either; the cross product
        // at `merge_selectors` already handles list parents.
        replace_ampersand(child, parent)
    } else {
        // Descendant join. Leading combinators are accommodated by
        // the single space — `parent ` + `> .b` = `parent > .b`.
        let mut s = String::with_capacity(parent.len() + 1 + child.len());
        s.push_str(parent);
        s.push(' ');
        s.push_str(child);
        s
    }
}

/// Replace every top-level `&` (not inside a string) with `parent`.
/// We don't recurse into bracketed pseudo-classes like `:not(&)` —
/// postcss-nested DOES support `&` inside pseudos, so a `&` anywhere
/// (including inside parens) gets substituted.
fn replace_ampersand(s: &str, parent: &str) -> String {
    let mut out = String::with_capacity(s.len() + parent.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' => {
                let end = find_string_end(bytes, i);
                out.push_str(&s[i..end]);
                i = end;
            }
            b'\\' if i + 1 < bytes.len() => {
                out.push('\\');
                out.push(bytes[i + 1] as char);
                i += 2;
            }
            b'&' => {
                out.push_str(parent);
                i += 1;
            }
            b => {
                out.push(b as char);
                i += 1;
            }
        }
    }
    out
}

/// Build the bubbled at-rule node. For each child of the at-rule:
///   - decl/comment → goes into a clone of the parent rule that
///     gets prepended to the at-rule's body
///   - rule → selectors merged with the parent, then recursed
///     through the same expansion (so deep nesting inside @media
///     still flattens)
///   - inner bubble at-rule → recurse with the same parent
fn bubble_at_rule(
    parent_sel: &str,
    parent_list: &[String],
    at_name: &str,
    at_params: &str,
    children: Vec<Node>,
    opts: &NestingOptions,
) -> Node {
    let mut wrapper_decls: Vec<Node> = Vec::new();
    let mut emitted: Vec<Node> = Vec::new();
    for child in children {
        match child {
            Node::Decl { .. } | Node::Comment(_) | Node::Raw(_) => {
                wrapper_decls.push(child);
            }
            Node::AtRule {
                name,
                params,
                children: at_kids,
            } if at_kids.is_some()
                && opts.bubble.iter().any(|n| n == &name)
                && opts.bubble.iter().any(|n| n == at_name) =>
            {
                // Nested @media inside @media etc. — recursively
                // bubble with the same parent rule.
                let inner = bubble_at_rule(
                    parent_sel,
                    parent_list,
                    &name,
                    &params,
                    at_kids.unwrap(),
                    opts,
                );
                emitted.push(inner);
            }
            Node::Rule {
                selector: rule_sel,
                children: rule_kids,
            } => {
                let merged = merge_selectors(parent_list, &rule_sel);
                let new_sel = merged.join(", ");
                expand_rule(&new_sel, rule_kids, &mut emitted, opts);
            }
            other => emitted.push(other),
        }
    }
    let mut at_body: Vec<Node> = Vec::new();
    if !wrapper_decls.is_empty() {
        at_body.push(Node::Rule {
            selector: parent_sel.to_string(),
            children: wrapper_decls,
        });
    }
    for n in emitted.drain(..) {
        at_body.push(n);
    }
    Node::AtRule {
        name: at_name.to_string(),
        params: at_params.to_string(),
        children: Some(at_body),
    }
}

// ---------- Emitter ----------

fn emit(node: &Node, out: &mut String) {
    match node {
        Node::Comment(s) => {
            out.push_str(s);
        }
        Node::Decl {
            property,
            value,
            important,
        } => {
            out.push_str(property);
            out.push(':');
            out.push(' ');
            out.push_str(value);
            if *important {
                out.push_str(" !important");
            }
            out.push(';');
            out.push(' ');
        }
        Node::Rule { selector, children } => {
            out.push_str(selector);
            out.push_str(" { ");
            for c in children {
                emit(c, out);
            }
            out.push('}');
            out.push(' ');
        }
        Node::AtRule {
            name,
            params,
            children,
        } => {
            out.push('@');
            out.push_str(name);
            if !params.is_empty() {
                out.push(' ');
                out.push_str(params);
            }
            match children {
                Some(kids) => {
                    out.push_str(" { ");
                    for c in kids {
                        emit(c, out);
                    }
                    out.push('}');
                    out.push(' ');
                }
                None => {
                    out.push(';');
                    out.push(' ');
                }
            }
        }
        Node::Raw(s) => {
            out.push_str(s);
            out.push(' ');
        }
    }
}

// ---------- Low-level scanners ----------

fn find_comment_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
        i += 1;
    }
    (i + 2).min(bytes.len())
}

fn find_string_end(bytes: &[u8], start: usize) -> usize {
    let q = bytes[start];
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == q {
            return i + 1;
        }
        i += 1;
    }
    i
}

/// Split a selector string on top-level commas (commas not inside
/// brackets/parens/braces or strings).
pub(crate) fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    let bytes = s.as_bytes();
    let mut depth_p = 0i32;
    let mut depth_b = 0i32;
    let mut depth_c = 0i32;
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' => {
                i = find_string_end(bytes, i);
                continue;
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i = find_comment_end(bytes, i + 2);
                continue;
            }
            b'(' => depth_p += 1,
            b')' if depth_p > 0 => depth_p -= 1,
            b'[' => depth_b += 1,
            b']' if depth_b > 0 => depth_b -= 1,
            b'{' => depth_c += 1,
            b'}' if depth_c > 0 => depth_c -= 1,
            b',' if depth_p == 0 && depth_b == 0 && depth_c == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(&s[start..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalize(s: &str) -> String {
        s.replace('\n', " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn case(input: &str, expected: &str) {
        let actual = expand_nesting(input, &NestingOptions::default());
        assert_eq!(normalize(&actual), normalize(expected), "input: {input}");
    }

    #[test]
    fn bare_child_descendant() {
        case(".a { .b { color: red; } }", ".a .b { color: red; }");
    }

    #[test]
    fn ampersand_pseudo() {
        case(
            ".btn { &:hover { color: red; } }",
            ".btn:hover { color: red; }",
        );
    }

    #[test]
    fn ampersand_suffix_concat() {
        case(
            ".btn { &-primary { color: red; } }",
            ".btn-primary { color: red; }",
        );
    }

    #[test]
    fn leading_child_combinator() {
        case(".a { > .b { color: red; } }", ".a > .b { color: red; }");
    }

    #[test]
    fn leading_sibling_combinator() {
        case(".a { + .b { color: red; } }", ".a + .b { color: red; }");
    }

    #[test]
    fn trailing_ampersand() {
        case(
            ".a { .parent & { color: red; } }",
            ".parent .a { color: red; }",
        );
    }

    #[test]
    fn selector_list_with_ampersand() {
        case(
            ".a, .b { &:hover { color: red; } }",
            ".a:hover, .b:hover { color: red; }",
        );
    }

    #[test]
    fn decl_before_nested_emits_parent_first() {
        case(
            ".a { color: red; .b { color: blue; } }",
            ".a { color: red; } .a .b { color: blue; }",
        );
    }

    #[test]
    fn decl_after_nested_emits_parent_clone() {
        case(
            ".a { .b { color: blue; } color: red; }",
            ".a .b { color: blue; } .a { color: red; }",
        );
    }

    #[test]
    fn decl_both_sides() {
        case(
            ".a { color: red; .b { color: blue; } font-size: 10px; }",
            ".a { color: red; } .a .b { color: blue; } .a { font-size: 10px; }",
        );
    }

    #[test]
    fn nested_media_bubbles() {
        case(
            ".a { @media (min-width: 768px) { color: red; } }",
            "@media (min-width: 768px) { .a { color: red; } }",
        );
    }

    #[test]
    fn nested_media_with_inner_rule() {
        case(
            ".a { @media (min-width: 768px) { .b { color: red; } } }",
            "@media (min-width: 768px) { .a .b { color: red; } }",
        );
    }

    #[test]
    fn deeply_nested_three_levels() {
        case(
            ".a { .b { .c { color: red; } } }",
            ".a .b .c { color: red; }",
        );
    }

    #[test]
    fn multiple_nested_siblings() {
        case(
            ".a { .b { color: red; } .c { color: blue; } }",
            ".a .b { color: red; } .a .c { color: blue; }",
        );
    }

    #[test]
    fn font_face_unwraps() {
        case(
            ".a { @font-face { font-family: x; } }",
            "@font-face { font-family: x; }",
        );
    }

    #[test]
    fn apply_in_nested_passes_through() {
        case(
            ".btn { &:hover { @apply text-red; } }",
            ".btn:hover { @apply text-red; }",
        );
    }

    #[test]
    fn multi_class_with_pseudo() {
        case(
            ".foo.bar { &:hover { color: red; } }",
            ".foo.bar:hover { color: red; }",
        );
    }

    #[test]
    fn attribute_suffix() {
        case(
            ".x { &[disabled] { opacity: 0.5; } }",
            ".x[disabled] { opacity: 0.5; }",
        );
    }

    #[test]
    fn pseudo_element() {
        case(
            ".x { &::before { content: \"\"; } }",
            ".x::before { content: \"\"; }",
        );
    }

    #[test]
    fn list_parent_list_child_cross_product() {
        case(
            ".a, .b { .x, .y { color: red; } }",
            ".a .x, .a .y, .b .x, .b .y { color: red; }",
        );
    }

    #[test]
    fn no_nesting_passes_through_unchanged() {
        let input = ".foo { color: red; } .bar { font-size: 14px; }";
        let out = expand_nesting(input, &NestingOptions::default());
        assert_eq!(out, input);
    }

    #[test]
    fn comment_inside_rule_preserved() {
        let out = expand_nesting(
            "/* outside */ .a { /* inside */ .b { color: red; } }",
            &NestingOptions::default(),
        );
        assert!(out.contains("/* outside */"));
        assert!(out.contains("/* inside */"));
        assert!(out.contains(".a .b"));
    }
}
