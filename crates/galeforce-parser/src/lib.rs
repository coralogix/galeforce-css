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

//! Tailwind v3 candidate parser.
//!
//! Splits a raw token like `md:hover:!tw-bg-red-500/50` into structured
//! variants, prefix, important/negative flags, the utility "root" string
//! (everything that isn't variants/flags/modifier), and an optional
//! `/`-modifier. The compiler is responsible for matching `root` against the
//! registered utility set; the parser does not enumerate utilities.
//!
//! Bracket-aware splitting is the key invariant. A colon inside `[...]` is
//! NOT a variant boundary; a slash inside `[...]` is NOT a modifier
//! boundary. This makes `data-[state=open]:hover:bg-red-500/[.31]` parse
//! correctly as variants=[`data-[state=open]`, `hover`], root=`bg-red-500`,
//! modifier=`Arbitrary(".31")`.
//!
//! Important / negative / prefix order matches Tailwind 3:
//! ```text
//!   <variants>:[!][-][prefix]<root>[/modifier]
//! ```
//! e.g. `hover:!-tw-mt-4` is variants=[hover], important=true, negative=true,
//! prefix="tw-", root="mt-4".

#![allow(clippy::doc_markdown)]

mod split;

pub use split::{
    find_top_level_colons, find_top_level_modifier_slash, find_top_level_separator_runs,
};

use smallvec::SmallVec;

/// Structured form of a candidate token. All `&str` fields borrow from
/// the original `raw: &'a str` passed to `parse`. This is the per-build
/// hot path (one ParsedCandidate per token, ~2,500+ on a real corpus);
/// we previously allocated 5–7 Strings per parse, now we allocate
/// zero. The `&'a str` slices are taken directly from `raw`.
///
/// No `Serialize`/`Deserialize` — ParsedCandidate is internal to the
/// compiler crate and never crosses the JSON boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedCandidate<'a> {
    pub raw: &'a str,
    pub variants: SmallVec<[ParsedVariant<'a>; 2]>,
    pub important: bool,
    pub negative: bool,
    /// Configured prefix (e.g. "tw-") that was consumed from the candidate.
    /// `None` means either the candidate had no prefix or the parser was
    /// invoked with no prefix configured. Borrows from `ParseOptions.prefix`.
    pub prefix: Option<&'a str>,
    /// The utility expression that survives variant/flag/modifier stripping.
    /// For `hover:!tw-bg-red-500/50` this is `"bg-red-500"`. For `flex` it's
    /// `"flex"`. For `bg-[#fff]` it's `"bg-[#fff]"`.
    pub root: &'a str,
    pub modifier: Option<Modifier<'a>>,
}

