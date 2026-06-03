//! Minimal selector AST + parser/serializer used by `movePseudos`.
//!
//! Direct port of the recursive structure that
//! `vendor/tailwindcss-v3/src/util/pseudoElements.js` operates on
//! (postcss-selector-parser's `Selector`/`Pseudo` shape) — just
//! enough to host the move-to-end algorithm. Other selector
//! work (substitution, sorting) keeps using the byte-level
//! `tokenize_compound` helper because they don't need recursion
//! into pseudo-class subnodes.
//!
//! Serialization preserves the original combinator bytes (single
//! space, ` > `, ` + `, ` ~ `, etc.) so this module's output is
//! byte-equivalent to the input modulo the moved-pseudo nodes.

#[derive(Debug, Clone, Default)]
pub struct SelectorAst {
    pub nodes: Vec<SelAstNode>,
}

#[derive(Debug, Clone)]
pub enum SelAstNode {
    Tag(String),
    Class(String),
    Id(String),
    Attribute(String),
    Pseudo {
        /// Full token including leading colons: `":hover"`,
        /// `"::before"`, `":is"`, etc. Stored without the
        /// argument list — see `args` / `nodes` for that.
        value: String,
        /// Raw arg text including outer parens, used when we
        /// don't model the inner structure (`:nth-child(2n+1)`,
        /// `:lang(en)`, etc.). Empty when there's no argument
        /// or when `nodes` is populated.
        args: String,
        /// Parsed sub-selector list for `:is/:where/:has/:not`.
        /// Empty otherwise.
        nodes: Vec<SelectorAst>,
    },
    /// Combinator characters as they appeared in source — single
    /// space for descendant, `" > "` / `" + "` / `" ~ "` for the
    /// explicit forms (with whatever surrounding whitespace was
    /// present). Preserves output fidelity.
    Combinator(String),
    Universal,
    Nesting,
}

/// Parse a comma-separated selector list.
pub fn parse_selector_list(input: &str) -> Vec<SelectorAst> {
    let mut out = Vec::new();
    for piece in split_top_level_commas(input) {
        out.push(parse_one_selector(piece.trim()));
    }
    out
}

/// Serialize a selector list into CSS text.
pub fn serialize_selector_list(list: &[SelectorAst]) -> String {
    let mut out = String::new();
    for (i, sel) in list.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        serialize_selector_into(sel, &mut out);
    }
    out
}

fn serialize_selector_into(sel: &SelectorAst, out: &mut String) {
    for node in &sel.nodes {
        match node {
            SelAstNode::Tag(s) => out.push_str(s),
            SelAstNode::Class(s) => {
                out.push('.');
                out.push_str(s);
            }
            SelAstNode::Id(s) => {
                out.push('#');
                out.push_str(s);
            }
            SelAstNode::Attribute(s) => out.push_str(s),
            SelAstNode::Pseudo { value, args, nodes } => {
                out.push_str(value);
                if !nodes.is_empty() {
                    out.push('(');
                    for (i, sub) in nodes.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        serialize_selector_into(sub, out);
                    }
                    out.push(')');
                } else if !args.is_empty() {
                    out.push_str(args);
                }
            }
            SelAstNode::Combinator(c) => out.push_str(c),
            SelAstNode::Universal => out.push('*'),
            SelAstNode::Nesting => out.push('&'),
        }
    }
}

