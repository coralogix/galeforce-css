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

//! Variant resolution and application.
//!
//! See `todo.md` § 14 and Tailwind 3.4.19 `corePlugins.js` for variant
//! definitions. We mirror the JS source: each variant either modifies the
//! selector (via a "format" string containing `&`) or wraps the rule in an
//! at-rule. Some variants emit multiple selector formats — those produce
//! multiple `Rule`s rather than one rule with comma-joined selectors,
//! because the conformance normalizer compares rules by `(at-rule context,
//! selector)` key and treats `.a, .b` as a different key than `.a` plus `.b`.
//!
//! ## Composition order
//!
//! Variants are listed left-to-right in the candidate (`md:hover:flex` →
//! `[md, hover]`). For chained selector variants, the LATER variant in
//! source-order ends up CLOSER to the class in the final selector — e.g.
//! `hover:focus:flex` produces `.<class>:focus:hover`. This matches the
//! oracle and is achieved by iterating variants in **reverse source-order**
//! and substituting `&` in each variant's format with the running selector.
//!
//! At-rules accumulate **outermost-first** in source-order: `dark:md:hover`
//! emits `@media (prefers-color-scheme: dark) { @media (min-width: 768px)
//! { …:hover { … } } }`. Because we iterate variants in reverse, we prepend
//! to the at-rule list as we encounter at-rule variants — the source-first
//! variant (encountered last in our reverse loop) ends up at index 0.

#![allow(clippy::doc_markdown)]

use galeforce_css::escape_class_name;
use galeforce_parser::ParsedVariant;
use serde_json::Value;

use crate::screens::{default_screens, resolve_screens, Screen};