/// A single variant segment, in source order (left-to-right).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedVariant<'a> {
    /// e.g. `hover`, `md`, `dark`, `group-hover`, `data-[state=open]`,
    /// `group-hover/sidebar`, `supports-[display:grid]`.
    Named(&'a str),
    /// `[&>*]`-style arbitrary selector. The string is the inner selector
    /// (no surrounding brackets): `&>*`, `&:nth-child(3)`, `html:has(&)`.
    Arbitrary(&'a str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Modifier<'a> {
    /// e.g. `/50` -> `Named("50")`.
    Named(&'a str),
    /// e.g. `/[.31]` -> `Arbitrary(".31")`.
    Arbitrary(&'a str),
}

#[derive(Clone, Debug)]
pub struct ParseOptions<'a> {
    /// Tailwind config `prefix`. Empty / `None` means no prefix configured.
    pub prefix: Option<&'a str>,
    /// Variant separator. Tailwind defaults to `:`; users can override
    /// to `__` etc. for templating systems that can't put `:` inside a
    /// class attribute. Multi-byte separators are supported.
    pub separator: &'a str,
}

impl<'a> Default for ParseOptions<'a> {
    fn default() -> Self {
        Self {
            prefix: None,
            separator: ":",
        }
    }
}

/// Parse a single token into a `ParsedCandidate`. Returns `None` for tokens
/// the parser refuses to interpret (e.g. empty after flag stripping). The
/// "is this a real Tailwind utility?" question is the compiler's; this
/// function only does syntactic decomposition.
///
/// Returns a `ParsedCandidate<'a>` whose `&str` fields all borrow from
/// `raw` (or `opts.prefix` for the prefix field). Zero allocations on
/// the parse hot path.
pub fn parse<'a>(raw: &'a str, opts: &ParseOptions<'a>) -> Option<ParsedCandidate<'a>> {
    if raw.is_empty() {
        return None;
    }

    // 1. Split on top-level separator (default `:`) to separate
    // variants from the utility segment.
    let segments = split_segments(raw, opts.separator);
    if segments.is_empty() {
        return None;
    }

    let utility_segment = segments.last().copied()?;
    let variant_segments = &segments[..segments.len() - 1];

    let variants: SmallVec<[ParsedVariant<'a>; 2]> = variant_segments
        .iter()
        .map(|s| classify_variant(s))
        .collect();

    // 2. Decompose utility segment: !, prefix, -, root, /modifier
    //
    // Order matches Tailwind v3: important goes first. Then either:
    //   `<prefix><-><root>` — class `.tw--mx-0` (prefix-then-negative)
    //   `<-><prefix><root>` — class `.-tw-mx-0` (negative-then-prefix)
    // Tailwind v3 accepts BOTH orderings: the candidate string is the
    // class name verbatim, and a user can write either form in their
    // markup. We try prefix-first because it's the more common form
    // in `prefix: 'tw-'` configs, then fall back to negative-first.
    let mut rest = utility_segment;

    let important = if let Some(stripped) = rest.strip_prefix('!') {
        rest = stripped;
        true
    } else {
        false
    };

    let mut consumed_prefix: Option<&'a str> = None;
    let mut negative = false;

    // Arbitrary-property syntax (`[appearance:textfield]`) is exempt
    // from the prefix requirement — the entire utility is the
    // declaration in brackets, not a utility class name. Tailwind v3
    // accepts these regardless of the configured `prefix`. Detect
    // by leading `[` (after important-stripping).
    let is_arbitrary_property = rest.starts_with('[');

    if !is_arbitrary_property {
        if let Some(prefix) = opts.prefix.filter(|p| !p.is_empty()) {
            if let Some(stripped) = rest.strip_prefix(prefix) {
                // <prefix><...> — prefix-then-(maybe negative)
                consumed_prefix = Some(prefix);
                rest = stripped;
                if let Some(s) = rest.strip_prefix('-') {
                    negative = true;
                    rest = s;
                }
            } else if let Some(after_neg) = rest.strip_prefix('-') {
                if let Some(stripped) = after_neg.strip_prefix(prefix) {
                    // <-><prefix><root> — negative-then-prefix.
                    negative = true;
                    consumed_prefix = Some(prefix);
                    rest = stripped;
                } else {
                    // Leading `-` but no prefix follows — Tailwind v3
                    // with a configured prefix rejects this.
                    return None;
                }
            } else {
                // Prefix is configured but the candidate doesn't carry
                // it — Tailwind v3 treats this as a non-utility and
                // skips compilation. Returning None means `.fixed`,
                // `.invisible` etc. don't leak into a `prefix:'tw-'`
                // project's CSS.
                return None;
            }
            // After stripping the prefix, if what remains is itself
            // arbitrary-property syntax (`[prop:val]`), reject. Tailwind v3
            // doesn't run arbitrary properties through the prefix
            // pipeline — `tw-[font:var(...)]` is treated as a non-
            // utility, only the bare `[font:var(...)]` form emits.
            // Without this guard we'd leak `.tw-[prop:val]` rules
            // into the output.
            if rest.starts_with('[') {
                return None;
            }
        } else if let Some(stripped) = rest.strip_prefix('-') {
            // No prefix configured — just check negative.
            negative = true;
            rest = stripped;
        }
    }

    // 3. Modifier split (/ at top bracket-depth).
    let (root, modifier) = match find_top_level_modifier_slash(rest) {
        Some(idx) => {
            let (root, after) = rest.split_at(idx);
            // after starts with '/', skip it
            let modifier_str = &after[1..];
            // Empty modifier (`text-lg/`) is invalid — a trailing
            // slash with nothing after produces no meaningful
            // utility. Tailwind drops these candidates.
            if modifier_str.is_empty() {
                return None;
            }
            (root, Some(classify_modifier(modifier_str)))
        }
        None => (rest, None),
    };

    if root.is_empty() {
        return None;
    }

    Some(ParsedCandidate {
        raw,
        variants,
        important,
        negative,
        prefix: consumed_prefix,
        root,
        modifier,
    })
}

fn split_segments<'a>(raw: &'a str, sep: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, len) in find_top_level_separator_runs(raw, sep) {
        out.push(&raw[start..i]);
        start = i + len;
    }
    out.push(&raw[start..]);
    out
}

fn classify_variant(seg: &str) -> ParsedVariant<'_> {
    // A pure-arbitrary variant is exactly `[...]` (entire segment is one
    // bracket pair). Anything else — including `data-[state=open]` or
    // `group-hover/sidebar` — is Named.
    if seg.len() >= 2 && seg.starts_with('[') && seg.ends_with(']') {
        let inner = &seg[1..seg.len() - 1];
        ParsedVariant::Arbitrary(inner)
    } else {
        ParsedVariant::Named(seg)
    }
}

fn classify_modifier(s: &str) -> Modifier<'_> {
    if s.len() >= 2 && s.starts_with('[') && s.ends_with(']') {
        Modifier::Arbitrary(&s[1..s.len() - 1])
    } else {
        Modifier::Named(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_simple(raw: &str) -> ParsedCandidate {
        parse(raw, &ParseOptions::default()).unwrap_or_else(|| panic!("parse failed: {raw}"))
    }

    #[test]
    fn bare_static_utility() {
        let c = parse_simple("flex");
        assert_eq!(c.root, "flex");
        assert!(c.variants.is_empty());
        assert!(!c.important);
        assert!(!c.negative);
        assert!(c.prefix.is_none());
        assert!(c.modifier.is_none());
    }

    #[test]
    fn single_variant() {
        let c = parse_simple("hover:bg-red-500");
        assert_eq!(c.variants.as_slice(), &[ParsedVariant::Named("hover")]);
        assert_eq!(c.root, "bg-red-500");
    }

    #[test]
    fn two_variants_in_order() {
        let c = parse_simple("md:hover:bg-red-500");
        assert_eq!(
            c.variants.as_slice(),
            &[ParsedVariant::Named("md"), ParsedVariant::Named("hover"),],
        );
        assert_eq!(c.root, "bg-red-500");
    }

    #[test]
    fn three_variants_dark_md_hover() {
        let c = parse_simple("dark:md:hover:bg-red-500");
        assert_eq!(c.variants.len(), 3);
        assert_eq!(c.root, "bg-red-500");
    }

    #[test]
    fn important_only() {
        let c = parse_simple("!mt-4");
        assert!(c.important);
        assert!(!c.negative);
        assert_eq!(c.root, "mt-4");
    }

    #[test]
    fn negative_only() {
        let c = parse_simple("-mt-4");
        assert!(c.negative);
        assert!(!c.important);
        assert_eq!(c.root, "mt-4");
    }

    #[test]
    fn important_and_negative() {
        let c = parse_simple("!-mt-4");
        assert!(c.important);
        assert!(c.negative);
        assert_eq!(c.root, "mt-4");
    }

    #[test]
    fn prefix_strips_when_configured() {
        let c = parse(
            "tw-mt-4",
            &ParseOptions {
                prefix: Some("tw-"),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(c.prefix, Some("tw-"));
        assert_eq!(c.root, "mt-4");
    }

    #[test]
    fn prefix_no_op_when_not_configured() {
        let c = parse_simple("tw-mt-4");
        assert!(c.prefix.is_none());
        assert_eq!(c.root, "tw-mt-4");
    }

    #[test]
    fn prefix_allows_arbitrary_property_syntax() {
        // `[appearance:textfield]` is Tailwind v3's "arbitrary CSS
        // property" form — the entire candidate IS a CSS declaration
        // wrapped in brackets, not a utility class. Tailwind allows
        // these even when `prefix:'tw-'` is configured (they don't
        // route through the utility-class pipeline). Our prefix
        // strictness must exempt them.
        let opts = ParseOptions {
            prefix: Some("tw-"),
            ..Default::default()
        };
        let c = parse("[appearance:textfield]", &opts).expect("arbitrary prop must parse");
        assert!(c.prefix.is_none());
        assert_eq!(c.root, "[appearance:textfield]");
        // With important too.
        let c = parse("![contain:strict]", &opts).expect("!arbitrary prop must parse");
        assert!(c.important);
        assert_eq!(c.root, "[contain:strict]");
    }

    #[test]
    fn prefix_rejects_prefixed_arbitrary_property() {
        // Tailwind v3 doesn't apply `prefix` to arbitrary-property
        // syntax. Only the bare `[font:var(...)]` form compiles
        // when `prefix: 'tw-'` is configured; `tw-[font:var(...)]`
        // is treated as a non-utility and skipped. (Confirmed by
        // running tailwindcss@3.3.2 against both candidates — only
        // the unprefixed form emits a rule.) Our parser must reject
        // the prefixed form so we don't leak `.tw-[prop:val]`
        // rules into the output.
        let opts = ParseOptions {
            prefix: Some("tw-"),
            ..Default::default()
        };
        assert!(parse("tw-[font:var(--f-paragraph-bold)]", &opts).is_none());
        assert!(parse("tw-[text-wrap:pretty]", &opts).is_none());
        assert!(parse("tw-[appearance:none]", &opts).is_none());
    }

    #[test]
    fn prefix_rejects_unprefixed_candidate() {
        // When `prefix: 'tw-'` is configured, Tailwind v3 ignores
        // utility-like candidates that lack the prefix. `flex` would
        // NOT compile to `.flex { display: flex }` — only `tw-flex`
        // does. Our parser must mirror that by returning None for
        // unprefixed candidates so they don't leak into the output.
        let opts = ParseOptions {
            prefix: Some("tw-"),
            ..Default::default()
        };
        assert!(parse("flex", &opts).is_none());
        assert!(parse("fixed", &opts).is_none());
        assert!(parse("invisible", &opts).is_none());
        assert!(parse("mt-4", &opts).is_none());
        assert!(
            parse("!fixed", &opts).is_none(),
            "important + unprefixed must reject"
        );
    }

    #[test]
    fn prefix_with_negative_inside() {
        // Tailwind v3 accepts `tw--mx-0` = prefix `tw-` + negative `-`
        // + root `mx-0`. The class name is preserved as `tw--mx-0`.
        let c = parse(
            "tw--mx-0",
            &ParseOptions {
                prefix: Some("tw-"),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(c.prefix, Some("tw-"));
        assert!(c.negative);
        assert_eq!(c.root, "mx-0");
        assert_eq!(c.raw, "tw--mx-0", "raw preserves the literal class name");
    }

    #[test]
    fn prefix_with_negative_outside() {
        // Tailwind v3 ALSO accepts `-tw-mx-0` = negative `-` + prefix
        // `tw-` + root `mx-0`. The class name is preserved as
        // `-tw-mx-0`. Both forms compile to identical declarations
        // (the negative is applied either way) but the class names
        // differ verbatim.
        let c = parse(
            "-tw-mx-0",
            &ParseOptions {
                prefix: Some("tw-"),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(c.prefix, Some("tw-"));
        assert!(c.negative);
        assert_eq!(c.root, "mx-0");
        assert_eq!(c.raw, "-tw-mx-0", "raw preserves the literal class name");
    }

    #[test]
    fn prefix_with_important_and_negative() {
        // `!tw--mx-0` — important + prefix + negative + root.
        let c = parse(
            "!tw--mx-0",
            &ParseOptions {
                prefix: Some("tw-"),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(c.important);
        assert_eq!(c.prefix, Some("tw-"));
        assert!(c.negative);
        assert_eq!(c.root, "mx-0");
    }

    #[test]
    fn prefix_with_leading_dash_but_no_prefix_after_rejects() {
        // `-flex` with `prefix: 'tw-'` configured — the leading `-`
        // is followed by `flex`, not by `tw-`. There's no valid
        // prefixed utility here, so reject.
        let opts = ParseOptions {
            prefix: Some("tw-"),
            ..Default::default()
        };
        assert!(parse("-flex", &opts).is_none());
        assert!(parse("-mx-0", &opts).is_none());
    }

    #[test]
    fn variant_important_prefix() {
        let c = parse(
            "hover:!tw-bg-red-500",
            &ParseOptions {
                prefix: Some("tw-"),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(c.variants.as_slice(), &[ParsedVariant::Named("hover")]);
        assert!(c.important);
        assert_eq!(c.prefix, Some("tw-"));
        assert_eq!(c.root, "bg-red-500");
    }

    #[test]
    fn slash_modifier_named() {
        let c = parse_simple("bg-red-500/50");
        assert_eq!(c.root, "bg-red-500");
        assert_eq!(c.modifier, Some(Modifier::Named("50")));
    }

    #[test]
    fn slash_modifier_text_sm_6() {
        let c = parse_simple("text-sm/6");
        assert_eq!(c.root, "text-sm");
        assert_eq!(c.modifier, Some(Modifier::Named("6")));
    }

    #[test]
    fn slash_modifier_arbitrary() {
        let c = parse_simple("border-blue-500/[.31]");
        assert_eq!(c.root, "border-blue-500");
        assert_eq!(c.modifier, Some(Modifier::Arbitrary(".31")));
    }

    #[test]
    fn arbitrary_value_with_calc() {
        let c = parse_simple("w-[calc(100%-1rem)]");
        assert_eq!(c.root, "w-[calc(100%-1rem)]");
        assert!(c.modifier.is_none());
    }

    #[test]
    fn arbitrary_value_with_hex() {
        let c = parse_simple("bg-[#123456]");
        assert_eq!(c.root, "bg-[#123456]");
    }

    #[test]
    fn arbitrary_value_with_type_hint() {
        let c = parse_simple("text-[color:var(--brand)]");
        // Slash inside brackets is NOT a modifier boundary (there is no
        // slash here, but we want to confirm the bracket-aware split).
        assert_eq!(c.root, "text-[color:var(--brand)]");
        assert!(c.modifier.is_none());
    }

    #[test]
    fn arbitrary_variant() {
        let c = parse_simple("[&>*]:mt-2");
        assert_eq!(c.variants.as_slice(), &[ParsedVariant::Arbitrary("&>*")],);
        assert_eq!(c.root, "mt-2");
    }

    #[test]
    fn supports_arbitrary_variant_is_named() {
        // `supports-[display:grid]` is a NAMED variant whose name happens to
        // contain a bracketed value. The colon between `:grid` and the rest
        // separates variant from utility, but the colon inside `[...]` is
        // protected.
        let c = parse_simple("supports-[display:grid]:grid");
        assert_eq!(
            c.variants.as_slice(),
            &[ParsedVariant::Named("supports-[display:grid]")],
        );
        assert_eq!(c.root, "grid");
    }

    #[test]
    fn data_attribute_variant() {
        let c = parse_simple("data-[state=open]:block");
        assert_eq!(
            c.variants.as_slice(),
            &[ParsedVariant::Named("data-[state=open]")],
        );
        assert_eq!(c.root, "block");
    }

    #[test]
    fn named_group_variant_with_slash() {
        // `group-hover/sidebar` keeps its slash — it's part of the variant
        // name, not a utility-level modifier.
        let c = parse_simple("group-hover/sidebar:block");
        assert_eq!(
            c.variants.as_slice(),
            &[ParsedVariant::Named("group-hover/sidebar")],
        );
        assert_eq!(c.root, "block");
    }

    #[test]
    fn arbitrary_value_with_modifier() {
        let c = parse_simple("bg-[#123]/50");
        assert_eq!(c.root, "bg-[#123]");
        assert_eq!(c.modifier, Some(Modifier::Named("50")));
    }

    #[test]
    fn empty_returns_none() {
        assert!(parse("", &ParseOptions::default()).is_none());
    }

    #[test]
    fn empty_after_stripping_returns_none() {
        // `!` alone strips to empty -> reject.
        assert!(parse("!", &ParseOptions::default()).is_none());
    }
}