fn parse_one_selector(input: &str) -> SelectorAst {
    let bytes = input.as_bytes();
    let mut nodes: Vec<SelAstNode> = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        // Combinator: whitespace and/or `>`/`+`/`~`. Capture the
        // entire whitespace+symbol run as one combinator node so
        // serialization round-trips byte-for-byte.
        if b.is_ascii_whitespace() || matches!(b, b'>' | b'+' | b'~') {
            let start = i;
            // Consume leading whitespace.
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            // Optional explicit combinator symbol.
            if i < bytes.len() && matches!(bytes[i], b'>' | b'+' | b'~') {
                i += 1;
                // Trailing whitespace after the symbol.
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
            }
            // If we're at end-of-input, drop the trailing combinator
            // entirely — it's not legal CSS.
            if i >= bytes.len() {
                break;
            }
            nodes.push(SelAstNode::Combinator(input[start..i].to_string()));
            continue;
        }
        match b {
            b'.' => {
                let start = i + 1;
                let j = read_ident_with_escapes(bytes, start);
                nodes.push(SelAstNode::Class(input[start..j].to_string()));
                i = j;
            }
            b'#' => {
                let start = i + 1;
                let j = read_ident_with_escapes(bytes, start);
                nodes.push(SelAstNode::Id(input[start..j].to_string()));
                i = j;
            }
            b'[' => {
                let mut j = i + 1;
                let mut depth = 1i32;
                while j < bytes.len() && depth > 0 {
                    match bytes[j] {
                        b'\\' if j + 1 < bytes.len() => j += 2,
                        b'[' => {
                            depth += 1;
                            j += 1;
                        }
                        b']' => {
                            depth -= 1;
                            j += 1;
                        }
                        b'"' | b'\'' => {
                            let q = bytes[j];
                            j += 1;
                            while j < bytes.len() && bytes[j] != q {
                                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                                    j += 2;
                                } else {
                                    j += 1;
                                }
                            }
                            if j < bytes.len() {
                                j += 1;
                            }
                        }
                        _ => j += 1,
                    }
                }
                nodes.push(SelAstNode::Attribute(input[i..j].to_string()));
                i = j;
            }
            b':' => {
                // `::` for pseudo-element vs `:` for pseudo-class —
                // both share the AST; the string includes the colons
                // so `pseudo_element_is_*` checks work directly.
                let value_start = i;
                let mut p = i + 1;
                if p < bytes.len() && bytes[p] == b':' {
                    p += 1;
                }
                let name_start = p;
                while p < bytes.len() && (is_ident_byte(bytes[p]) || bytes[p] == b'\\') {
                    if bytes[p] == b'\\' && p + 1 < bytes.len() {
                        p += 2;
                    } else {
                        p += 1;
                    }
                }
                let value = input[value_start..p].to_string();
                let name_lower = input[name_start..p].to_ascii_lowercase();
                // Optional argument list.
                let mut args = String::new();
                let mut sub_nodes: Vec<SelectorAst> = Vec::new();
                if p < bytes.len() && bytes[p] == b'(' {
                    let arg_start = p;
                    let mut q = p + 1;
                    let mut depth = 1i32;
                    while q < bytes.len() && depth > 0 {
                        match bytes[q] {
                            b'\\' if q + 1 < bytes.len() => q += 2,
                            b'(' => {
                                depth += 1;
                                q += 1;
                            }
                            b')' => {
                                depth -= 1;
                                q += 1;
                            }
                            b'"' | b'\'' => {
                                let qc = bytes[q];
                                q += 1;
                                while q < bytes.len() && bytes[q] != qc {
                                    if bytes[q] == b'\\' && q + 1 < bytes.len() {
                                        q += 2;
                                    } else {
                                        q += 1;
                                    }
                                }
                                if q < bytes.len() {
                                    q += 1;
                                }
                            }
                            b'[' => {
                                depth += 1;
                                q += 1;
                            }
                            b']' => {
                                depth -= 1;
                                q += 1;
                            }
                            _ => q += 1,
                        }
                    }
                    let inner = &input[arg_start + 1..q.saturating_sub(1)];
                    if matches!(name_lower.as_str(), "is" | "where" | "has" | "not") {
                        sub_nodes = parse_selector_list(inner);
                    } else {
                        args = input[arg_start..q].to_string();
                    }
                    p = q;
                }
                nodes.push(SelAstNode::Pseudo {
                    value,
                    args,
                    nodes: sub_nodes,
                });
                i = p;
            }
            b'*' => {
                nodes.push(SelAstNode::Universal);
                i += 1;
            }
            b'&' => {
                nodes.push(SelAstNode::Nesting);
                i += 1;
            }
            _ if is_ident_byte(b) || b == b'\\' => {
                let start = i;
                let j = read_ident_with_escapes(bytes, i);
                if j == start {
                    // Unrecognized byte — skip to avoid infinite loop.
                    i += 1;
                } else {
                    nodes.push(SelAstNode::Tag(input[start..j].to_string()));
                    i = j;
                }
            }
            _ => i += 1,
        }
    }
    SelectorAst { nodes }
}