#[derive(Clone, Debug)]
pub struct VariantContext {
    pub dark_mode: DarkMode,
    pub screens: Vec<Screen>,
    pub prefix: String,
    pub aria_shortcuts: Vec<(String, String)>,
    pub data_shortcuts: Vec<(String, String)>,
    pub supports_shortcuts: Vec<(String, String)>,
    /// User-defined variants captured from `addVariant()` plugin
    /// calls. Keyed by variant name. Looked up after the built-in
    /// match so users can register new variants without name
    /// collisions affecting built-ins.
    pub plugin_variants: Vec<crate::plugins::PluginVariant>,
    /// True when the screens config is "complex" (any object-form
    /// screen) or "mixed-units" (string screens use more than one
    /// unit, e.g. mixing `px` and `rem`). Both conditions disable
    /// arbitrary `min-[<value>]` and `max-[<value>]` variants —
    /// upstream warns and skips emission. Mirrors the
    /// `complex-screen-config` / `mixed-screen-units` /
    /// `minmax-have-mixed-units` warnings in upstream's
    /// `setupContextUtils.js`.
    pub disable_minmax_arbitrary_screen: bool,
    /// `future.hoverOnlyWhenSupported: true` wraps `:hover` (and
    /// `group-hover:`, `peer-hover:`) variants in
    /// `@media (hover: hover) and (pointer: fine)` so devices that
    /// don't actually support hover (touchscreens) skip the rule.
    /// Mirrors upstream's `flagEnabled('hoverOnlyWhenSupported')`
    /// branch in `corePlugins.js`.
    pub hover_only_when_supported: bool,
    /// The unit (`px`, `rem`, etc.) the `min-[<v>]` / `max-[<v>]`
    /// arbitrary variant is allowed to emit for the current build.
    /// `None` means "no restriction" — use this when there's only
    /// one unit across screens + min-/max- arbitrary candidates.
    /// Mirrors upstream's `canUseUnits` cache: the FIRST unit seen
    /// when iterating candidates in alphabetical order wins; later
    /// candidates with a different unit are dropped (with a
    /// `minmax-have-mixed-units` warning upstream).
    pub minmax_allowed_unit: Option<String>,
    /// True when the input CSS contains `@tailwind base;`. The
    /// `experimental.optimizeUniversalDefaults` cascade-var
    /// inlining only kicks in when the universal defaults block
    /// would have emitted — without `@tailwind base` there's
    /// nothing to inline.
    pub has_tailwind_base: bool,
    /// `tailwindVersion: '3.3'` compat mode. Set from
    /// `config.__tailwindVersion` (injected by `compile()` when
    /// `features.compat.tailwind_version == V33`). Currently gates
    /// the attribute-value quoting in `data-[k=v]` / `aria-[k=v]` /
    /// `supports-[k=v]` selectors: v3.3 emits `[data-k=v]`
    /// (unquoted), v3.4 emits `[data-k="v"]`.
    pub is_v33_compat: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DarkMode {
    /// `darkMode: 'media'` — Tailwind's default — `@media (prefers-color-scheme: dark)`.
    Media,
    /// `darkMode: 'class'` — old-style — `&:is(<sel> *)`. Default selector `.dark`.
    Class(String),
    /// `darkMode: 'selector'` — newer — `&:where(<sel>, <sel> *)`. Default selector `.dark`.
    Selector(String),
    /// `darkMode: 'variant'` — fully custom — caller supplies one or more
    /// `&`-bearing format strings.
    Variant(Vec<String>),
}

impl Default for VariantContext {
    fn default() -> Self {
        Self {
            dark_mode: DarkMode::Media,
            screens: default_screens(),
            prefix: String::new(),
            aria_shortcuts: default_aria_shortcuts(),
            data_shortcuts: Vec::new(),
            supports_shortcuts: Vec::new(),
            plugin_variants: Vec::new(),
            disable_minmax_arbitrary_screen: false,
            hover_only_when_supported: false,
            minmax_allowed_unit: None,
            has_tailwind_base: false,
            is_v33_compat: false,
        }
    }
}

impl VariantContext {
    pub fn from_config(config: Option<&Value>) -> Self {
        let mut cx = VariantContext::default();
        let Some(cfg) = config else {
            return cx;
        };
        if let Some(dm) = cfg.get("darkMode") {
            cx.dark_mode = parse_dark_mode(dm);
        }
        cx.screens = resolve_screens(cfg);
        cx.disable_minmax_arbitrary_screen = detect_complex_or_mixed_screens(cfg);
        cx.hover_only_when_supported = cfg
            .get("future")
            .and_then(|f| f.get("hoverOnlyWhenSupported"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if let Some(p) = cfg.get("prefix").and_then(Value::as_str) {
            cx.prefix = p.to_string();
        }
        if let Some(theme) = cfg.get("theme") {
            // theme.aria / theme.data / theme.supports — explicit replaces,
            // theme.extend.* merges on top.
            if let Some(aria) = theme.get("aria").and_then(Value::as_object) {
                cx.aria_shortcuts = read_kv(aria);
            } else if let Some(aria_ext) = theme
                .get("extend")
                .and_then(|e| e.get("aria"))
                .and_then(Value::as_object)
            {
                merge_kv(&mut cx.aria_shortcuts, aria_ext);
            }
            if let Some(data) = theme.get("data").and_then(Value::as_object) {
                cx.data_shortcuts = read_kv(data);
            } else if let Some(data_ext) = theme
                .get("extend")
                .and_then(|e| e.get("data"))
                .and_then(Value::as_object)
            {
                merge_kv(&mut cx.data_shortcuts, data_ext);
            }
            if let Some(supports) = theme.get("supports").and_then(Value::as_object) {
                cx.supports_shortcuts = read_kv(supports);
            } else if let Some(supports_ext) = theme
                .get("extend")
                .and_then(|e| e.get("supports"))
                .and_then(Value::as_object)
            {
                merge_kv(&mut cx.supports_shortcuts, supports_ext);
            }
        }
        // Pull plugin variants out of `__pluginOutput.variants` if any.
        cx.plugin_variants = crate::plugins::read_plugin_output(Some(cfg)).variants;
        cx.is_v33_compat = cfg.get("__tailwindVersion").and_then(Value::as_str) == Some("3.3");
        cx
    }

    /// Pre-pass over the candidate set: collect every unit used by
    /// `min-[<v>]` / `max-[<v>]` arbitrary variant. If multiple
    /// units are present (and the screens config doesn't already
    /// pin one), the FIRST unit when candidates are sorted
    /// alphabetically wins — matching upstream's `canUseUnits`
    /// behavior, where Tailwind iterates over the candidate Set in
    /// the order generated by `setupContextUtils.js`'s screen-
    /// variant matcher (which sorts candidates internally before
    /// invoking the fn).
    pub fn compute_minmax_allowed_unit<I: IntoIterator<Item = S>, S: AsRef<str>>(
        &mut self,
        candidates: I,
    ) {
        // Collect existing screen units; if exactly one, that's the
        // anchor and any candidate must match it (already handled
        // in `candidate_unit_matches_screens`). If there are zero,
        // we let the candidate units decide.
        let mut screen_units: std::collections::HashSet<&'static str> =
            std::collections::HashSet::new();
        for s in &self.screens {
            if s.is_raw {
                continue;
            }
            if let Some(u) = extract_length_unit(&s.value) {
                screen_units.insert(u);
            }
        }
        if screen_units.len() == 1 {
            // The single screen unit IS the anchor — no need to
            // pre-pick from candidates.
            return;
        }
        // Gather candidate-side units. Walk every variant segment of
        // every candidate looking for `min-[<v>]` / `max-[<v>]`.
        // Sort candidates alphabetically so the first-wins rule is
        // deterministic and matches upstream's iteration order.
        let mut sorted: Vec<String> = candidates
            .into_iter()
            .map(|s| s.as_ref().to_string())
            .collect();
        sorted.sort();
        let mut first_unit: Option<&'static str> = None;
        for cand in &sorted {
            for seg in cand.split(':') {
                let arb = if let Some(rest) = seg.strip_prefix("min-") {
                    arbitrary_inner(rest)
                } else if let Some(rest) = seg.strip_prefix("max-") {
                    arbitrary_inner(rest)
                } else {
                    None
                };
                let Some(arb) = arb else { continue };
                if let Some(u) = extract_length_unit(arb) {
                    if first_unit.is_none() {
                        first_unit = Some(u);
                    }
                }
            }
        }
        if let Some(u) = first_unit {
            self.minmax_allowed_unit = Some(u.to_string());
        }
    }

    fn group_class(&self) -> String {
        // Wrap in `:merge(...)` so the post-process pass can detect
        // and coalesce adjacent `.group` ancestors when the user
        // chains group-* variants (`group-focus:group-hover:foo` →
        // `.group:focus:hover .foo`, not `.group:focus .group:hover
        // .foo`). Mirrors upstream's `addVariant('group-...')` which
        // emits `:merge(.group):X &` and relies on
        // `mergeAdjacentRules.js` to coalesce.
        format!(":merge(.{}group)", self.prefix)
    }

    fn peer_class(&self) -> String {
        format!(":merge(.{}peer)", self.prefix)
    }
}

fn parse_dark_mode(v: &Value) -> DarkMode {
    match v {
        Value::String(s) => string_dark_mode(s, ".dark"),
        Value::Array(arr) => {
            let mode = arr.first().and_then(Value::as_str).unwrap_or("media");
            // For 'class' / 'selector' the second element is the selector.
            // For 'variant' the rest are raw `&`-bearing format strings.
            match mode {
                "class" => {
                    let sel = arr.get(1).and_then(Value::as_str).unwrap_or(".dark");
                    DarkMode::Class(sel.to_string())
                }
                "selector" => {
                    let sel = arr.get(1).and_then(Value::as_str).unwrap_or(".dark");
                    DarkMode::Selector(sel.to_string())
                }
                "variant" => {
                    let mut formats = Vec::new();
                    for item in arr.iter().skip(1) {
                        match item {
                            Value::String(s) => formats.push(s.clone()),
                            Value::Array(inner) => {
                                for x in inner {
                                    if let Some(s) = x.as_str() {
                                        formats.push(s.to_string());
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    DarkMode::Variant(formats)
                }
                _ => DarkMode::Media,
            }
        }
        _ => DarkMode::Media,
    }
}

fn string_dark_mode(s: &str, default_selector: &str) -> DarkMode {
    match s {
        "media" => DarkMode::Media,
        "class" => DarkMode::Class(default_selector.to_string()),
        "selector" => DarkMode::Selector(default_selector.to_string()),
        _ => DarkMode::Media,
    }
}

fn read_kv(obj: &serde_json::Map<String, Value>) -> Vec<(String, String)> {
    obj.iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect()
}

fn merge_kv(dst: &mut Vec<(String, String)>, src: &serde_json::Map<String, Value>) {
    for (k, v) in src {
        if let Some(s) = v.as_str() {
            if let Some(existing) = dst.iter_mut().find(|(kk, _)| kk == k) {
                existing.1 = s.to_string();
            } else {
                dst.push((k.clone(), s.to_string()));
            }
        }
    }
}

/// Mirrors Tailwind's default `theme.aria` table from `stubs/config.full.js`.
fn default_aria_shortcuts() -> Vec<(String, String)> {
    vec![
        ("busy".into(), "busy=\"true\"".into()),
        ("checked".into(), "checked=\"true\"".into()),
        ("disabled".into(), "disabled=\"true\"".into()),
        ("expanded".into(), "expanded=\"true\"".into()),
        ("hidden".into(), "hidden=\"true\"".into()),
        ("pressed".into(), "pressed=\"true\"".into()),
        ("readonly".into(), "readonly=\"true\"".into()),
        ("required".into(), "required=\"true\"".into()),
        ("selected".into(), "selected=\"true\"".into()),
    ]
}

/// Result of resolving one variant against a `VariantContext`.
#[derive(Clone, Debug, Default)]
pub struct ResolvedVariant {
    /// Selector formats containing `&` placeholders. Each format becomes a
    /// distinct `Rule`. May be empty for at-rule-only variants like `print`.
    pub selector_formats: Vec<String>,
    /// At-rule header without trailing braces, e.g. `"@media (min-width: 768px)"`.
    pub at_rule: Option<String>,
    /// `before` / `after` need `content: var(--tw-content)` prepended unless
    /// the rule already declares `content`.
    pub prepend_content: bool,
    /// True if this variant introduces a pseudo-element (`::before`,
    /// `::after`, `::placeholder`, `::file-selector-button`, etc.). CSS
    /// pseudo-elements are terminal — they must sit at the very end of
    /// the compound selector. Tailwind's PostCSS pipeline enforces this
    /// regardless of the variant's source position; we deferred-apply
    /// to mirror it. Without the flag, `checked:before:foo` produced
    /// `:: before:checked` instead of `:checked::before`.
    pub is_pseudo_element: bool,
    /// `--tw-*-opacity` cascade variables this variant strips from the
    /// rule body. Mirrors upstream's `removeAlphaVariables` calls in
    /// `marker:` (text-opacity), `visited:` (text/border/bg), and
    /// `first-letter:` (text/bg). Stripping means: drop any
    /// `<var>: 1` declaration outright, AND remove `/ var(<var>)` /
    /// `/ var(<var>, 1)` substrings from other declaration values.
    /// Empty for variants that don't strip anything.
    pub removes_alpha_vars: &'static [&'static str],
    /// Optional `decl.value` transform captured from a function-form
    /// `addVariant` that called `container.walkDecls`. The template
    /// uses `{}` as the placeholder for the original value (e.g.
    /// `calc(0 + {})`). Applied to every emitted decl in rules
    /// using this variant.
    pub decl_value_template: Option<String>,
}

/// Apply all variants on a candidate to an initial selector and declarations,
/// returning one (selector, at-rules) pair per output rule, plus any extra
/// declarations that variants demand be prepended (used by `before` / `after`).
#[derive(Clone, Debug)]
pub struct AppliedVariants {
    /// Each entry is the final selector for one emitted rule.
    pub selectors: Vec<String>,
    /// Outermost-first stack of at-rule headers shared by all selectors.
    pub at_rules: Vec<String>,
    /// `(property, value)` pairs to prepend (in order) to the rule's
    /// declarations if not already present.
    pub prepend_decls: Vec<(&'static str, &'static str)>,
    /// Cascade-variable names (`--tw-text-opacity`, etc.) that any
    /// variant in the chain demands be stripped from the rule body.
    /// The `marker:` and `visited:` variants set this to mirror
    /// upstream's `removeAlphaVariables` post-pass: matching `<var>: 1`
    /// declarations are dropped entirely, and `/ var(<var>)` /
    /// `/ var(<var>, 1)` substrings are removed from other declaration
    /// values so the color emits without the opacity indirection.
    pub remove_alpha_vars: Vec<&'static str>,
    /// `decl.value` transform templates collected from any
    /// function-form addVariant in the chain that mutates
    /// declarations via `container.walkDecls`. Each template uses
    /// `{}` as the placeholder for the original value and is
    /// applied in order to the resolved value of every declaration.
    pub decl_value_templates: Vec<String>,
}

/// Apply the variants of a parsed candidate. `class_selector` is the final,
/// already-escaped class selector for the candidate (e.g. `.hover\:flex`).
///
/// Returns `Err` with a human-readable reason if any variant cannot be
/// resolved — the caller should propagate that as an `unsupported-variant`
/// diagnostic.
pub fn apply_variants(
    variants: &[ParsedVariant],
    class_selector: &str,
    cx: &VariantContext,
) -> Result<AppliedVariants, String> {
    if variants.is_empty() {
        return Ok(AppliedVariants {
            selectors: vec![class_selector.to_string()],
            at_rules: Vec::new(),
            prepend_decls: Vec::new(),
            remove_alpha_vars: Vec::new(),
            decl_value_templates: Vec::new(),
        });
    }

    let mut working_selectors: Vec<String> = vec!["&".to_string()];
    let mut at_rules: Vec<String> = Vec::new();
    let mut prepend_decls: Vec<(&'static str, &'static str)> = Vec::new();
    let mut remove_alpha_vars: Vec<&'static str> = Vec::new();
    let mut decl_value_templates: Vec<String> = Vec::new();
    // Pseudo-element variants (`before`, `after`, `placeholder`,
    // `marker`, `selection`, `file`, `backdrop`, `first-letter`,
    // `first-line`) are CSS-terminal — they must sit at the very end
    // of the compound selector regardless of source position. We
    // stash their formats and layer them once the regular chain is
    // built, so `checked:before:foo` produces `:checked::before`
    // rather than `::before:checked`.
    let mut pseudo_element_formats: Vec<String> = Vec::new();

    // Reverse source-order so the LAST variant in source ends up CLOSEST
    // to the class in the final selector. See module docs.
    for variant in variants.iter().rev() {
        let resolved = resolve_variant(variant, cx)
            .ok_or_else(|| format!("variant `{}` is not supported", describe_variant(variant)))?;

        if resolved.prepend_content {
            prepend_decls.push(("content", "var(--tw-content)"));
        }

        for v in resolved.removes_alpha_vars {
            if !remove_alpha_vars.contains(v) {
                remove_alpha_vars.push(v);
            }
        }

        if let Some(t) = &resolved.decl_value_template {
            decl_value_templates.push(t.clone());
        }

        if let Some(at_rule) = resolved.at_rule {
            // Plugin variants may encode a nested at-rule chain as
            // `"@supports (hover: hover) > @media print"`. Split
            // on the ` > ` separator (outside brackets/parens) and
            // insert each as a separate at-rule, outermost-first.
            let chain = split_at_rule_chain(&at_rule);
            for (offset, part) in chain.into_iter().rev().enumerate() {
                let _ = offset;
                at_rules.insert(0, part);
            }
        }

        if resolved.is_pseudo_element {
            pseudo_element_formats.extend(resolved.selector_formats);
            continue;
        }

        if !resolved.selector_formats.is_empty() {
            let mut next: Vec<String> =
                Vec::with_capacity(working_selectors.len() * resolved.selector_formats.len());
            for sel in &working_selectors {
                for fmt in &resolved.selector_formats {
                    next.push(substitute_amp(fmt, sel));
                }
            }
            working_selectors = next;
        }
    }

    // Layer the pseudo-element terminal onto whatever the regular
    // variant chain produced. Multiple pseudo-elements is unusual but
    // possible (`marker` emits two formats; each gets its own rule).
    if !pseudo_element_formats.is_empty() {
        let mut next: Vec<String> =
            Vec::with_capacity(working_selectors.len() * pseudo_element_formats.len());
        for sel in &working_selectors {
            for fmt in &pseudo_element_formats {
                next.push(substitute_amp(fmt, sel));
            }
        }
        working_selectors = next;
    }

    // Final pass: replace `&` with the actual class selector,
    // coalesce duplicate `:merge(.<id>)` references that ended up in
    // the same ancestor chain (so chained `group-focus:group-hover:`
    // collapses to a single `.group` ancestor with both pseudo-
    // classes), and strip the `:merge(...)` wrapper.
    let selectors = working_selectors
        .into_iter()
        .map(|s| strip_merge(&coalesce_merge(&substitute_amp(&s, class_selector))))
        .collect();

    Ok(AppliedVariants {
        selectors,
        at_rules,
        prepend_decls,
        remove_alpha_vars,
        decl_value_templates,
    })
}

/// Apply each variant's `{}` template to `value` in registration
/// order. `calc(0 + {})` against `700` -> `calc(0 + 700)`. No-op
/// when `templates` is empty.
pub fn apply_decl_value_templates(value: &str, templates: &[String]) -> String {
    let mut current = value.to_string();
    for tmpl in templates {
        current = tmpl.replace("{}", &current);
    }
    current
}

/// Replace every literal `&` in `format` with `replacement`. We need this in
/// two phases (variant chaining, then final class-selector substitution) so
/// it can't blindly use one-shot `String::replace` semantics — it does, but
/// for documentation: `&` is not a CSS identifier character, so no class
/// or selector can legally contain a bare `&` that we'd accidentally match.
fn substitute_amp(format: &str, replacement: &str) -> String {
    format.replace('&', replacement)
}

/// Coalesce adjacent `:merge(.<id>)<suffix>` references in a selector.
/// Mirrors upstream's `mergeAdjacentRules.js`: when two `:merge(.<id>)`
/// tokens with the same `<id>` appear separated only by a combinator,
/// they refer to the same ancestor element and the combinator is
/// dropped. So
/// `:merge(.group):focus :merge(.group):hover &` becomes
/// `:merge(.group):focus:hover &`. Without this collapse, chaining
/// `group-focus:group-hover:` would emit two separate `.group`
/// ancestors in the descendant chain — different markup pattern.
fn coalesce_merge(input: &str) -> String {
    let mut out = input.to_string();
    loop {
        let next = coalesce_merge_once(&out);
        if next == out {
            return out;
        }
        out = next;
    }
}

fn coalesce_merge_once(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 7 <= bytes.len() {
        if let Some((merge1_end, id1_start, id1_end)) = parse_merge_token(bytes, i) {
            // Scan from merge1_end to find a top-level combinator.
            if let Some((suffix1_end, comb_end)) = scan_to_combinator(bytes, merge1_end) {
                if let Some((merge2_end, id2_start, id2_end)) = parse_merge_token(bytes, comb_end) {
                    if s[id1_start..id1_end] == s[id2_start..id2_end] {
                        // Coalesce: keep first :merge(...) + suffix1, drop
                        // combinator and second :merge(...). Remaining
                        // bytes (suffix2 + rest) follow.
                        let mut new = String::with_capacity(s.len());
                        new.push_str(&s[..suffix1_end]);
                        new.push_str(&s[merge2_end..]);
                        return new;
                    }
                }
            }
            i = merge1_end;
            continue;
        }
        i += 1;
    }
    s.to_string()
}

/// Parse `:merge(<id>)` at `bytes[idx..]`. Returns
/// `(end_index_after_close_paren, id_start, id_end)` if matched.
fn parse_merge_token(bytes: &[u8], idx: usize) -> Option<(usize, usize, usize)> {
    if idx + 7 > bytes.len() {
        return None;
    }
    if &bytes[idx..idx + 7] != b":merge(" {
        return None;
    }
    let id_start = idx + 7;
    let mut depth = 1i32;
    let mut j = id_start;
    while j < bytes.len() && depth > 0 {
        match bytes[j] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return Some((j + 1, id_start, j));
        }
        j += 1;
    }
    None
}

/// From `bytes[start..]`, scan forward at top-level (skipping inside
/// `()`/`[]`) until the first whitespace-or-structural combinator
/// (`<space>`, `>`, `+`, `~`). Returns
/// `(suffix_end_exclusive_combinator, after_combinator_index)` —
/// `suffix_end` is the byte index of the start of the combinator run;
/// `after_combinator` is the byte index just past the combinator
/// (including any surrounding whitespace).
fn scan_to_combinator(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    let mut depth = 0i32;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            _ => {}
        }
        if depth == 0 && (bytes[i] == b' ' || matches!(bytes[i], b'>' | b'+' | b'~')) {
            let comb_start = i;
            let mut j = i;
            // Eat leading whitespace.
            while j < bytes.len() && bytes[j] == b' ' {
                j += 1;
            }
            // Optional structural combinator.
            if j < bytes.len() && matches!(bytes[j], b'>' | b'+' | b'~') {
                j += 1;
            }
            // Eat trailing whitespace.
            while j < bytes.len() && bytes[j] == b' ' {
                j += 1;
            }
            if j == comb_start {
                // No actual movement — bail.
                return None;
            }
            return Some((comb_start, j));
        }
        i += 1;
    }
    None
}

/// Drop `:merge(<inner>)` wrappers, keeping `<inner>` in place. Tailwind
/// uses `:merge(.group)` to coalesce duplicate `.group` ancestors when the
/// same group appears in multiple variants on the same element; the wrapper
/// is purely a hint to the post-processor and isn't valid CSS on its own.
/// Our compiled output never benefits from coalescing (one rule per
/// candidate), so we strip and emit the bare selector.
fn strip_merge(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 7 <= bytes.len() && &bytes[i..i + 7] == b":merge(" {
            // Find matching close paren accounting for nesting.
            let mut depth = 1usize;
            let mut j = i + 7;
            while j < bytes.len() && depth > 0 {
                match bytes[j] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    break;
                }
                j += 1;
            }
            if depth == 0 {
                // Inner is bytes[i+7..j] (j is the `)` index).
                out.push_str(&s[i + 7..j]);
                i = j + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn describe_variant(v: &ParsedVariant) -> String {
    match v {
        ParsedVariant::Named(s) => (*s).to_string(),
        ParsedVariant::Arbitrary(s) => format!("[{s}]"),
    }
}

/// Resolve a single variant. Returns `None` for variants we don't know how
/// to handle yet — the caller turns that into an `unsupported-variant`
/// diagnostic so the conformance harness can flag the gap loudly.
pub fn resolve_variant(variant: &ParsedVariant, cx: &VariantContext) -> Option<ResolvedVariant> {
    match variant {
        ParsedVariant::Arbitrary(inner) => resolve_arbitrary(inner),
        ParsedVariant::Named(name) => resolve_named(name, cx),
    }
}

// Default `theme.aria` keys from `vendor/tailwindcss-v3/stubs/config.full.js`.
// Used when the resolved config doesn't supply `theme.aria` (e.g. fixtures
// that pass `corePlugins.preflight=false` without going through
// `resolveConfig`).
const DEFAULT_ARIA_KEYS: &[&str] = &[
    "busy", "checked", "disabled", "expanded", "hidden", "pressed", "readonly", "required",
    "selected",
];

/// Register the `<family>-<key>` named variants supplied by
/// `matchVariant(family, fn, { values })` in upstream. Each key gets
/// its own bit (mirrors `addVariant(`${family}-${key}`, …)` inside
/// `matchVariant`), then the family base form gets the final bit.
/// Mirrors `setupContextUtils.js:586-641`.
fn register_match_variant_family(
    offsets: &mut crate::offsets::Offsets,
    family: &str,
    config: Option<&serde_json::Value>,
    theme_path: &str,
    default_keys: &[&str],
) {
    let mut keys: Vec<String> = Vec::new();
    // `__pseudo__` is a sentinel for the hardcoded pseudo-class list
    // used by `matchVariant('group', …, {values: pseudoVariants})`
    // and `matchVariant('peer', …, {values: pseudoVariants})` — there
    // is no `theme.__pseudo__` entry to read; the values are baked
    // into corePlugins.js. Skip the theme lookup in that case.
    let from_config = if theme_path == "__pseudo__" {
        None
    } else {
        config
            .and_then(|c| c.get("theme"))
            .and_then(|t| t.get(theme_path))
            .and_then(|v| v.as_object())
    };
    if let Some(obj) = from_config {
        for k in obj.keys() {
            if k == "DEFAULT" {
                continue;
            }
            keys.push(k.clone());
        }
    } else {
        for k in default_keys {
            keys.push((*k).to_string());
        }
    }
    for k in &keys {
        let name = format!("{family}-{k}");
        offsets.record_variant_bit(&name, 1);
    }
    offsets.record_variant_bit(family, 1);
}

pub fn register_core_variants_with_config(
    offsets: &mut crate::offsets::Offsets,
    config: Option<&serde_json::Value>,
) {
    // ---- beforeVariants (setupContextUtils.js:768-775) ----

    // childVariant
    offsets.record_variant_bit("*", 1);

    // pseudoElementVariants (corePlugins.js:31-85)
    offsets.record_variant_bit("first-letter", 1);
    offsets.record_variant_bit("first-line", 1);
    offsets.record_variant_bit("marker", 2); // 2 parallel selectors
    offsets.record_variant_bit("selection", 2); // 2 parallel selectors
    offsets.record_variant_bit("file", 1);
    offsets.record_variant_bit("placeholder", 1);
    offsets.record_variant_bit("backdrop", 1);
    offsets.record_variant_bit("before", 1);
    offsets.record_variant_bit("after", 1);

    // pseudoClassVariants positional (corePlugins.js:90-97)
    offsets.record_variant_bit("first", 1);
    offsets.record_variant_bit("last", 1);
    offsets.record_variant_bit("only", 1);
    offsets.record_variant_bit("odd", 1);
    offsets.record_variant_bit("even", 1);
    offsets.record_variant_bit("first-of-type", 1);
    offsets.record_variant_bit("last-of-type", 1);
    offsets.record_variant_bit("only-of-type", 1);

    // pseudoClassVariants state (corePlugins.js:100-114)
    offsets.record_variant_bit("visited", 1);
    offsets.record_variant_bit("target", 1);
    offsets.record_variant_bit("open", 1);

    // pseudoClassVariants forms (corePlugins.js:116-128)
    offsets.record_variant_bit("default", 1);
    offsets.record_variant_bit("checked", 1);
    offsets.record_variant_bit("indeterminate", 1);
    offsets.record_variant_bit("placeholder-shown", 1);
    offsets.record_variant_bit("autofill", 1);
    offsets.record_variant_bit("optional", 1);
    offsets.record_variant_bit("required", 1);
    offsets.record_variant_bit("valid", 1);
    offsets.record_variant_bit("invalid", 1);
    offsets.record_variant_bit("in-range", 1);
    offsets.record_variant_bit("out-of-range", 1);
    offsets.record_variant_bit("read-only", 1);

    // pseudoClassVariants content (corePlugins.js:130)
    offsets.record_variant_bit("empty", 1);

    // pseudoClassVariants interactive (corePlugins.js:132-144)
    offsets.record_variant_bit("focus-within", 1);
    offsets.record_variant_bit("hover", 1);
    offsets.record_variant_bit("focus", 1);
    offsets.record_variant_bit("focus-visible", 1);
    offsets.record_variant_bit("active", 1);
    offsets.record_variant_bit("enabled", 1);
    offsets.record_variant_bit("disabled", 1);

    // pseudoClassVariants matchVariants: group, peer (corePlugins.js:155-206).
    // Each is matchVariant(NAME, fn, {values: pseudoVariants}) where
    // pseudoVariants is the entire pseudo-class list (first/last/.../
    // disabled). That registers `group-first`, …, `group-disabled`,
    // then `group` base form last. Same for `peer`.
    let pseudo_keys: &[&str] = &[
        "first",
        "last",
        "only",
        "odd",
        "even",
        "first-of-type",
        "last-of-type",
        "only-of-type",
        "visited",
        "target",
        "open",
        "default",
        "checked",
        "indeterminate",
        "placeholder-shown",
        "autofill",
        "optional",
        "required",
        "valid",
        "invalid",
        "in-range",
        "out-of-range",
        "read-only",
        "empty",
        "focus-within",
        "hover",
        "focus",
        "focus-visible",
        "active",
        "enabled",
        "disabled",
    ];
    register_match_variant_family(offsets, "group", config, "__pseudo__", pseudo_keys);
    register_match_variant_family(offsets, "peer", config, "__pseudo__", pseudo_keys);

    // hasVariants (corePlugins.js:437-471). Each of `has`, `group-has`,
    // `peer-has` is a matchVariant with no preset values — only the
    // base form gets a bit. Arbitrary `[X]` inner values get their
    // own bits at compile-pre-pass time.
    offsets.record_variant_bit("has", 1);
    offsets.record_variant_bit("group-has", 1);
    offsets.record_variant_bit("peer-has", 1);

    // ariaVariants (corePlugins.js:474-494). Three matchVariants —
    // `aria`, `group-aria`, `peer-aria` — each with the same
    // `theme.aria` values. Order: all `aria-<key>` first then `aria`,
    // then all `group-aria-<key>` then `group-aria`, then peer-aria
    // family.
    register_match_variant_family(offsets, "aria", config, "aria", DEFAULT_ARIA_KEYS);
    register_match_variant_family(offsets, "group-aria", config, "aria", DEFAULT_ARIA_KEYS);
    register_match_variant_family(offsets, "peer-aria", config, "aria", DEFAULT_ARIA_KEYS);

    // dataVariants (corePlugins.js:496-516). Default theme.data is
    // empty, so only base forms get bits unless the user supplies
    // `theme.data`.
    register_match_variant_family(offsets, "data", config, "data", &[]);
    register_match_variant_family(offsets, "group-data", config, "data", &[]);
    register_match_variant_family(offsets, "peer-data", config, "data", &[]);

    // User-supplied plugin variants (between beforeVariants and
    // afterVariants per `setupContextUtils.js:768-787`). Each
    // `addVariant(name, …)` / `matchVariant(name, …, {values})` call
    // gets one bit here in plugin registration order. Without this,
    // two plugin-added variants with similar names (e.g.
    // `peer-aria-expanded`, `peer-aria-expanded-2`) would fall through
    // to family-prefix resolution and collide on a shared composite
    // bitmask — sort would tie-break on `input_index` alphabetically
    // instead of plugin registration order.
    let plugin_output = crate::plugins::read_plugin_output(config);
    for v in &plugin_output.variants {
        // Skip variants already registered as core (the plugin runner
        // may capture re-registrations from preset/parent configs).
        if !offsets.has_variant(&v.name) {
            offsets.record_variant_bit(&v.name, 1);
        }
    }

    // ---- afterVariants (setupContextUtils.js:777-787) ----

    // supportsVariants (corePlugins.js:408). Default theme.supports
    // is empty.
    register_match_variant_family(offsets, "supports", config, "supports", &[]);

    // reducedMotionVariants (corePlugins.js:214-217)
    offsets.record_variant_bit("motion-safe", 1);
    offsets.record_variant_bit("motion-reduce", 1);

    // prefersContrastVariants (corePlugins.js:523-526)
    offsets.record_variant_bit("contrast-more", 1);
    offsets.record_variant_bit("contrast-less", 1);

    // screenVariants — default screens sm/md/lg/xl/2xl
    // (corePlugins.js:281-406; the `addVariant(screen.name, ...)`
    // call shares an `id` with the min/max matchVariants so
    // upstream's sort callback can compare across them).
    offsets.record_variant_bit("sm", 1);
    offsets.record_variant_bit("md", 1);
    offsets.record_variant_bit("lg", 1);
    offsets.record_variant_bit("xl", 1);
    offsets.record_variant_bit("2xl", 1);
    // Arbitrary min-/max-[…] matchVariants
    offsets.record_variant_bit("min", 1);
    offsets.record_variant_bit("max", 1);

    // orientationVariants (corePlugins.js:518)
    offsets.record_variant_bit("portrait", 1);
    offsets.record_variant_bit("landscape", 1);

    // directionVariants (corePlugins.js:209-211)
    offsets.record_variant_bit("ltr", 1);
    offsets.record_variant_bit("rtl", 1);

    // darkVariants (corePlugins.js:219-275)
    offsets.record_variant_bit("dark", 1);

    // forcedColorsVariants (corePlugins.js:528)
    offsets.record_variant_bit("forced-colors", 1);

    // printVariant (corePlugins.js:277)
    offsets.record_variant_bit("print", 1);
}

/// `[foo]` arbitrary variant. Tailwind validates the inner as a selector
/// or at-rule format: it must contain `&` (selector form) or start with
/// `@` (at-rule form). Anything else is dropped. See `isValidVariantFormatString`
/// in `vendor/tailwindcss-v3/src/lib/setupContextUtils.js`:
///
/// ```js
/// export function isValidVariantFormatString(format) {
///   return format.startsWith('@') || format.includes('&')
/// }
/// ```
fn resolve_arbitrary(inner: &str) -> Option<ResolvedVariant> {
    let inner = inner.trim();
    let normalized = replace_normalize_underscores(inner);
    // Nested at-rule + selector format:
    //   `[@media (hover:hover) { &:hover }]:underline`
    //   `[@media screen { @media (hover:hover) }]:underline`
    // Mirrors upstream's `parseVariantFormatString`. Split braces
    // to peel off the at-rule chain (joined as `' > '` so the
    // chain consumer in `apply_variants` can splay them) and the
    // trailing selector format.
    if normalized.contains('{') {
        let parts = split_arbitrary_variant_format(&normalized);
        let mut at_chain: Vec<String> = Vec::new();
        let mut sel_format: Option<String> = None;
        for p in parts {
            if p.starts_with('@') {
                at_chain.push(p);
            } else if p.contains('&') {
                if sel_format.is_some() {
                    // Multiple selector formats inside a single
                    // arbitrary variant — Tailwind drops these.
                    return None;
                }
                sel_format = Some(p);
            }
        }
        if at_chain.is_empty() && sel_format.is_none() {
            return None;
        }
        let joined_at = if at_chain.is_empty() {
            None
        } else {
            Some(at_chain.join(" > "))
        };
        let formats = match sel_format {
            Some(s) => vec![s],
            None => Vec::new(),
        };
        return Some(ResolvedVariant {
            selector_formats: formats,
            at_rule: joined_at,
            prepend_content: false,
            is_pseudo_element: false,
            removes_alpha_vars: &[],
            decl_value_template: None,
        });
    }
    if normalized.starts_with('@') {
        return Some(ResolvedVariant {
            selector_formats: Vec::new(),
            at_rule: Some(normalized),
            prepend_content: false,
            is_pseudo_element: false,
            removes_alpha_vars: &[],
            decl_value_template: None,
        });
    }
    if !normalized.contains('&') {
        // Tailwind drops these (`tests/arbitrary-variants.test.js` —
        // "variants without & or an at-rule are ignored"). Returning
        // `None` propagates an `unsupported-variant` diagnostic.
        return None;
    }
    // Reject selectors with characters that would emit invalid CSS
    // even after escape (top-level `;` ends a declaration, top-level
    // `{`/`}` would start/end a rule). Mirrors upstream's
    // `parseSelectorList` / `validateFormalSyntax` failure path —
    // the candidate is silently dropped.
    if arbitrary_selector_has_invalid_top_level_chars(&normalized) {
        return None;
    }
    // Reject multi-selector arbitrary variants (`[div,span]`).
    // Tailwind discards them per `arbitrary-variants.test.js`'s
    // "it should discard arbitrary variants with multiple
    // selectors" — they'd produce ambiguous selector substitution.
    // Escaped commas (`\,`) are kept.
    if has_top_level_comma(&normalized) {
        return None;
    }
    Some(ResolvedVariant {
        selector_formats: vec![normalized],
        at_rule: None,
        prepend_content: false,
        is_pseudo_element: false,
        removes_alpha_vars: &[],
        decl_value_template: None,
    })
}

/// Mirrors upstream's `parseVariantFormatString` for the inner
/// of a nested arbitrary variant `[<inner>]:foo` where `<inner>`
/// contains `{` / `}`. Splits on braces (preserving escapes) so
/// `@media screen { @media (hover:hover) { &:hover } }` becomes
/// `["@media screen", "@media (hover:hover)", "&:hover"]`.
fn split_arbitrary_variant_format(input: &str) -> Vec<String> {
    let bytes = input.as_bytes();
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i];
        if ch == b'\\' && i + 1 < bytes.len() {
            current.push(ch as char);
            current.push(bytes[i + 1] as char);
            i += 2;
            continue;
        }
        if ch == b'{' {
            depth += 1;
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                parts.push(trimmed);
            }
            current.clear();
            i += 1;
            continue;
        }
        if ch == b'}' {
            depth -= 1;
            if depth < 0 {
                break;
            }
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                parts.push(trimmed);
            }
            current.clear();
            i += 1;
            continue;
        }
        current.push(ch as char);
        i += 1;
    }
    let tail = current.trim().to_string();
    if !tail.is_empty() {
        parts.push(tail);
    }
    parts
}

/// Split a `>`-joined at-rule chain like
/// `"@supports (hover: hover) > @media print"` into individual
/// at-rule headers. Respects bracket/paren nesting so a literal
/// `>` inside `[...]` (e.g. attribute matchers) doesn't split.
fn split_at_rule_chain(s: &str) -> Vec<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    let mut start = 0;
    let mut i = 0;
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
            b'(' => depth_paren += 1,
            b')' => depth_paren -= 1,
            b'[' => depth_bracket += 1,
            b']' => depth_bracket -= 1,
            b' ' if depth_paren == 0 && depth_bracket == 0 => {
                if i + 2 < bytes.len() && bytes[i + 1] == b'>' && bytes[i + 2] == b' ' {
                    out.push(s[start..i].trim().to_string());
                    i += 3;
                    start = i;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let tail = s[start..].trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

/// Returns true if `selector` has an unescaped, top-level `,` —
/// indicating multiple selectors, which Tailwind discards in
/// arbitrary variants.
fn has_top_level_comma(selector: &str) -> bool {
    let bytes = selector.as_bytes();
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
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
            b',' if depth_paren == 0 && depth_bracket == 0 => return true,
            _ => i += 1,
        }
    }
    false
}

/// Returns true if `selector` contains a top-level `;`, `{`, or `}`
/// — characters that would terminate the selector or open/close a
/// rule body when emitted into CSS. Tailwind rejects these via its
/// selector parser before generating output.
fn arbitrary_selector_has_invalid_top_level_chars(selector: &str) -> bool {
    let bytes = selector.as_bytes();
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
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
            b';' | b'{' | b'}' if depth_paren == 0 && depth_bracket == 0 => {
                return true;
            }
            _ => i += 1,
        }
    }
    false
}