fn read_ident_with_escapes(bytes: &[u8], start: usize) -> usize {
    let mut j = start;
    while j < bytes.len() {
        if bytes[j] == b'\\' && j + 1 < bytes.len() {
            j += 2;
        } else if is_ident_byte(bytes[j]) {
            j += 1;
        } else {
            break;
        }
    }
    j
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

/// Split a selector list on top-level commas (not inside `(`, `[`,
/// or quoted strings). Returns owned slices.
fn split_top_level_commas(input: &str) -> Vec<&str> {
    let bytes = input.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
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
            b',' if depth_paren == 0 && depth_bracket == 0 => {
                parts.push(&input[start..i]);
                start = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    parts.push(&input[start..]);
    parts
}

/// Move pseudo-elements (and any "actionable"-attached pseudo-
/// classes) to the tail of the outermost selector. Direct port of
/// upstream's `movePseudos` in
/// `vendor/tailwindcss-v3/src/util/pseudoElements.js`.
pub fn move_pseudos_in_selector(sel: SelectorAst) -> SelectorAst {
    let (mut sel, hoisted, _) = collect_movable(sel);
    sel.nodes.extend(hoisted);
    sel
}

/// Walk one selector's nodes, returning:
/// - The cleaned selector (with movable pseudos removed from this
///   level AND from any nested `:is/:where/:has/:not` subnodes).
/// - The buffer of nodes that should be appended to the OUTERMOST
///   selector.
/// - The "last seen movable pseudo-element" value at end of walk —
///   propagated upward so an outer `:is(.a::before)` plus a
///   following `:hover` correctly attaches.
fn collect_movable(sel: SelectorAst) -> (SelectorAst, Vec<SelAstNode>, Option<String>) {
    let mut new_nodes: Vec<SelAstNode> = Vec::with_capacity(sel.nodes.len());
    let mut buffer: Vec<SelAstNode> = Vec::new();
    let mut last_seen: Option<String> = None;
    for node in sel.nodes {
        match node {
            SelAstNode::Combinator(c) => {
                // On combinator, drop non-jumpable pseudos from the
                // buffer (they can't cross combinators) and reset
                // last_seen.
                buffer.retain(|n| match n {
                    SelAstNode::Pseudo { value, .. } => is_jumpable(value),
                    _ => true,
                });
                last_seen = None;
                new_nodes.push(SelAstNode::Combinator(c));
            }
            SelAstNode::Pseudo { value, args, nodes } => {
                let movable = is_movable_pseudo_element(&value);
                let attachable = !movable
                    && last_seen.is_some()
                    && is_attachable_pseudo_class(&value, last_seen.as_deref().unwrap());
                if movable {
                    last_seen = Some(value.clone());
                } else if !attachable {
                    last_seen = None;
                }
                // Recurse into subnodes regardless of classification.
                let mut new_subs: Vec<SelectorAst> = Vec::with_capacity(nodes.len());
                for sub in nodes {
                    let (clean_sub, sub_buffer, sub_ls) = collect_movable(sub);
                    new_subs.push(clean_sub);
                    buffer.extend(sub_buffer);
                    if sub_ls.is_some() {
                        last_seen = sub_ls;
                    }
                }
                let pseudo_node = SelAstNode::Pseudo {
                    value,
                    args,
                    nodes: new_subs,
                };
                if movable || attachable {
                    buffer.push(pseudo_node);
                } else {
                    new_nodes.push(pseudo_node);
                }
            }
            other => {
                last_seen = None;
                new_nodes.push(other);
            }
        }
    }
    (SelectorAst { nodes: new_nodes }, buffer, last_seen)
}

/// Pseudo-element-like check. Mirrors upstream's `isPseudoElement`:
/// either `::*` or one of the legacy single-colon
/// pseudo-elements / opaque grouping pseudos in the table.
fn is_pseudo_element_like(value: &str) -> bool {
    value.starts_with("::")
        || matches!(
            value,
            ":after" | ":before" | ":first-letter" | ":first-line" | ":is" | ":where" | ":has"
        )
}

/// True for pseudo-elements with `terminal` in their property
/// list — they're the ones that get hoisted to the tail. Mirrors
/// upstream's `isMovablePseudoElement` plus the `__default__`
/// fallback (unknown `::*` pseudos default to terminal).
fn is_movable_pseudo_element(value: &str) -> bool {
    if !is_pseudo_element_like(value) {
        return false;
    }
    !matches!(
        value,
        ":is" | ":where" | ":has" | "::deep" | "::v-deep" | "::ng-deep"
    )
}

/// True for pseudo-elements with `jumpable` in their property
/// list — they survive a combinator boundary in the move buffer.
fn is_jumpable(value: &str) -> bool {
    matches!(
        value,
        "::after"
            | "::backdrop"
            | "::before"
            | "::first-letter"
            | "::first-line"
            | "::marker"
            | "::placeholder"
            | "::selection"
            | ":after"
            | ":before"
            | ":first-letter"
            | ":first-line"
    )
}

/// Pseudo-class attaches to a preceding pseudo-element when the
/// element is `actionable`. Mirrors upstream's
/// `isAttachablePseudoClass`.
fn is_attachable_pseudo_class(node_value: &str, last_seen_value: &str) -> bool {
    if is_pseudo_element_like(node_value) {
        return false;
    }
    is_actionable_pseudo_element(last_seen_value)
}

/// Pseudo-elements that allow user-action pseudo-classes to
/// attach. Mirrors upstream's `actionable` flag — explicit list
/// for the spec elements that are NOT jumpable, plus the
/// `__default__` fallback (unknown `::*` defaults to actionable).
fn is_actionable_pseudo_element(value: &str) -> bool {
    if !is_pseudo_element_like(value) {
        return false;
    }
    match value {
        // Spec-defined pseudo-elements: actionable iff non-jumpable
        // AND non-empty in the property table.
        "::after" | "::before" | "::backdrop" | "::first-letter" | "::first-line" | "::marker"
        | "::placeholder" | "::selection" => false,
        ":after" | ":before" | ":first-letter" | ":first-line" => false,
        "::cue" | "::cue-region" | "::grammar-error" | "::slotted" | "::spelling-error"
        | "::target-text" => false,
        // Empty property list — not actionable.
        ":is" | ":where" | ":has" => false,
        // Explicitly actionable in upstream's table.
        "::file-selector-button" | "::part" | "::deep" | "::v-deep" | "::ng-deep" => true,
        // Default: anything else (vendor-prefixed `::-webkit-*`,
        // unknown future pseudo-elements) is actionable per
        // upstream's `__default__`.
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(input: &str) -> String {
        let parsed = parse_selector_list(input);
        serialize_selector_list(&parsed)
    }

    #[test]
    fn round_trips_simple() {
        assert_eq!(round_trip(".foo"), ".foo");
        assert_eq!(round_trip(".foo .bar"), ".foo .bar");
        assert_eq!(round_trip(".foo > .bar"), ".foo > .bar");
        assert_eq!(round_trip(".foo:hover"), ".foo:hover");
        assert_eq!(round_trip(".foo::before"), ".foo::before");
    }

    #[test]
    fn round_trips_attribute_and_pseudo_args() {
        assert_eq!(
            round_trip("[data-foo='bar']:nth-child(2n+1)"),
            "[data-foo='bar']:nth-child(2n+1)"
        );
        assert_eq!(round_trip(":is(.a, .b):hover"), ":is(.a, .b):hover");
    }

    fn move_one(input: &str) -> String {
        let list = parse_selector_list(input);
        let moved: Vec<_> = list.into_iter().map(move_pseudos_in_selector).collect();
        serialize_selector_list(&moved)
    }

    #[test]
    fn movepseudos_at_end_no_change() {
        assert_eq!(move_one(".foo::before"), ".foo::before");
    }

    #[test]
    fn movepseudos_hoists_terminal_to_end() {
        assert_eq!(move_one(".foo::before .bar"), ".foo .bar::before");
    }

    #[test]
    fn movepseudos_attaches_actionable_pseudo_class() {
        // `::file-selector-button` is actionable AND terminal —
        // `:hover` following it attaches and moves with it. No
        // combinator separates them so the actionable pseudo-class
        // stays attached.
        assert_eq!(
            move_one(".foo::file-selector-button:hover"),
            ".foo::file-selector-button:hover"
        );
    }

    #[test]
    fn movepseudos_recurses_into_is() {
        // `::before` inside `:is(...)` hoists out to the outer tail.
        assert_eq!(move_one(":is(.foo::before)"), ":is(.foo)::before");
    }
}