/// Lightweight version of Tailwind's `normalize`: convert non-escaped `_`
/// to spaces. Tailwind does more (math operator spacing, url() preservation),
/// but for arbitrary variants we only need underscore translation. The
/// conformance fixtures in Phase D don't exercise the math-operator path
/// for variant values.
fn replace_normalize_underscores(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'_' {
            out.push('_');
            i += 2;
            continue;
        }
        if b == b'_' {
            out.push(' ');
            i += 1;
            continue;
        }
        out.push(b as char);
        i += 1;
    }
    out
}

fn resolve_named(name: &str, cx: &VariantContext) -> Option<ResolvedVariant> {
    // 0. User-defined variants registered via `addVariant()`. Checked
    //    BEFORE built-ins so plugin variants can shadow built-in
    //    names if the user really wants — matches upstream's
    //    "last-registered-wins" semantics for variant registration.
    if let Some(r) = resolve_plugin_variant(name, cx) {
        return Some(r);
    }

    // 1. Bare pseudo-class / pseudo-element / direction / media variants.
    if let Some(r) = resolve_simple(name, cx) {
        return Some(r);
    }

    // 2. Group / peer family — `group-…`, `peer-…`, including modifiers and
    //    arbitrary inner values. Must come before the screen split so
    //    `group-aria-checked` etc. don't get parsed as `min-/max-` etc.
    if let Some(r) = resolve_group_or_peer(name, cx) {
        return Some(r);
    }

    // 3. Aria / data / has / supports prefixed variants (without group/peer).
    if let Some(r) = resolve_aria(name, cx) {
        return Some(r);
    }
    if let Some(r) = resolve_data(name, cx) {
        return Some(r);
    }
    if let Some(r) = resolve_has(name) {
        return Some(r);
    }
    if let Some(r) = resolve_supports(name, cx) {
        return Some(r);
    }

    // 4. Screen-derived variants: `sm`, `md`, `2xl`, `min-[…]`, `max-md`,
    //    `min-md`, `max-[…]`.
    if let Some(r) = resolve_screen(name, cx) {
        return Some(r);
    }

    None
}

/// Pseudo-class / pseudo-element / direction / media variants that don't
/// take an inner value. These live in fixed tables mirrored from
/// `corePlugins.js`. Returning `None` lets the caller fall through to
/// prefix-based resolvers.
fn resolve_simple(name: &str, cx: &VariantContext) -> Option<ResolvedVariant> {
    // Pseudo-classes — `&:<state>`-style.
    let pseudo_class_suffix: Option<&'static str> = match name {
        "first" => Some(":first-child"),
        "last" => Some(":last-child"),
        "only" => Some(":only-child"),
        "odd" => Some(":nth-child(odd)"),
        "even" => Some(":nth-child(even)"),
        "first-of-type" => Some(":first-of-type"),
        "last-of-type" => Some(":last-of-type"),
        "only-of-type" => Some(":only-of-type"),
        "visited" => Some(":visited"),
        "target" => Some(":target"),
        "open" => Some("[open]"),
        "default" => Some(":default"),
        "checked" => Some(":checked"),
        "indeterminate" => Some(":indeterminate"),
        "placeholder-shown" => Some(":placeholder-shown"),
        "autofill" => Some(":autofill"),
        "optional" => Some(":optional"),
        "required" => Some(":required"),
        "valid" => Some(":valid"),
        "invalid" => Some(":invalid"),
        "in-range" => Some(":in-range"),
        "out-of-range" => Some(":out-of-range"),
        "read-only" => Some(":read-only"),
        "empty" => Some(":empty"),
        "focus-within" => Some(":focus-within"),
        "hover" => Some(":hover"),
        "focus" => Some(":focus"),
        "focus-visible" => Some(":focus-visible"),
        "active" => Some(":active"),
        "enabled" => Some(":enabled"),
        "disabled" => Some(":disabled"),
        _ => None,
    };
    if let Some(suffix) = pseudo_class_suffix {
        // `:visited` strips opacity-tracking custom properties: the
        // browser hides `var(--tw-*-opacity)` reads on visited links
        // for privacy, so Tailwind's `visited` variant emits the raw
        // colour without the cascade-var indirection. Mirrors
        // upstream's `removeAlphaVariables(['--tw-text-opacity',
        // '--tw-border-opacity', '--tw-bg-opacity'])` in
        // `corePlugins.js`.
        let removes: &'static [&'static str] = if name == "visited" {
            &[
                "--tw-text-opacity",
                "--tw-border-opacity",
                "--tw-bg-opacity",
            ]
        } else {
            &[]
        };
        // `future.hoverOnlyWhenSupported: true` wraps `:hover` in
        // `@media (hover: hover) and (pointer: fine)`. Mirrors
        // `corePlugins.js`'s flag check.
        let at_rule = if name == "hover" && cx.hover_only_when_supported {
            Some("@media (hover: hover) and (pointer: fine)".to_string())
        } else {
            None
        };
        return Some(ResolvedVariant {
            selector_formats: vec![format!("&{suffix}")],
            at_rule,
            prepend_content: false,
            is_pseudo_element: false,
            removes_alpha_vars: removes,
            decl_value_template: None,
        });
    }

    // Pseudo-elements.
    match name {
        "marker" => {
            return Some(ResolvedVariant {
                selector_formats: vec!["& *::marker".into(), "&::marker".into()],
                at_rule: None,
                prepend_content: false,
                is_pseudo_element: true,
                removes_alpha_vars: &["--tw-text-opacity"],
                decl_value_template: None,
            });
        }
        "selection" => {
            return Some(ResolvedVariant {
                selector_formats: vec!["& *::selection".into(), "&::selection".into()],
                at_rule: None,
                prepend_content: false,
                is_pseudo_element: true,
                removes_alpha_vars: &[],
                decl_value_template: None,
            });
        }
        "file" => {
            // ::file-selector-button is "actionable" in upstream's
            // `pseudoElements.js` taxonomy: pseudo-classes attach to
            // it directly rather than jumping past it. Use a plain
            // format so the normal variant chain composition produces
            // `&::file-selector-button:hover` for `hover:file:foo`.
            // (`placeholder:`/`before:`/etc. are "jumpable" — pseudo-
            // classes after them hoist back, so they keep
            // `is_pseudo_element` to defer.)
            return Some(simple_format("&::file-selector-button"));
        }
        "placeholder" => {
            return Some(pseudo_element_format("&::placeholder"));
        }
        "backdrop" => {
            return Some(pseudo_element_format("&::backdrop"));
        }
        "before" => {
            return Some(ResolvedVariant {
                selector_formats: vec!["&::before".into()],
                at_rule: None,
                prepend_content: true,
                is_pseudo_element: true,
                removes_alpha_vars: &[],
                decl_value_template: None,
            });
        }
        "after" => {
            return Some(ResolvedVariant {
                selector_formats: vec!["&::after".into()],
                at_rule: None,
                prepend_content: true,
                is_pseudo_element: true,
                removes_alpha_vars: &[],
                decl_value_template: None,
            });
        }
        "first-letter" => {
            return Some(pseudo_element_format("&::first-letter"));
        }
        "first-line" => {
            return Some(pseudo_element_format("&::first-line"));
        }
        // Direction variants.
        "ltr" => {
            return Some(simple_format("&:where([dir=\"ltr\"], [dir=\"ltr\"] *)"));
        }
        "rtl" => {
            return Some(simple_format("&:where([dir=\"rtl\"], [dir=\"rtl\"] *)"));
        }
        // Media variants without value.
        "motion-safe" => {
            return Some(at_rule_only(
                "@media (prefers-reduced-motion: no-preference)",
            ));
        }
        "motion-reduce" => {
            return Some(at_rule_only("@media (prefers-reduced-motion: reduce)"));
        }
        "print" => {
            return Some(at_rule_only("@media print"));
        }
        "portrait" => {
            return Some(at_rule_only("@media (orientation: portrait)"));
        }
        "landscape" => {
            return Some(at_rule_only("@media (orientation: landscape)"));
        }
        "contrast-more" => {
            return Some(at_rule_only("@media (prefers-contrast: more)"));
        }
        "contrast-less" => {
            return Some(at_rule_only("@media (prefers-contrast: less)"));
        }
        "forced-colors" => {
            return Some(at_rule_only("@media (forced-colors: active)"));
        }
        "dark" => {
            return Some(resolve_dark(cx));
        }
        // `*` = childVariant — `& > *`.
        "*" => {
            return Some(simple_format("& > *"));
        }
        _ => {}
    }

    None
}

/// Look up `name` in the plugin variant table (`addVariant()`
/// registrations). Returns the corresponding `ResolvedVariant` if
/// matched, `None` otherwise.
fn resolve_plugin_variant(name: &str, cx: &VariantContext) -> Option<ResolvedVariant> {
    let entry = cx.plugin_variants.iter().find(|v| v.name == name)?;
    Some(ResolvedVariant {
        selector_formats: entry.selector_formats.clone(),
        // Plugin variants can encode a nested at-rule chain
        // (`@supports (hover: hover) { @media print { &:disabled } }`)
        // as a `>`-joined string from the JS-side parser. Rust
        // unpacks the chain back into individual at-rules.
        at_rule: entry.at_rule.clone(),
        prepend_content: false,
        is_pseudo_element: false,
        removes_alpha_vars: &[],
        decl_value_template: entry.decl_value_template.clone(),
    })
}

fn simple_format(format: &'static str) -> ResolvedVariant {
    ResolvedVariant {
        selector_formats: vec![format.to_string()],
        at_rule: None,
        prepend_content: false,
        is_pseudo_element: false,
        removes_alpha_vars: &[],
        decl_value_template: None,
    }
}

fn pseudo_element_format(format: &'static str) -> ResolvedVariant {
    ResolvedVariant {
        selector_formats: vec![format.to_string()],
        at_rule: None,
        prepend_content: false,
        is_pseudo_element: true,
        removes_alpha_vars: &[],
        decl_value_template: None,
    }
}

fn at_rule_only(at: &'static str) -> ResolvedVariant {
    ResolvedVariant {
        selector_formats: Vec::new(),
        at_rule: Some(at.to_string()),
        prepend_content: false,
        is_pseudo_element: false,
        removes_alpha_vars: &[],
        decl_value_template: None,
    }
}

fn resolve_dark(cx: &VariantContext) -> ResolvedVariant {
    match &cx.dark_mode {
        DarkMode::Media => at_rule_only("@media (prefers-color-scheme: dark)"),
        DarkMode::Class(sel) => ResolvedVariant {
            selector_formats: vec![format!("&:is({sel} *)")],
            at_rule: None,
            prepend_content: false,
            is_pseudo_element: false,
            removes_alpha_vars: &[],
            decl_value_template: None,
        },
        DarkMode::Selector(sel) => ResolvedVariant {
            selector_formats: vec![format!("&:where({sel}, {sel} *)")],
            at_rule: None,
            prepend_content: false,
            is_pseudo_element: false,
            removes_alpha_vars: &[],
            decl_value_template: None,
        },
        DarkMode::Variant(formats) => ResolvedVariant {
            selector_formats: formats.clone(),
            at_rule: None,
            prepend_content: false,
            is_pseudo_element: false,
            removes_alpha_vars: &[],
            decl_value_template: None,
        },
    }
}

/// `aria-…` variants. `aria-<key>` resolves through the theme shortcut
/// table; `aria-[<value>]` uses the bracketed value verbatim.
fn resolve_aria(name: &str, cx: &VariantContext) -> Option<ResolvedVariant> {
    let rest = name.strip_prefix("aria-")?;
    let value = resolve_attr_value(rest, &cx.aria_shortcuts)?;
    Some(simple_attr("aria", &value, cx))
}

fn resolve_data(name: &str, cx: &VariantContext) -> Option<ResolvedVariant> {
    let rest = name.strip_prefix("data-")?;
    let value = resolve_attr_value(rest, &cx.data_shortcuts)?;
    Some(simple_attr("data", &value, cx))
}

fn resolve_supports(name: &str, cx: &VariantContext) -> Option<ResolvedVariant> {
    let rest = name.strip_prefix("supports-")?;
    let value = if let Some(arb) = arbitrary_inner(rest) {
        replace_normalize_underscores(arb)
    } else {
        let (_, val) = cx.supports_shortcuts.iter().find(|(k, _)| k == rest)?;
        val.clone()
    };
    Some(at_rule_only_string(format_supports(&value)))
}

/// `has-[<sel>]` resolves to `&:has(<sel>)`. Tailwind 3.4.19's `has`
/// matchVariant has no theme-driven `values` table — only the arbitrary
/// form is accepted, so `has-foo:flex` falls through as unknown.
fn resolve_has(name: &str) -> Option<ResolvedVariant> {
    let rest = name.strip_prefix("has-")?;
    let arb = arbitrary_inner(rest)?;
    let inner = replace_normalize_underscores(arb);
    Some(ResolvedVariant {
        selector_formats: vec![format!("&:has({inner})")],
        at_rule: None,
        prepend_content: false,
        is_pseudo_element: false,
        removes_alpha_vars: &[],
        decl_value_template: None,
    })
}

/// Extract `&[<prefix>-<value>]` as a `ResolvedVariant`. `value` is the
/// already-normalized attribute body (e.g. `state="open"` or just `state`).
fn simple_attr(prefix: &str, value: &str, cx: &VariantContext) -> ResolvedVariant {
    ResolvedVariant {
        selector_formats: vec![format!(
            "&[{prefix}-{}]",
            normalize_attribute_value(value, !cx.is_v33_compat)
        )],
        at_rule: None,
        prepend_content: false,
        is_pseudo_element: false,
        removes_alpha_vars: &[],
        decl_value_template: None,
    }
}

/// Either look up `key` in the theme shortcut table, or — if the key is
/// `[<inner>]` — return the inner value. Returns `None` for an unknown key
/// without an arbitrary form, mirroring Tailwind which drops the candidate.
fn resolve_attr_value(key: &str, table: &[(String, String)]) -> Option<String> {
    if let Some(arb) = arbitrary_inner(key) {
        return Some(replace_normalize_underscores(arb));
    }
    let (_, val) = table.iter().find(|(k, _)| k == key)?;
    Some(val.clone())
}

/// If `s` is `[<inner>]`, return `Some(<inner>)`; otherwise `None`. Doesn't
/// dive into nesting — Tailwind variant arbitrary values can't nest brackets
/// so the simplest match is correct.
fn arbitrary_inner(s: &str) -> Option<&str> {
    let s = s.strip_prefix('[')?.strip_suffix(']')?;
    Some(s)
}

/// Mirror of Tailwind's `normalizeAttributeSelectors`: when the value
/// contains `=`, the right-hand side becomes a quoted string.
/// `state=open` → `state="open"`, but `state="open"` (already quoted) and
/// flag-bearing forms like `state=open i` are preserved.
///
/// `quote_rhs` toggles the v3.4 quoting behavior — v3.3 leaves the rhs
/// bare, v3.4 wraps in double quotes. The flag/operator handling is
/// preserved in both modes.
fn normalize_attribute_value(value: &str, quote_rhs: bool) -> String {
    let Some(eq_idx) = value.find('=') else {
        return value.to_string();
    };
    let lhs = &value[..eq_idx];
    let rhs = &value[eq_idx + 1..];
    if rhs.is_empty() {
        return value.to_string();
    }
    let first = rhs.as_bytes()[0];
    if first == b'\'' || first == b'"' {
        return value.to_string();
    }
    // Detect a trailing `i`/`I`/`s`/`S` flag separated by a space.
    if rhs.len() > 2 {
        let bytes = rhs.as_bytes();
        let last = bytes[rhs.len() - 1];
        let prev = bytes[rhs.len() - 2];
        if prev == b' ' && (last == b'i' || last == b'I' || last == b's' || last == b'S') {
            let inner = &rhs[..rhs.len() - 2];
            if quote_rhs {
                return format!("{lhs}=\"{inner}\" {}", last as char);
            }
            return format!("{lhs}={inner} {}", last as char);
        }
    }
    if quote_rhs {
        format!("{lhs}=\"{rhs}\"")
    } else {
        format!("{lhs}={rhs}")
    }
}

/// `@supports …` body construction. Mirrors Tailwind's
/// `supportsVariants` matchVariant body.
fn format_supports(value: &str) -> String {
    let mut check = if value.starts_with("--") {
        value.to_string()
    } else {
        replace_normalize_underscores(value)
    };
    let is_raw = is_raw_supports(&check);
    if is_raw {
        // Pad bare `and`/`or`/`not` with spaces so
        // `(foo:bar)and(bar:baz)` becomes
        // `(foo:bar) and (bar:baz)`. Mirrors upstream's
        // `replace(/\b(and|or|not)\b/g, ' $1 ')` in `corePlugins.js`.
        check = pad_supports_keywords(&check);
        return format!("@supports {check}");
    }
    if !check.contains(':') {
        check.push_str(": var(--tw)");
    }
    if !(check.starts_with('(') && check.ends_with(')')) {
        check = format!("({check})");
    }
    format!("@supports {check}")
}

/// Insert a single space on either side of bare `and` / `or` /
/// `not` keywords (treating them as whole-word matches). Excess
/// whitespace is collapsed so `(foo:bar) and (bar:baz)` stays
/// stable across re-runs.
fn pad_supports_keywords(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + 8);
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let is_word = b.is_ascii_alphanumeric() || b == b'_';
        let prev_is_word = if i == 0 {
            false
        } else {
            let p = bytes[i - 1];
            p.is_ascii_alphanumeric() || p == b'_'
        };
        if is_word && !prev_is_word {
            // Look ahead for the bounded word.
            let start = i;
            let mut j = i;
            while j < bytes.len() {
                let c = bytes[j];
                if c.is_ascii_alphanumeric() || c == b'_' {
                    j += 1;
                } else {
                    break;
                }
            }
            let word = &s[start..j];
            if matches!(word, "and" | "or" | "not") {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str(word);
                if !(j < bytes.len() && bytes[j] == b' ') {
                    out.push(' ');
                }
                i = j;
                continue;
            }
        }
        out.push(b as char);
        i += 1;
    }
    // Collapse repeated spaces.
    let mut collapsed = String::with_capacity(out.len());
    let mut prev_space = false;
    for c in out.chars() {
        if c == ' ' {
            if prev_space {
                continue;
            }
            prev_space = true;
        } else {
            prev_space = false;
        }
        collapsed.push(c);
    }
    collapsed.trim().to_string()
}

/// Tailwind treats a value as "raw" (already a complete `@supports` body)
/// if it begins with an identifier-like token followed by `(`, optionally
/// with whitespace — e.g. `selector(:has(.foo))`.
fn is_raw_supports(check: &str) -> bool {
    let bytes = check.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' {
            i += 1;
            continue;
        }
        break;
    }
    while i < bytes.len() && bytes[i] == b' ' {
        i += 1;
    }
    i < bytes.len() && bytes[i] == b'('
}

fn at_rule_only_string(at: String) -> ResolvedVariant {
    ResolvedVariant {
        selector_formats: Vec::new(),
        at_rule: Some(at),
        prepend_content: false,
        is_pseudo_element: false,
        removes_alpha_vars: &[],
        decl_value_template: None,
    }
}

/// Group / peer family. Handles `group-<inner>`, `peer-<inner>`, with
/// optional `/modifier` and arbitrary `[<…>]` inner. The family includes
/// `group-aria-…`, `group-data-…`, etc., which we delegate to the matching
/// non-grouped resolver and then wrap.
fn resolve_group_or_peer(name: &str, cx: &VariantContext) -> Option<ResolvedVariant> {
    let (head, modifier) = split_modifier(name);

    let (kind, inner) = if let Some(rest) = head.strip_prefix("group-") {
        (GroupKind::Group, rest)
    } else if let Some(rest) = head.strip_prefix("peer-") {
        (GroupKind::Peer, rest)
    } else {
        return None;
    };

    // Inner can be:
    //   * arbitrary `[<…>]` — glue to .group/.peer as a compound selector,
    //     OR if it begins with one of the structural pseudo combinators
    //     (`>`, `+`, `~`) the inner becomes its own descendant context. In
    //     practice `group-[.foo]` is a compound and `group-[:hover]` glues
    //     a pseudo-class — both are "after `&`" cases in Tailwind's
    //     replacement logic.
    //   * a known pseudo-class (resolved via `resolve_simple`) — produces
    //     `:hover`-style suffix.
    //   * `aria-…` / `data-…` — produces `[aria-…]` / `[data-…]` suffix.
    //   * `has-[<…>]` — produces `:has(<…>)` suffix.

    let suffix = if let Some(arb) = arbitrary_inner(inner) {
        // The arbitrary inner can reference `&` to mean "the
        // .group / .peer class itself" (so `group-[&:focus]`
        // produces `.group:focus .candidate` and
        // `group-[.in-foo_&]` produces `.in-foo .group .candidate`).
        // Strip the `&` before composition — the `combinator + &`
        // tail below adds the candidate-class reference.
        let normalized = replace_normalize_underscores(arb);
        if normalized.contains('&') {
            // The `&` in the inner refers to the .group / .peer
            // class. Wrap the inner as a separate descendant
            // chunk so anything BEFORE the `&` becomes ancestor
            // context (e.g. `.in-foo &` → `.in-foo .group`), and
            // anything AFTER the `&` glues to the .group class.
            return resolve_group_peer_with_amp(kind, &normalized, modifier, cx);
        }
        normalized
    } else if let Some(simple) = resolve_simple(inner, cx) {
        // Selector-format simple variant: extract the part after `&`.
        // At-rule-only inners are not valid for group/peer — Tailwind would
        // emit nothing — so we drop them by returning None.
        if simple.selector_formats.is_empty() {
            return None;
        }
        // For multi-format variants (selection/marker), fold each into a
        // separate group/peer format below. We handle the single-format
        // case here; for multi-format, we'd need a different return path.
        if simple.selector_formats.len() != 1 {
            return None;
        }
        suffix_after_amp(&simple.selector_formats[0]).to_string()
    } else if let Some(rest) = inner.strip_prefix("aria-") {
        let value = resolve_attr_value(rest, &cx.aria_shortcuts)?;
        format!(
            "[aria-{}]",
            normalize_attribute_value(&value, !cx.is_v33_compat)
        )
    } else if let Some(rest) = inner.strip_prefix("data-") {
        let value = resolve_attr_value(rest, &cx.data_shortcuts)?;
        format!(
            "[data-{}]",
            normalize_attribute_value(&value, !cx.is_v33_compat)
        )
    } else if let Some(rest) = inner.strip_prefix("has-") {
        // `group-has-[…]` / `peer-has-[…]` — `:has(…)` glued onto the base
        // class. Like the bare `has-` variant, only the arbitrary form is
        // accepted (no theme values table in v3.4.19).
        let arb = arbitrary_inner(rest)?;
        format!(":has({})", replace_normalize_underscores(arb))
    } else {
        return None;
    };

    let base = match kind {
        GroupKind::Group => cx.group_class(),
        GroupKind::Peer => cx.peer_class(),
    };
    let combinator = match kind {
        GroupKind::Group => " ",
        GroupKind::Peer => " ~ ",
    };
    let modifier_part = if let Some(m) = modifier {
        format!("\\/{}", escape_class_name(m))
    } else {
        String::new()
    };
    let format = format!("{base}{modifier_part}{suffix}{combinator}&");
    // Inherit `hover_only_when_supported` wrapping when the inner
    // resolved to `:hover`. Only `group-hover` / `peer-hover` get
    // the media query — other `:hover`-bearing inners (`group-has-`
    // etc.) leave it alone since they aren't directly hover
    // variants.
    let at_rule = if cx.hover_only_when_supported && inner == "hover" {
        Some("@media (hover: hover) and (pointer: fine)".to_string())
    } else {
        None
    };
    Some(ResolvedVariant {
        selector_formats: vec![format],
        at_rule,
        prepend_content: false,
        is_pseudo_element: false,
        removes_alpha_vars: &[],
        decl_value_template: None,
    })
}

#[derive(Clone, Copy, Debug)]
enum GroupKind {
    Group,
    Peer,
}

/// Resolve a `group-[<inner>]` / `peer-[<inner>]` form whose
/// `<inner>` contains a literal `&`. The `&` references the
/// .group / .peer class itself; everything BEFORE it becomes
/// ancestor context (descendant combinator), everything AFTER it
/// glues to the .group / .peer class as a compound suffix.
///
/// Examples (group form):
///   * `group-[&:focus]` -> `.group:focus .<candidate>`
///   * `group-[&[data-open]]` -> `.group[data-open] .<candidate>`
///   * `group-[.in-foo_&]` -> `.in-foo .group .<candidate>`
fn resolve_group_peer_with_amp(
    kind: GroupKind,
    inner_normalized: &str,
    modifier: Option<&str>,
    cx: &VariantContext,
) -> Option<ResolvedVariant> {
    let amp_idx = inner_normalized.find('&')?;
    let prefix = &inner_normalized[..amp_idx];
    let suffix = &inner_normalized[amp_idx + 1..];
    let base = match kind {
        GroupKind::Group => cx.group_class(),
        GroupKind::Peer => cx.peer_class(),
    };
    let combinator = match kind {
        GroupKind::Group => " ",
        GroupKind::Peer => " ~ ",
    };
    let modifier_part = if let Some(m) = modifier {
        format!("\\/{}", escape_class_name(m))
    } else {
        String::new()
    };
    let format = if prefix.trim().is_empty() {
        format!("{base}{modifier_part}{suffix}{combinator}&")
    } else {
        // Prefix was supplied as ancestor context — emit it
        // literally before the .group/.peer chunk. Trim trailing
        // whitespace so the descendant combinator doesn't double up.
        let prefix_trimmed = prefix.trim_end();
        format!("{prefix_trimmed} {base}{modifier_part}{suffix}{combinator}&")
    };
    Some(ResolvedVariant {
        selector_formats: vec![format],
        at_rule: None,
        prepend_content: false,
        is_pseudo_element: false,
        removes_alpha_vars: &[],
        decl_value_template: None,
    })
}

/// Split a variant name into `(name-without-modifier, Some(modifier))`. The
/// modifier is the chunk after a top-level `/` *outside* any brackets —
/// e.g. `group-[.foo]/sidebar` → (`group-[.foo]`, Some("sidebar")).
fn split_modifier(name: &str) -> (&str, Option<&str>) {
    let bytes = name.as_bytes();
    let mut depth = 0i32;
    let mut last_slash: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'[' => depth += 1,
            b']' => depth -= 1,
            b'/' if depth == 0 => last_slash = Some(i),
            _ => {}
        }
    }
    if let Some(idx) = last_slash {
        return (&name[..idx], Some(&name[idx + 1..]));
    }
    (name, None)
}

/// For a single selector format like `&:hover`, return the slice after the
/// first `&`. For `& *::selection`, returns ` *::selection`. We use this to
/// glue an inner pseudo onto a `.group`/`.peer` base.
fn suffix_after_amp(format: &str) -> &str {
    if let Some(idx) = format.find('&') {
        &format[idx + 1..]
    } else {
        format
    }
}

/// Screen / responsive variants: `sm`, `md`, `lg`, `xl`, `2xl` (configured
/// names), plus `min-<screen>`, `max-<screen>`, `min-[<value>]`, `max-[<value>]`.
fn resolve_screen(name: &str, cx: &VariantContext) -> Option<ResolvedVariant> {
    if let Some(screen) = cx.screens.iter().find(|s| s.name == name) {
        if screen.is_raw {
            // `{ raw: '(min-aspect-ratio: 1/10)' }` — emit verbatim
            // as the media query body. Tailwind formats slashes
            // with surrounding spaces (`1 / 10`), but the
            // conformance harness's normalizer collapses whitespace
            // so we emit the source form as-is.
            return Some(at_rule_only_string(format!("@media {}", screen.value)));
        }
        return Some(at_rule_only_string(format!(
            "@media (min-width: {})",
            screen.value
        )));
    }
    if let Some(rest) = name.strip_prefix("min-") {
        // Tailwind 3.4.19's `min` matchVariant has NO named-values table —
        // only the arbitrary `min-[<value>]` form is accepted. Bare
        // `min-md:flex` produces no output in the oracle, so we mirror by
        // refusing to resolve it (the caller emits an unknown-variant
        // diagnostic).
        let arb = arbitrary_inner(rest)?;
        // Upstream disables `min-*`/`max-*` arbitrary variants when
        // the screens config is complex (any object screen) or has
        // mixed units across simple-string screens. Refuse to
        // resolve so the candidate becomes UnknownVariant — the
        // emitter drops it.
        if cx.disable_minmax_arbitrary_screen {
            return None;
        }
        if !candidate_unit_matches_screens(arb, cx) {
            return None;
        }
        let value = replace_normalize_underscores(arb);
        return Some(at_rule_only_string(format!("@media (min-width: {value})")));
    }
    if let Some(rest) = name.strip_prefix("max-") {
        if let Some(arb) = arbitrary_inner(rest) {
            if cx.disable_minmax_arbitrary_screen {
                return None;
            }
            if !candidate_unit_matches_screens(arb, cx) {
                return None;
            }
            let value = replace_normalize_underscores(arb);
            return Some(at_rule_only_string(format!("@media (max-width: {value})")));
        }
        // `max-md` etc.: Tailwind inverts the screen — `not all and
        // (min-width: 768px)`.
        let screen = cx.screens.iter().find(|s| s.name == rest)?;
        return Some(at_rule_only_string(format!(
            "@media not all and (min-width: {})",
            screen.value
        )));
    }
    None
}

/// Reject `min-[<value>]` / `max-[<value>]` arbitrary variants
/// whose unit doesn't match the screens config's unit. Mirrors
/// upstream's `minmax-have-mixed-units` warning — when all simple-
/// string screens share a unit (e.g. all `px`) and the candidate
/// uses a different unit (`min-[700rem]`), Tailwind warns and
/// skips emission. Same-unit candidates (or unitless / non-px
/// values like `theme()` calls) pass through.
fn candidate_unit_matches_screens(arb_value: &str, cx: &VariantContext) -> bool {
    let candidate_unit = match extract_length_unit(arb_value) {
        Some(u) => u,
        None => return true,
    };
    let mut screen_units: std::collections::HashSet<&'static str> =
        std::collections::HashSet::new();
    for s in &cx.screens {
        if s.is_raw {
            continue;
        }
        if let Some(u) = extract_length_unit(&s.value) {
            screen_units.insert(u);
        }
    }
    if !screen_units.is_empty() {
        return screen_units.contains(candidate_unit);
    }
    // No screen units to anchor against — fall back to the
    // pre-computed `minmax_allowed_unit`. Set when the candidate
    // set itself uses mixed units (the first alphabetically wins
    // upstream); unset means there's no conflict to resolve and
    // every candidate is allowed.
    match cx.minmax_allowed_unit.as_deref() {
        Some(u) => candidate_unit == u,
        None => true,
    }
}

/// True when the user's `theme.screens` configuration would cause
/// upstream Tailwind to disable arbitrary `min-[<value>]` and
/// `max-[<value>]` variants. The criteria mirror upstream's
/// `setupContextUtils.js`:
///
/// 1. Any screen value is an object (e.g. `{ min: '700px' }`,
///    `{ raw: '...' }`, `{ min, max }`) — "complex" config.
/// 2. The string-form screens use more than one CSS length unit
///    (mixing `px` and `rem`, etc.) — "mixed-units" config.
fn detect_complex_or_mixed_screens(config: &Value) -> bool {
    let theme = match config.get("theme") {
        Some(t) => t,
        None => return false,
    };
    let screens = theme
        .get("screens")
        .or_else(|| theme.get("extend").and_then(|e| e.get("screens")));
    let Some(screens) = screens.and_then(Value::as_object) else {
        return false;
    };
    let mut units = std::collections::HashSet::new();
    for (_name, value) in screens {
        if value.is_object() {
            // Any object form → "complex" — disable.
            return true;
        }
        if let Some(s) = value.as_str() {
            if let Some(unit) = extract_length_unit(s) {
                units.insert(unit);
                if units.len() > 1 {
                    return true;
                }
            }
        }
    }
    false
}

/// Extract a CSS length unit suffix from a value like `"640px"`,
/// `"48rem"`. Returns `None` if the value isn't a `<number><unit>`
/// shape (e.g. `theme()` calls, var() refs, or empty strings).
fn extract_length_unit(value: &str) -> Option<&'static str> {
    const UNITS: &[&str] = &[
        "px", "rem", "em", "vh", "vw", "%", "ch", "ex", "pt", "cm", "mm",
    ];
    UNITS.iter().copied().find(|unit| value.ends_with(unit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> VariantContext {
        VariantContext::default()
    }

    #[test]
    fn hover_resolves_to_pseudo_class() {
        let r = resolve_variant(&ParsedVariant::Named("hover"), &ctx()).unwrap();
        assert_eq!(r.selector_formats, vec!["&:hover".to_string()]);
        assert!(r.at_rule.is_none());
    }

    #[test]
    fn before_marks_prepend_content() {
        let r = resolve_variant(&ParsedVariant::Named("before"), &ctx()).unwrap();
        assert!(r.prepend_content);
        assert_eq!(r.selector_formats, vec!["&::before".to_string()]);
    }

    #[test]
    fn marker_emits_two_formats() {
        let r = resolve_variant(&ParsedVariant::Named("marker"), &ctx()).unwrap();
        assert_eq!(r.selector_formats.len(), 2);
    }

    #[test]
    fn md_screen_resolves_to_min_width_media() {
        let r = resolve_variant(&ParsedVariant::Named("md"), &ctx()).unwrap();
        assert!(r.selector_formats.is_empty());
        assert_eq!(r.at_rule.as_deref(), Some("@media (min-width: 768px)"));
    }

    #[test]
    fn max_md_inverts_screen() {
        let r = resolve_variant(&ParsedVariant::Named("max-md"), &ctx()).unwrap();
        assert_eq!(
            r.at_rule.as_deref(),
            Some("@media not all and (min-width: 768px)")
        );
    }

    #[test]
    fn min_arbitrary_uses_value_directly() {
        let r = resolve_variant(&ParsedVariant::Named("min-[600px]"), &ctx()).unwrap();
        assert_eq!(r.at_rule.as_deref(), Some("@media (min-width: 600px)"));
    }

    #[test]
    fn dark_default_is_media() {
        let r = resolve_variant(&ParsedVariant::Named("dark"), &ctx()).unwrap();
        assert_eq!(
            r.at_rule.as_deref(),
            Some("@media (prefers-color-scheme: dark)")
        );
    }

    #[test]
    fn dark_class_uses_is_selector() {
        let mut cx = ctx();
        cx.dark_mode = DarkMode::Class(".dark".into());
        let r = resolve_variant(&ParsedVariant::Named("dark"), &cx).unwrap();
        assert_eq!(r.selector_formats, vec!["&:is(.dark *)".to_string()]);
    }

    #[test]
    fn aria_named_resolves_via_theme_shortcut() {
        let r = resolve_variant(&ParsedVariant::Named("aria-checked"), &ctx()).unwrap();
        assert_eq!(
            r.selector_formats,
            vec!["&[aria-checked=\"true\"]".to_string()]
        );
    }

    #[test]
    fn aria_arbitrary_inserts_value_quoted() {
        let r = resolve_variant(&ParsedVariant::Named("aria-[busy=true]"), &ctx()).unwrap();
        assert_eq!(
            r.selector_formats,
            vec!["&[aria-busy=\"true\"]".to_string()]
        );
    }

    #[test]
    fn data_arbitrary_without_value_keeps_bare_attr() {
        let r = resolve_variant(&ParsedVariant::Named("data-[disabled]"), &ctx()).unwrap();
        assert_eq!(r.selector_formats, vec!["&[data-disabled]".to_string()]);
    }

    #[test]
    fn data_arbitrary_quotes_value_in_v34() {
        let r = resolve_variant(
            &ParsedVariant::Named("data-[orientation=horizontal]"),
            &ctx(),
        )
        .unwrap();
        assert_eq!(
            r.selector_formats,
            vec!["&[data-orientation=\"horizontal\"]".to_string()]
        );
    }

    #[test]
    fn data_arbitrary_leaves_unquoted_in_v33() {
        let mut cx = ctx();
        cx.is_v33_compat = true;
        let r =
            resolve_variant(&ParsedVariant::Named("data-[orientation=horizontal]"), &cx).unwrap();
        assert_eq!(
            r.selector_formats,
            vec!["&[data-orientation=horizontal]".to_string()]
        );
    }

    #[test]
    fn data_arbitrary_operator_unquoted_in_v33() {
        let mut cx = ctx();
        cx.is_v33_compat = true;
        let r = resolve_variant(&ParsedVariant::Named("data-[placement^=bottom]"), &cx).unwrap();
        assert_eq!(
            r.selector_formats,
            vec!["&[data-placement^=bottom]".to_string()]
        );
    }

    #[test]
    fn aria_arbitrary_unquoted_in_v33() {
        let mut cx = ctx();
        cx.is_v33_compat = true;
        let r = resolve_variant(&ParsedVariant::Named("aria-[busy=true]"), &cx).unwrap();
        assert_eq!(r.selector_formats, vec!["&[aria-busy=true]".to_string()]);
    }

    #[test]
    fn group_hover_wraps_with_group_class() {
        let r = resolve_variant(&ParsedVariant::Named("group-hover"), &ctx()).unwrap();
        assert_eq!(
            r.selector_formats,
            vec![":merge(.group):hover &".to_string()]
        );
    }

    #[test]
    fn peer_checked_uses_sibling_combinator() {
        let r = resolve_variant(&ParsedVariant::Named("peer-checked"), &ctx()).unwrap();
        assert_eq!(
            r.selector_formats,
            vec![":merge(.peer):checked ~ &".to_string()]
        );
    }

    #[test]
    fn group_with_named_modifier() {
        let r = resolve_variant(&ParsedVariant::Named("group-hover/sidebar"), &ctx()).unwrap();
        assert_eq!(
            r.selector_formats,
            vec![":merge(.group)\\/sidebar:hover &".to_string()]
        );
    }

    #[test]
    fn group_with_arbitrary_inner() {
        let r = resolve_variant(&ParsedVariant::Named("group-[.is-active]"), &ctx()).unwrap();
        assert_eq!(
            r.selector_formats,
            vec![":merge(.group).is-active &".to_string()]
        );
    }

    #[test]
    fn group_aria_wraps_with_attribute() {
        let r = resolve_variant(&ParsedVariant::Named("group-aria-checked"), &ctx()).unwrap();
        assert_eq!(
            r.selector_formats,
            vec![":merge(.group)[aria-checked=\"true\"] &".to_string()]
        );
    }

    #[test]
    fn arbitrary_variant_with_amp_substitutes_in_place() {
        let r = resolve_variant(&ParsedVariant::Arbitrary("html:has(&)"), &ctx()).unwrap();
        assert_eq!(r.selector_formats, vec!["html:has(&)".to_string()]);
    }

    #[test]
    fn arbitrary_variant_without_amp_or_at_is_dropped() {
        // Mirrors `vendor/tailwindcss-v3/tests/arbitrary-variants.test.js`,
        // "variants without & or an at-rule are ignored": Tailwind's
        // isValidVariantFormatString requires `@` or `&`.
        assert!(resolve_variant(&ParsedVariant::Arbitrary(">*"), &ctx()).is_none());
        assert!(resolve_variant(&ParsedVariant::Arbitrary("lol"), &ctx()).is_none());
        assert!(resolve_variant(&ParsedVariant::Arbitrary(":hover"), &ctx()).is_none());
    }

    #[test]
    fn arbitrary_at_rule_variant_resolves_to_at_rule() {
        let r = resolve_variant(
            &ParsedVariant::Arbitrary("@media (min-width: 999px)"),
            &ctx(),
        )
        .unwrap();
        assert!(r.selector_formats.is_empty());
        assert_eq!(r.at_rule.as_deref(), Some("@media (min-width: 999px)"));
    }

    #[test]
    fn supports_arbitrary_with_colon_wraps_in_parens() {
        let r = resolve_variant(&ParsedVariant::Named("supports-[display:grid]"), &ctx()).unwrap();
        assert_eq!(r.at_rule.as_deref(), Some("@supports (display:grid)"));
    }

    #[test]
    fn supports_arbitrary_without_colon_synthesizes_var() {
        let r = resolve_variant(&ParsedVariant::Named("supports-[gap]"), &ctx()).unwrap();
        assert_eq!(r.at_rule.as_deref(), Some("@supports (gap: var(--tw))"));
    }

    #[test]
    fn unknown_variant_returns_none() {
        assert!(resolve_variant(&ParsedVariant::Named("not-real"), &ctx()).is_none());
    }

    #[test]
    fn min_named_screen_does_not_resolve() {
        // Tailwind 3.4.19's `min` matchVariant has no values table, so
        // `min-md:flex` produces no oracle output. We mirror by refusing.
        assert!(resolve_variant(&ParsedVariant::Named("min-md"), &ctx()).is_none());
    }

    #[test]
    fn min_arbitrary_still_works() {
        let r = resolve_variant(&ParsedVariant::Named("min-[640px]"), &ctx()).unwrap();
        assert_eq!(r.at_rule.as_deref(), Some("@media (min-width: 640px)"));
    }

    #[test]
    fn has_arbitrary_resolves_to_has_pseudo() {
        let r = resolve_variant(&ParsedVariant::Named("has-[:focus]"), &ctx()).unwrap();
        assert_eq!(r.selector_formats, vec!["&:has(:focus)".to_string()]);
    }

    #[test]
    fn has_named_does_not_resolve() {
        // `has-foo:flex` has no theme values table — only the arbitrary
        // form is accepted. Fall through to unknown.
        assert!(resolve_variant(&ParsedVariant::Named("has-foo"), &ctx()).is_none());
    }

    #[test]
    fn group_has_wraps_with_has_on_group() {
        let r = resolve_variant(&ParsedVariant::Named("group-has-[:focus]"), &ctx()).unwrap();
        assert_eq!(
            r.selector_formats,
            vec![":merge(.group):has(:focus) &".to_string()]
        );
    }

    #[test]
    fn peer_has_wraps_with_has_on_peer_with_sibling_combinator() {
        let r = resolve_variant(&ParsedVariant::Named("peer-has-[:focus]"), &ctx()).unwrap();
        assert_eq!(
            r.selector_formats,
            vec![":merge(.peer):has(:focus) ~ &".to_string()]
        );
    }

    #[test]
    fn apply_chained_pseudos_keeps_last_variant_innermost() {
        let v = vec![ParsedVariant::Named("hover"), ParsedVariant::Named("focus")];
        let r = apply_variants(&v, ".x", &ctx()).unwrap();
        // hover then focus -> .x:focus:hover (focus innermost = nearest the class)
        assert_eq!(r.selectors, vec![".x:focus:hover".to_string()]);
        assert!(r.at_rules.is_empty());
    }

    #[test]
    fn apply_at_rule_then_pseudo_stacks_outermost_first() {
        let v = vec![ParsedVariant::Named("md"), ParsedVariant::Named("hover")];
        let r = apply_variants(&v, ".x", &ctx()).unwrap();
        assert_eq!(r.selectors, vec![".x:hover".to_string()]);
        assert_eq!(r.at_rules, vec!["@media (min-width: 768px)".to_string()]);
    }

    #[test]
    fn apply_double_at_rules_outer_then_inner() {
        let v = vec![
            ParsedVariant::Named("dark"),
            ParsedVariant::Named("md"),
            ParsedVariant::Named("hover"),
        ];
        let r = apply_variants(&v, ".x", &ctx()).unwrap();
        assert_eq!(r.selectors, vec![".x:hover".to_string()]);
        assert_eq!(
            r.at_rules,
            vec![
                "@media (prefers-color-scheme: dark)".to_string(),
                "@media (min-width: 768px)".to_string(),
            ]
        );
    }

    #[test]
    fn apply_group_hover_emits_descendant_selector() {
        let v = vec![ParsedVariant::Named("group-hover")];
        let r = apply_variants(&v, ".gh\\:flex", &ctx()).unwrap();
        assert_eq!(r.selectors, vec![".group:hover .gh\\:flex".to_string()]);
    }

    #[test]
    fn apply_marker_emits_two_selectors_one_at_rule() {
        let v = vec![ParsedVariant::Named("marker")];
        let r = apply_variants(&v, ".x", &ctx()).unwrap();
        assert_eq!(r.selectors.len(), 2);
        assert!(r.selectors.iter().any(|s| s == ".x *::marker"));
        assert!(r.selectors.iter().any(|s| s == ".x::marker"));
    }

    #[test]
    fn apply_before_with_hover_orders_correctly_and_prepends_content() {
        let v = vec![
            ParsedVariant::Named("before"),
            ParsedVariant::Named("hover"),
        ];
        let r = apply_variants(&v, ".x", &ctx()).unwrap();
        assert_eq!(r.selectors, vec![".x:hover::before".to_string()]);
        assert_eq!(r.prepend_decls, vec![("content", "var(--tw-content)")]);
    }

    #[test]
    fn apply_arbitrary_with_amp_substitutes_class() {
        let v = vec![ParsedVariant::Arbitrary("html:has(&)")];
        let r = apply_variants(&v, ".x", &ctx()).unwrap();
        assert_eq!(r.selectors, vec!["html:has(.x)".to_string()]);
    }

    #[test]
    fn split_modifier_recognizes_top_level_slash() {
        assert_eq!(split_modifier("group-hover"), ("group-hover", None));
        assert_eq!(
            split_modifier("group-hover/sidebar"),
            ("group-hover", Some("sidebar"))
        );
        // Slash inside `[...]` is NOT a modifier boundary.
        assert_eq!(
            split_modifier("data-[state=open/foo]"),
            ("data-[state=open/foo]", None)
        );
        assert_eq!(
            split_modifier("group-[.is-active]/sidebar"),
            ("group-[.is-active]", Some("sidebar"))
        );
    }
}
