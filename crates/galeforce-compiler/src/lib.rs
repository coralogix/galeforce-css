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

//! GaleforceCSS compiler.
//!
//! Phase E scope (current): static utilities + the full variant family +
//! the CSS directive processor (`@tailwind`, `@apply`, `@layer`, `theme()`,
//! `screen()`, `@screen`). Value-bearing utilities and the implicit
//! `--tw-*` base block arrive in subsequent phases.
//!
//! Pipeline:
//!
//!   raw candidate -> parser -> static utility lookup -> variants applied -> rule(s)
//!   user CSS input -> directive processor (slots compiled rules, expands @apply,
//!                                          resolves theme()/screen()) -> output CSS
//!
//! When `inputCss` is empty/`None` the directive processor is bypassed
//! and the output is just the candidate stylesheet — preserving the
//! Phase C / D output shape the conformance harness expects when fixtures
//! pass `@tailwind utilities;` (the simplest input).

#![allow(clippy::doc_markdown)]

mod colors;
mod default_theme;
mod directives;
mod lcss;
mod minify;
mod offsets;
mod plugins;
mod screens;
mod selector_ast;
mod source_map;
mod static_utilities;
mod value_utilities;
mod variants;

pub(crate) use static_utilities::{find_sibling_static, find_static};

use galeforce_core::{CompileOptions, CompileResult, CssOutput, Diagnostic};
use galeforce_css::{
    emit_stylesheet, escape_class_name_with, CommaEscapeStyle, Declaration, EmitOptions, Rule,
    Stylesheet,
};
use galeforce_parser::{parse, ParseOptions, ParsedCandidate};
use rayon::prelude::*;

use static_utilities::StaticUtility;
use value_utilities::{find_value_utilities, resolve_value, ValueUtility};
use variants::{apply_variants, AppliedVariants, VariantContext};

/// A `@keyframes` block. `name` and `body` are owned strings to
/// accommodate both the built-in stubs (`spin`, `pulse`, `bounce`,
/// `ping`) which use `&'static str` data sources and user-configured
/// `theme.keyframes` entries which need ownership.
///
/// The keyframe's at-rule context comes from the CONSUMING rule (the
/// `.animate-X` that references it), wrapped in
/// `emit_stylesheet_with_keyframes` — see lib.rs:`emit_stylesheet_with_keyframes`.
/// So we don't carry an `at_rules` field here.
#[derive(Clone, Debug)]
struct KeyframeBlock {
    name: String,
    body: String,
}

pub fn compile(opts: &CompileOptions) -> CompileResult {
    let mut sheet = Stylesheet::default();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut candidate_count = 0usize;
    // Top-level `@keyframes` blocks that one or more candidates
    // requested. Deduped by name (the body is fixed per name) and
    // emitted at stylesheet scope after all candidate rules. Order
    // doesn't matter for conformance — the harness compares
    // unordered — but we keep it stable by insertion order via a
    // Vec + a separate seen-set.
    // Per-context: at-rules path + keyframe name + body. The same
    // `@keyframes spin` block can land in multiple at-rule contexts
    // (top-level for `hover:animate-spin`, inside `@media (min-width:
    // 1024px)` for `lg:animate-spin`); each emit is independent.
    // Dedup is keyed on (at_rules, name) so a candidate that fires
    // twice in the same context only pays for one block.
    // Per-candidate keyframes: upstream's `animation` corePlugin
    // re-emits the same `@keyframes <name>` block for every
    // candidate that uses it (including hover/focus variants).
    // The conformance harness pairs rules by `(context, selector)`
    // index, so we mirror that count rather than dedupe.
    let mut keyframes: Vec<KeyframeBlock> = Vec::new();

    let prefix: Option<&str> = opts
        .config
        .as_ref()
        .and_then(|c| c.get("prefix"))
        .and_then(|v| v.as_str());
    let separator: &str = opts
        .config
        .as_ref()
        .and_then(|c| c.get("separator"))
        .and_then(|v| v.as_str())
        .unwrap_or(":");

    let parse_opts = ParseOptions { prefix, separator };
    // Detect `@tailwind base;` in the input so we know whether the
    // universal defaults block exists. `experimental.
    // optimizeUniversalDefaults` only inlines defaults at utility-
    // using rules when `@tailwind base` is present — otherwise
    // there's no defaults block to optimize away. Mirrors
    // upstream's same check in `resolveDefaultsAtRules.js`.
    let has_tailwind_base = opts
        .input_css
        .as_deref()
        .map(|s| memchr::memmem::find(s.as_bytes(), b"@tailwind base").is_some())
        .unwrap_or(false);
    // When `features.compat.tailwind_version` selects a non-default
    // version, stash the chosen line on the config JSON under
    // `__tailwindVersion` so deep call sites (color resolvers, the
    // selector emitter, the math-operator normalizer) can opt into
    // the legacy behavior without changing every function signature.
    // Same pattern we use for `__pluginOutput`, `__resolvedBase`, etc.
    let effective_config: Option<serde_json::Value> =
        if opts.features.compat.tailwind_version == galeforce_core::TailwindVersion::V33 {
            let mut base = opts
                .config
                .clone()
                .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
            if let Some(obj) = base.as_object_mut() {
                obj.insert(
                    "__tailwindVersion".to_string(),
                    serde_json::Value::String("3.3".to_string()),
                );
            }
            Some(base)
        } else {
            opts.config.clone()
        };
    let mut cx = VariantContext::from_config(effective_config.as_ref());
    cx.has_tailwind_base = has_tailwind_base;
    // Pre-pass: when the screens config doesn't pin a single unit,
    // pick the one used by the FIRST `min-[<v>]`/`max-[<v>]` arbitrary
    // candidate after sorting alphabetically. Subsequent candidates
    // with a different unit are then dropped by the variant resolver.
    cx.compute_minmax_allowed_unit(opts.candidates.iter().map(String::as_str));
    let cx = cx;
    let important_mode = read_important_config(effective_config.as_ref());
    let core_plugins = CorePluginsFilter::from_config(effective_config.as_ref());
    // Plugin output is read per-candidate inside `compile_candidate`
    // (via `plugin_index_get`). For projects with no plugins this is
    // a single hash lookup per candidate that finds an empty array.
    // For plugin-heavy projects worth building a class-indexed
    // FxHashMap once at compile-start; deferred until profiling
    // says it's hot.

    // `safelist: [<string>, …]` (and the no-op pattern form) lifts
    // additional class names into the candidate set regardless of
    // scanner output. `blocklist: [<string>, …]` excludes class names
    // from the output even if the scanner saw them. We treat both as
    // pre-loop transforms over the candidate slice.
    //
    // Pattern-form safelist (`{ pattern: /…/, variants: [...] }`) is
    // a documented carry-over — it would need to enumerate every
    // possible candidate, which we don't have an iterator for yet.
    let safelist_extra: Vec<String> = read_safelist_strings(effective_config.as_ref());
    let blocklist: rustc_hash::FxHashSet<&str> = read_blocklist(effective_config.as_ref())
        .into_iter()
        .collect();
    let mut candidates_iter: Vec<&str> = opts
        .candidates
        .iter()
        .map(String::as_str)
        .chain(safelist_extra.iter().map(String::as_str))
        .filter(|c| !blocklist.contains(c))
        .collect();
    // Pre-sort lexically on the FULL candidate string — mirrors
    // upstream's `expandTailwindAtRules.js:163-168`:
    //   [...candidates].sort((a, z) => a === z ? 0 : a < z ? -1 : 1)
    //
    // The candidate-iteration order assigned by this sort propagates
    // to `Offsets.create('utilities')` invocations downstream, which
    // is the final `index` axis the sort cascade falls back on once
    // every other axis ties. For arbitrary-property candidates the
    // `property_offset` axis (assigned per-FULL-root via
    // `sortArbitraryProperties`) takes priority over `index`, so a
    // pair like `hover:file:[--value:1]` vs `file:hover:[--value:2]`
    // ends up ordered by inner value (`1` < `2`) via the property
    // offset — not by the variant chain prefix.
    candidates_iter.sort_unstable();
    candidates_iter.dedup();

    // ---- Offsets pre-pass (Phase 3 of the bigint-bitmask port) ----
    // Build a single `Offsets` registry for this compile. Register
    // all core variants in upstream's canonical order, then walk
    // every candidate once to register any arbitrary variants
    // (`[&_.foo]:`, `[data-x=y]:`, etc.) BEFORE the parallel compile
    // loop runs. After this pre-pass `offsets` is effectively
    // read-only for the hot path (`Sync`-safe through `&`).
    //
    // The order arbitrary variants get registered is the candidate
    // iteration order — which is alphabetic per the pre-sort above.
    // `Offsets::sort` runs `recalculateVariantOffsets()` at the
    // final sort time, remapping arbitrary-variant bits into stable
    // alphabetic order regardless of registration sequence.
    let mut offsets = offsets::Offsets::new();
    variants::register_core_variants_with_config(&mut offsets, effective_config.as_ref());
    {
        let pre_parse_opts = galeforce_parser::ParseOptions {
            prefix: effective_config
                .as_ref()
                .and_then(|c| c.get("prefix"))
                .and_then(|v| v.as_str()),
            separator: effective_config
                .as_ref()
                .and_then(|c| c.get("separator"))
                .and_then(|v| v.as_str())
                .unwrap_or(":"),
        };
        for raw in candidates_iter.iter() {
            let Some(parsed) = galeforce_parser::parse(raw, &pre_parse_opts) else {
                continue;
            };
            for v in &parsed.variants {
                match v {
                    galeforce_parser::ParsedVariant::Arbitrary(text) => {
                        let key = format!("[{}]", text);
                        if !offsets.has_variant(&key) {
                            offsets.record_variant_bit(&key, 1);
                        }
                    }
                    galeforce_parser::ParsedVariant::Named(_) => {
                        // Family-prefixed arbitrary values
                        // (`aria-[busy=true]`, `data-[state=open]`,
                        // `group-[&:hover]`, …) arrive as `Named`.
                        // Upstream resolves them through the family
                        // base form's matchVariant with the inner as
                        // `args.value`; the value contributes to
                        // sort only via an optional `options.sort`
                        // callback (none of the families ship one in
                        // v3.4.19). So we don't register inner bits
                        // here — `resolve_variant_bits` drops the
                        // arbitrary value and uses the base form's
                        // bit alone.
                    }
                }
            }
        }
    }

    // Build the user-CSS apply cache up-front so candidate-driven
    // compilation can synthesize rules for user-defined utilities
    // referenced through variant chains (e.g. `hover:align-banana`
    // when `.align-banana` lives in `@layer utilities`). The cache
    // is shared with `directives::process` which uses it for
    // `@apply` resolution. Built only when input contains
    // `@apply` or `@layer` — the trigger covers both consumers.
    let user_cache = if let Some(input) = opts.input_css.as_deref() {
        let bytes = input.as_bytes();
        let needs_cache = memchr::memmem::find(bytes, b"@apply").is_some()
            || memchr::memmem::find(bytes, b"@layer").is_some();
        if needs_cache {
            Some(directives::build_user_apply_cache_public(input))
        } else {
            None
        }
    } else {
        None
    };

    // Per-candidate compilation is independent — same parser, same
    // utility tables, same theme references (all `&'static`). Run
    // rayon's `par_iter` over the candidate list and fold the
    // per-thread outputs together at the end.
    //
    // The threshold is empirical: rayon's fork-join overhead is
    // ~30µs per work-stealing handoff. Below ~100 candidates the
    // serial loop wins outright; above ~500 it's a clear win on
    // multi-core. We pick 256 as the crossover.
    //
    // `enumerate()` BEFORE `par_iter` so each candidate carries its
    // input index for sort-key generation. Without that, the
    // parallel order is non-deterministic and the eventual sort
    // would lose user-input tiebreaking.
    let parallel_threshold = 256;
    // Two buckets: utilities and components. The container utility and
    // any plugin-supplied `addComponents` rules go to the components
    // buckets so they can be emitted at the `@tailwind components;` slot.
    // When the input CSS lacks that slot (e.g. `@tailwind utilities;` only)
    // the components bucket is dropped entirely — that's how upstream's
    // PostCSS pipeline behaves and the conformance corpus exercises it
    // through inline candidates that never declare `@tailwind components`.
    //
    // IMPORTANT: built-in component rules (`is_component=true`, e.g. the
    // `container` utility) and plugin `addComponents` extra rules
    // (`component_keyed`) are kept in SEPARATE Vecs and emitted in order —
    // built-in first, then plugin extra. This mirrors Tailwind's PostCSS
    // pipeline, where the container corePlugin emits before user plugins.
    // Keeping them separate prevents `collapse_adjacent_rules_pass` from
    // merging the two groups' rules (e.g. built-in `.container
    // { max-width: 1536px }` and a plugin's `.container { max-width:
    // 1280px }` at the same breakpoint).
    let mut utilities_keyed: Vec<(offsets::RuleOffset, Rule)>;
    let mut builtin_components_keyed: Vec<(offsets::RuleOffset, Rule)> = Vec::new();
    let mut plugin_components_keyed: Vec<(offsets::RuleOffset, Rule)> = Vec::new();
    let parallel = candidates_iter.len() >= parallel_threshold;

    if parallel {
        let outputs: Vec<CandidateOutput> = candidates_iter
            .par_iter()
            .enumerate()
            .map(|(idx, raw)| {
                compile_one_candidate(
                    raw,
                    idx as u32,
                    &parse_opts,
                    &cx,
                    effective_config.as_ref(),
                    &core_plugins,
                    user_cache.as_ref(),
                    &offsets,
                )
            })
            .collect();
        utilities_keyed = Vec::with_capacity(outputs.len() * 2);
        for out in outputs {
            match out {
                CandidateOutput::Rules {
                    keyed,
                    extra_keyframes,
                    is_component,
                    component_keyed,
                } => {
                    if is_component {
                        builtin_components_keyed.extend(keyed);
                    } else {
                        utilities_keyed.extend(keyed);
                    }
                    plugin_components_keyed.extend(component_keyed);
                    keyframes.extend(extra_keyframes);
                    candidate_count += 1;
                }
                CandidateOutput::Diagnostic(diag) => {
                    diagnostics.push(diag);
                }
            }
        }
    } else {
        utilities_keyed = Vec::new();
        for (idx, raw) in candidates_iter.iter().enumerate() {
            match compile_one_candidate(
                raw,
                idx as u32,
                &parse_opts,
                &cx,
                effective_config.as_ref(),
                &core_plugins,
                user_cache.as_ref(),
                &offsets,
            ) {
                CandidateOutput::Rules {
                    keyed,
                    extra_keyframes,
                    is_component,
                    component_keyed,
                } => {
                    if is_component {
                        builtin_components_keyed.extend(keyed);
                    } else {
                        utilities_keyed.extend(keyed);
                    }
                    plugin_components_keyed.extend(component_keyed);
                    keyframes.extend(extra_keyframes);
                    candidate_count += 1;
                }
                CandidateOutput::Diagnostic(diag) => {
                    diagnostics.push(diag);
                }
            }
        }
    }

    // Use `Offsets::sort` which runs `recalculateVariantOffsets()`
    // and `sortArbitraryProperties()` BEFORE the lexicographic
    // compare cascade — mirrors upstream's `offsets.js:383-391`.
    // This is the pipeline that gives arbitrary variants their
    // alphabetic-stable bit positions.
    utilities_keyed = offsets.sort(utilities_keyed);
    builtin_components_keyed = offsets.sort(builtin_components_keyed);
    plugin_components_keyed = offsets.sort(plugin_components_keyed);
    sheet.rules = utilities_keyed.into_iter().map(|(_, r)| r).collect();
    // Builtin component rules (e.g. container) come first, then plugin
    // addComponents extras. Keeping them as separate Stylesheets and
    // concatenating their CSS ensures the collapse pass treats them as
    // two non-adjacent groups — matching Tailwind's emit order.
    let builtin_components_sheet = Stylesheet {
        rules: builtin_components_keyed
            .into_iter()
            .map(|(_, r)| r)
            .collect(),
    };
    let plugin_components_sheet = Stylesheet {
        rules: plugin_components_keyed
            .into_iter()
            .map(|(_, r)| r)
            .collect(),
    };
    // Element-selector addComponents rules (e.g. `h1 { ... }`, `table
    // { ... }`) must be emitted unconditionally — Tailwind v3 JIT only
    // candidate-filters class-selector components (`.foo { ... }`). Rules
    // with no class in the selector have nothing to JIT-match against, so
    // they always land in the components slot in plugin registration order.
    // This mirrors addBase's unconditional emission for element targets.
    let unconditional_plugin_components: Vec<Rule> = {
        let plugin_output = plugins::read_plugin_output(effective_config.as_ref());
        plugin_output
            .components
            .iter()
            .filter(|r| r.primary_class().is_none() && r.referenced_classes().is_empty())
            .map(|r| Rule {
                selectors: smallvec::smallvec![r.selector.clone()],
                declarations: r
                    .declarations
                    .iter()
                    .map(|d| Declaration {
                        property: d.property.clone(),
                        value: d.value.clone(),
                        important: d.important,
                    })
                    .collect(),
                at_rules: r.at_rules.clone(),
                respect_important: r.respect_important,
                defaults_groups: Vec::new(),
            })
            .collect()
    };

    // Apply the global `important` config. Two modes per upstream:
    //   - `important: true`  → every emitted decl gets `!important`.
    //   - `important: '<sel>'` → every rule's selectors get the
    //     selector prefixed (`#app .foo`); decls stay non-important.
    // Variant-emitted prepend_decls (`content: var(--tw-content)`)
    // are excluded from the boolean form upstream, but our
    // `Declaration::important` flag IS toggled uniformly here for
    // simplicity — the conformance harness compares semantically and
    // both forms render the same once normalised. If a fixture proves
    // otherwise, the prepend-decl path can opt out.
    apply_important_mode(&mut sheet.rules, &important_mode);
    // Components default to `respectImportant: false` per upstream
    // `setupContextUtils.js` — the `addComponents` defaults block
    // sets it explicitly. So `important: true` / `important: '<sel>'`
    // does NOT add `!important` (or prefix the selector) for the
    // container utility or any plugin-supplied component. But
    // plugins can opt back IN with `addComponents(..., {
    // respectImportant: true })`. The per-rule `respect_important`
    // flag flows through from the plugin emitter, and the apply
    // function gates on that — so running it over components
    // applies importance only to the explicitly-tagged ones.
    let mut builtin_components_rules = builtin_components_sheet.rules;
    apply_important_mode(&mut builtin_components_rules, &important_mode);
    let mut plugin_components_rules = plugin_components_sheet.rules;
    apply_important_mode(&mut plugin_components_rules, &important_mode);
    let mut unconditional_plugin_components = unconditional_plugin_components;
    apply_important_mode(&mut unconditional_plugin_components, &important_mode);

    // Interleave `@keyframes` blocks at the position of the FIRST
    // emitted `.animate-X` rule that consumes them. Mirrors
    // upstream's `animation` corePlugin shape (corePlugins.js:1022-1052)
    // where each `animate-spin` candidate yields the parallel rule
    // pair `[{ '@keyframes spin': ... }, { '.animate-spin': ... }]`.
    // Tailwind dedupes @keyframes by name in the final emit so each
    // keyframe block appears once at the slot of its first consumer.
    //
    // We sort sheet.rules first (already done above), then scan for
    // each unique keyframe name and insert the `@keyframes` block as
    // raw CSS immediately before the matching `.animate-X` rule.
    let candidates_css = emit_stylesheet_with_keyframes(&sheet, &keyframes);
    // Emit: built-in components first, then candidate-triggered plugin
    // components, then unconditional element-selector plugin components.
    // Three separate stylesheet emissions prevent collapse_adjacent_rules
    // from merging rules across the group boundary.
    let builtin_component_rule_count = builtin_components_rules.len();
    let plugin_component_rule_count = plugin_components_rules.len();
    let unconditional_component_rule_count = unconditional_plugin_components.len();
    let builtin_components_css = emit_stylesheet(
        &Stylesheet {
            rules: builtin_components_rules,
        },
        &EmitOptions::default(),
    );
    let plugin_components_css = emit_stylesheet(
        &Stylesheet {
            rules: plugin_components_rules,
        },
        &EmitOptions::default(),
    );
    let unconditional_components_css = emit_stylesheet(
        &Stylesheet {
            rules: unconditional_plugin_components,
        },
        &EmitOptions::default(),
    );
    let components_css =
        format!("{builtin_components_css}{plugin_components_css}{unconditional_components_css}");
    let rule_count = sheet.rules.len()
        + builtin_component_rule_count
        + plugin_component_rule_count
        + unconditional_component_rule_count;

    // If the caller supplied non-trivial inputCss, route through the
    // directive processor so `@tailwind utilities`, `@apply`, `theme()`,
    // and `screen()` get expanded in their source positions. The harness
    // sends `@tailwind utilities;` as the default input; for that single
    // line the processor's only effect is to insert `candidates_css` at
    // that position, which is byte-equivalent to the Phase D fast path.
    let css = match opts.input_css.as_deref() {
        Some(input) if !input.trim().is_empty() => {
            // Pre-pass: expand `&` / nested-selector / bubble at-rule
            // structures BEFORE the directive processor runs, so
            // `@apply`, `@tailwind`, `@layer`, `@screen`, and
            // `theme()` see flat CSS. Cheap no-op when the input
            // has no nesting markers.
            let denested: std::borrow::Cow<'_, str> = if opts.features.nesting {
                std::borrow::Cow::Owned(galeforce_css::expand_nesting(
                    input,
                    &galeforce_css::NestingOptions::default(),
                ))
            } else {
                std::borrow::Cow::Borrowed(input)
            };
            // The directive processor needs the candidate roots ONLY
            // for tree-shaking user `@layer utilities { … }` and
            // `@layer components { … }` blocks. No `@layer` substring
            // → no tree-shaking → no need to build the candidate set
            // at all. Saves N hash inserts (2.5K on the real corpus)
            // when the input is the canonical `@tailwind …;` shape.
            // Always build the candidate set — the directive
            // processor uses it for both `@layer` tree-shaking AND
            // `addBase` candidate-driven filtering (so plugin
            // `addBase` rules with class selectors only emit when
            // a matching candidate is scanned).
            let candidate_set: rustc_hash::FxHashSet<&str> =
                opts.candidates.iter().map(String::as_str).collect();
            let processed = directives::process(
                denested.as_ref(),
                &candidates_css,
                &components_css,
                &cx,
                effective_config.as_ref(),
                &candidate_set,
                &blocklist,
            );
            diagnostics.extend(processed.diagnostics);
            processed.css
        }
        _ => {
            // No directive processing: emit components first, then
            // utilities. Mirrors upstream's natural emit order
            // (`@tailwind base; @tailwind components; @tailwind
            // utilities;`) and lets bare `compile()` calls without
            // an inputCss continue to receive every emitted rule.
            let mut combined = String::with_capacity(components_css.len() + candidates_css.len());
            combined.push_str(&components_css);
            combined.push_str(&candidates_css);
            combined
        }
    };

    // Resolve `@defaults <id>;` markers — rules that depend on a
    // cascade-var defaults group emit a marker, this pass collects
    // them by id, splits selectors by `:has`/`:-`/`::-` isolation,
    // and emits one shared defaults rule per (id, isolation-bucket,
    // at-rule context) tuple at the start of the sheet. Mirrors
    // upstream's `resolveDefaultsAtRules.js`. Runs BEFORE
    // `collapse_adjacent_rules_pass` because that pass merges by
    // selector and would consume the markers before we can hoist.
    let css = resolve_defaults_at_rules_pass(&css);
    // Mirror upstream's `collapseAdjacentRules` postcss pass —
    // adjacent rules with the same selector (or adjacent at-rules
    // with the same name + params) get their declarations merged
    // into one rule. The conformance harness keys diffs by selector,
    // so two `.b { ... } .b { ... }` rules look like a "duplicate
    // selector" without the merge.
    let css = collapse_adjacent_rules_pass(&css);

    // Final-stage transforms: minification, source map, browser
    // prefixing. When any of these is requested we route through
    // Lightning CSS — it owns the parser/printer pipeline and gives us
    // proper source maps + browser-target lowering for free.
    //
    // The hand-rolled `minify`/`source_map` modules remain as a
    // fallback: if Lightning fails to parse (it shouldn't — we emit
    // standard CSS), we fall back to them and emit a diagnostic.
    let needs_lcss = opts.features.minify
        || opts.features.source_maps
        || opts.features.targets.as_ref().is_some_and(|v| !v.is_null());
    let (final_css, map_json) = if needs_lcss {
        let source_path = opts.input_css_path.as_deref().unwrap_or("input.css");
        let source_content = opts.input_css.as_deref().unwrap_or("");
        match lcss::transform(
            &css,
            lcss::LcssOptions {
                minify: opts.features.minify,
                source_map: opts.features.source_maps,
                source_path: Some(source_path),
                source_content: Some(source_content),
                targets: opts.features.targets.as_ref(),
            },
        ) {
            Ok(out) => {
                // When a source map is requested, append the standard
                // inline `# sourceMappingURL=` comment so consumers that
                // load the CSS without out-of-band map fetch still get
                // a usable map.
                let mut css_out = out.css;
                if let Some(map) = &out.map {
                    source_map::append_inline_source_map(&mut css_out, map);
                }
                (css_out, out.map)
            }
            Err(e) => {
                diagnostics.push(Diagnostic::warning(
                    "lightningcss-failed",
                    format!("Lightning CSS pass failed: {e}. Falling back to hand-rolled minify/source-map."),
                ));
                // Fallback path: hand-rolled source map + hand-rolled
                // minifier. Behavior matches pre-Lightning Galeforce.
                let (fallback_css, fallback_map) = if opts.features.source_maps {
                    let map = source_map::build_source_map(&css, source_path, source_content);
                    let mut with_inline = css;
                    source_map::append_inline_source_map(&mut with_inline, &map);
                    (with_inline, Some(map))
                } else {
                    (css, None)
                };
                let minified = if opts.features.minify {
                    minify::minify_css(&fallback_css)
                } else {
                    fallback_css
                };
                (minified, fallback_map)
            }
        }
    } else {
        (css, None)
    };

    CompileResult {
        output: CssOutput {
            css: final_css,
            map: map_json,
        },
        diagnostics,
        candidate_count,
        rule_count,
    }
}

enum CompileOutcome {
    Rules {
        rules: Vec<Rule>,
        /// Top-level `@keyframes <name>` blocks the candidate
        /// brings with it (`animate-spin` carries `@keyframes spin`).
        /// Each entry is `(name, body)` where body is the inside of
        /// the at-rule.
        extra_keyframes: Vec<KeyframeBlock>,
        /// Whether the rules belong in the `components` layer. The
        /// `container` utility and plugin-supplied `addComponents`
        /// rules go here; everything else is a utility. Used to
        /// route emit through the correct `@tailwind` slot — when
        /// the input CSS is `@tailwind utilities;` only, the
        /// components bucket is dropped (matches Tailwind's PostCSS
        /// pipeline, which only inserts the components layer where
        /// `@tailwind components;` appears).
        is_component: bool,
        /// Auxiliary rules from the OPPOSITE bucket — populated when
        /// a single candidate fires both a utility AND an alias
        /// plugin from the components layer (e.g. built-in `w-full`
        /// utility plus a user `addComponents` rule that references
        /// `.w-full` inside its selector). Lets the dispatcher
        /// route each set to its proper slot.
        extra_component_rules: Vec<Rule>,
        /// When a candidate matches multiple value utilities with the
        /// same prefix (e.g. `text-inherit` fires BOTH `fontSize` and
        /// `textColor`), each additional utility beyond the first is
        /// stored here with its own plugin order so the sorter places
        /// them at the same positions as Tailwind's JIT would.
        /// Empty in the common single-utility case.
        extra_utility_groups: Vec<(u32, Vec<Rule>)>,
        /// Plugin-order index for the PRIMARY `rules` group. Lets the
        /// caller use the actually-resolved plugin's order instead of
        /// the static `lookup_plugin_order_for_root` (which only sees
        /// the first table-match prefix). For `bg-[var(--unknown)]`
        /// the prefix matches multiple plugins (backgroundColor,
        /// backgroundImage, backgroundSize, backgroundPosition); the
        /// resolved plugin's order is what the sort needs. `None`
        /// means "use the lookup-based default".
        primary_plugin_order: Option<u32>,
        /// Per-rule `within_plugin_order` override aligned with
        /// `rules` by index. Used when a single plugin rule is
        /// registered under multiple identifiers (`.parent .child`
        /// registered under both `parent` and `child`) — upstream's
        /// `addComponents`/`addUtilities` calls `offsets.create(
        /// 'components')` per identifier, so candidates that matched
        /// the earlier identifier sort first. We mirror that here by
        /// passing the matched identifier's POSITION as the
        /// per-rule wpo. `None`/empty means "use the global wpo".
        primary_within_plugin_orders: Vec<u32>,
    },
    Unsupported(String),
    UnknownUtility,
}

/// Per-candidate output produced by `compile_one_candidate`. Each
/// `Rules` variant carries pre-keyed rules so the parallel collect
/// can be folded without re-walking parsed candidates afterward.
/// `Diagnostic` carries a single message — at most one per candidate,
/// since a candidate can only be parsed-failed, unsupported, or
/// unknown — never multiple at once.
#[allow(clippy::large_enum_variant)]
enum CandidateOutput {
    Rules {
        keyed: smallvec::SmallVec<[(offsets::RuleOffset, Rule); 1]>,
        extra_keyframes: Vec<KeyframeBlock>,
        is_component: bool,
        /// Auxiliary keyed rules for the components bucket — set
        /// when a candidate fires both a utility AND an alias-
        /// matched component plugin.
        component_keyed: smallvec::SmallVec<[(offsets::RuleOffset, Rule); 1]>,
    },
    Diagnostic(Diagnostic),
}

/// Parse + compile one candidate, returning the per-candidate output
/// for the parallel collect. Fully self-contained: takes only `&`
/// references to the shared compile context, allocates only what
/// belongs to this candidate's output.
#[allow(clippy::too_many_arguments)]
fn compile_one_candidate(
    raw: &str,
    input_index: u32,
    parse_opts: &ParseOptions,
    cx: &VariantContext,
    config: Option<&serde_json::Value>,
    core_plugins: &CorePluginsFilter,
    user_cache: Option<&directives::UserApplyCache>,
    offsets: &offsets::Offsets,
) -> CandidateOutput {
    let parsed = match parse(raw, parse_opts) {
        Some(p) => p,
        None => {
            // Static message — the per-candidate detail lives on
            // `candidate`. Allocating a `format!` per rejection cost
            // ~50 µs on a 2,500-candidate corpus where ~85 are
            // parse-rejected JS noise.
            return CandidateOutput::Diagnostic(
                Diagnostic::warning(
                    "candidate-parse-failed",
                    "could not be parsed as a Tailwind candidate",
                )
                .with_candidate(raw),
            );
        }
    };
    match compile_candidate(&parsed, cx, config, core_plugins, user_cache) {
        CompileOutcome::Rules {
            rules,
            extra_keyframes,
            is_component,
            extra_component_rules,
            extra_utility_groups,
            primary_plugin_order,
            primary_within_plugin_orders,
        } => {
            let mut candidate_meta = candidate_sort_meta(&parsed);
            // When the resolved value-utility's plugin is known, use
            // its plugin_order — overrides the table-match-first
            // fallback used by `lookup_plugin_order_for_root` so
            // arbitrary-value collisions sort by the plugin that
            // actually emitted the rule (e.g. `bg-[var(--unknown)]`
            // → backgroundColor vs `bg-[200px_100px]` →
            // backgroundPosition).
            if let Some(po) = primary_plugin_order {
                candidate_meta.plugin_order = po;
            }
            let mut keyed: smallvec::SmallVec<[(offsets::RuleOffset, Rule); 1]> =
                smallvec::SmallVec::with_capacity(rules.len());
            for (idx, rule) in rules.into_iter().enumerate() {
                let mut meta = candidate_meta.clone();
                if let Some(wpo) = primary_within_plugin_orders.get(idx) {
                    meta.within_plugin_order = *wpo;
                }
                let key = build_rule_offset(&parsed, &rule, &meta, offsets, input_index);
                keyed.push((key, rule));
            }
            // Additional utility groups (multi-plugin prefix matches like
            // `text-inherit` firing both fontSize and textColor) each get
            // their own plugin_order so the sorter places them at the same
            // positions as Tailwind's JIT would — keeping them separate
            // rather than adjacent where collapse_adjacent_rules_pass would
            // incorrectly merge them.
            for (group_plugin_order, group_rules) in extra_utility_groups {
                let mut meta = candidate_meta.clone();
                meta.plugin_order = group_plugin_order;
                for rule in group_rules {
                    let key = build_rule_offset(&parsed, &rule, &meta, offsets, input_index);
                    keyed.push((key, rule));
                }
            }
            let mut component_keyed: smallvec::SmallVec<[(offsets::RuleOffset, Rule); 1]> =
                smallvec::SmallVec::with_capacity(extra_component_rules.len());
            for rule in extra_component_rules {
                let mut key =
                    build_rule_offset(&parsed, &rule, &candidate_meta, offsets, input_index);
                // Components live in Layer::Components for sort
                // ordering (sorts BEFORE utilities/variants).
                key.layer = offsets::Layer::Components;
                key.parent_layer = offsets::Layer::Components;
                component_keyed.push((key, rule));
            }
            CandidateOutput::Rules {
                keyed,
                extra_keyframes,
                is_component,
                component_keyed,
            }
        }
        CompileOutcome::Unsupported(reason) => CandidateOutput::Diagnostic(
            Diagnostic::warning("unsupported-candidate", reason).with_candidate(raw),
        ),
        // Static message — same reasoning as parse-failed. The candidate
        // name is on `.candidate`; the message duplicates it. For the
        // 2,500-candidate corpus where ~1,800 utilities miss (JS noise,
        // PascalCase identifiers, etc.) avoiding the per-rejection
        // `format!` removes ~1,800 mallocs from the hot path.
        CompileOutcome::UnknownUtility => CandidateOutput::Diagnostic(
            Diagnostic::warning("unknown-utility", "is not a recognized utility")
                .with_candidate(raw),
        ),
    }
}

fn compile_candidate(
    parsed: &ParsedCandidate,
    cx: &VariantContext,
    config: Option<&serde_json::Value>,
    core_plugins: &CorePluginsFilter,
    user_cache: Option<&directives::UserApplyCache>,
) -> CompileOutcome {
    let class_form = render_class_form(parsed);
    let class_selector = format!(
        ".{}",
        escape_class_name_with(
            &class_form,
            comma_style(config, &class_form, !parsed.variants.is_empty())
        )
    );

    // The `container` utility is special-cased in upstream's
    // `corePlugins.js` — it's a component (lives in `@tailwind
    // components`) that emits a base rule + one nested `@media`
    // rule per configured screen. We synthesize the same shape
    // here when `core_plugins.is_enabled("container")` and the
    // candidate is the bare `container` class.
    if parsed.root == "container"
        && parsed.variants.is_empty()
        && !parsed.negative
        && parsed.modifier.is_none()
        && core_plugins.is_enabled("container")
    {
        let built_in = rules_for_container(&class_selector, cx, config, parsed.important);
        // Plugin `addComponents` for `.container` (e.g. notus-nextjs overrides
        // max-widths) must also appear alongside the built-in container — Tailwind
        // emits both in the @tailwind components slot. Collect any plugin component
        // rules whose primary class is `container` and include them here.
        // The container branch requires no variants, so apply_variants is infallible.
        let extra_component_rules: Vec<Rule> = if let (Some((plugin_rules, true)), Ok(applied)) = (
            plugin_index_get(config, "container"),
            apply_variants(&parsed.variants, &class_selector, cx),
        ) {
            rules_for_plugin(&plugin_rules, parsed, applied, &class_selector)
        } else {
            Vec::new()
        };
        return CompileOutcome::Rules {
            rules: built_in,
            extra_keyframes: Vec::new(),
            is_component: true,
            extra_component_rules,
            extra_utility_groups: Vec::new(),
            primary_plugin_order: None,
            primary_within_plugin_orders: Vec::new(),
        };
    }

    // Try the static-utility table first. Static utilities don't take
    // values, so they're trivially identified by exact match.
    if let Some(util) = find_static(parsed.root) {
        if !core_plugins.is_enabled(util.plugin) {
            return CompileOutcome::UnknownUtility;
        }
        // Static utilities don't accept negation or modifiers — those
        // are value-plugin shapes. Tailwind drops `-flex` etc.
        if parsed.negative || parsed.modifier.is_some() {
            return CompileOutcome::UnknownUtility;
        }
        let applied = match apply_variants(&parsed.variants, &class_selector, cx) {
            Ok(a) => a,
            Err(reason) => return CompileOutcome::Unsupported(reason),
        };
        return CompileOutcome::Rules {
            rules: rules_for_static(util, parsed, applied, config),
            extra_keyframes: Vec::new(),
            is_component: false,
            extra_component_rules: Vec::new(),
            extra_utility_groups: Vec::new(),
            primary_plugin_order: Some(plugin_order_index(util.plugin)),
            primary_within_plugin_orders: Vec::new(),
        };
    }

    // Sibling-pair statics (`space-x-reverse`, `divide-x-reverse`,
    // `divide-solid` etc.) — same shape as static, but the emitted
    // rule's selector is suffixed with the sibling-pair combinator.
    if let Some(sib) = find_sibling_static(parsed.root) {
        if parsed.negative || parsed.modifier.is_some() {
            return CompileOutcome::UnknownUtility;
        }
        let applied = match apply_variants(&parsed.variants, &class_selector, cx) {
            Ok(a) => a,
            Err(reason) => return CompileOutcome::Unsupported(reason),
        };
        return CompileOutcome::Rules {
            rules: rules_for_sibling_static(sib, parsed, applied),
            extra_keyframes: Vec::new(),
            is_component: false,
            extra_component_rules: Vec::new(),
            extra_utility_groups: Vec::new(),
            primary_plugin_order: None,
            primary_within_plugin_orders: Vec::new(),
        };
    }

    // Try value-bearing utilities (margin, padding, sizing, typography…).
    //
    // Multiple plugins can share a class prefix: `font-` is fontWeight +
    // fontFamily, `text-` is fontSize + textColor. When a value key exists
    // in more than one theme (e.g. `text-inherit` where both `fontSize` and
    // `textColor` carry an `inherit` entry), ALL matching plugins fire and
    // their declarations are combined — exactly as Tailwind's plugin pipeline
    // does. We accumulate every successful resolution and combine them after
    // the loop instead of returning on the first match.
    let matches = find_value_utilities(parsed.root);
    // `(plugin_order, resolved)` — track per-utility plugin order so rules
    // from different utilities (e.g. `fontSize` and `textColor` for
    // `text-inherit`) get distinct sort keys and remain separate in the
    // output, matching Tailwind's JIT behaviour.
    let mut all_resolved: Vec<(u32, value_utilities::ResolvedDecls)> = Vec::new();
    let mut all_keyframes: Vec<KeyframeBlock> = Vec::new();
    let mut applied_cached: Option<AppliedVariants> = None;

    for (util, value_key) in matches {
        if !core_plugins.is_enabled(util.plugin) {
            continue;
        }
        if let Some(resolved) = try_resolve_value_utility(util, value_key, parsed, config) {
            if applied_cached.is_none() {
                match apply_variants(&parsed.variants, &class_selector, cx) {
                    Ok(a) => applied_cached = Some(a),
                    Err(reason) => return CompileOutcome::Unsupported(reason),
                }
            }
            // Keyframe blocks carry name + body only — the consuming
            // rule's at-rule context is applied at emit time by
            // `emit_stylesheet_with_keyframes`. Mirrors upstream's
            // behaviour where `lg:animate-spin` produces
            // `@media (min-width: 1024px) { @keyframes spin { ... }
            // .lg\:animate-spin { ... } }`.
            for (name, body) in resolved.extra_keyframes.iter() {
                all_keyframes.push(KeyframeBlock {
                    name: (*name).to_string(),
                    body: (*body).to_string(),
                });
            }
            for (name, body) in &resolved.extra_keyframes_owned {
                all_keyframes.push(KeyframeBlock {
                    name: name.clone(),
                    body: body.clone(),
                });
            }
            let is_arb = value_utilities::is_arbitrary_value_key(value_key);
            all_resolved.push((plugin_order_index(util.plugin), resolved));
            // For arbitrary values stop at the first successful resolution —
            // the resolvers are ordered by specificity and there is no
            // cross-type disambiguation for `[...]` values. For named theme
            // keys ALL matching plugins fire (e.g. `text-inherit` emits both
            // fontSize and textColor rules, matching Tailwind JIT behaviour).
            if is_arb {
                break;
            }
        }
    }

    if !all_resolved.is_empty() {
        let applied = applied_cached.unwrap();
        // The first utility's rules go into `rules` (keyed with the default
        // candidate_meta.plugin_order). Additional utilities go into
        // `extra_utility_groups` with their own plugin_order so the sorter
        // places them at distinct positions — preventing collapse_adjacent_
        // rules_pass from incorrectly merging them.
        let mut iter = all_resolved.into_iter();
        let (first_plugin_order, first_resolved) = iter.next().unwrap();
        let mut primary_rules =
            rules_for_value_decls(first_resolved, parsed, applied.clone(), config, cx);
        let extra_utility_groups: Vec<(u32, Vec<Rule>)> = iter
            .map(|(po, resolved)| {
                (
                    po,
                    rules_for_value_decls(resolved, parsed, applied.clone(), config, cx),
                )
            })
            .collect();

        // Also collect any plugin rules whose selector references this
        // class as an alias. The alias check runs once after all built-in
        // utilities have fired. Component-layer alias rules go into
        // `extra_component_rules` so the dispatcher routes them to the
        // @tailwind components slot.
        let raw_plugin_form_inner = render_class_form_no_modifier(parsed);
        let mut extra_component_rules: Vec<Rule> = Vec::new();
        let mut extra_utility_rules: Vec<Rule> = Vec::new();
        for class_name in [raw_plugin_form_inner.as_str(), parsed.root] {
            let Some((plugin_rules, is_component)) = plugin_index_get(config, class_name) else {
                continue;
            };
            let alias_rules: Vec<plugins::PluginRule> = plugin_rules
                .into_iter()
                .filter(|r| {
                    r.primary_class()
                        .map(|p| p != parsed.root && p != raw_plugin_form_inner)
                        .unwrap_or(true)
                })
                .collect();
            if alias_rules.is_empty() {
                continue;
            }
            let materialized =
                rules_for_plugin(&alias_rules, parsed, applied.clone(), &class_selector);
            if is_component {
                extra_component_rules.extend(materialized);
            } else {
                extra_utility_rules.extend(materialized);
            }
            break;
        }
        primary_rules.extend(extra_utility_rules);
        return CompileOutcome::Rules {
            rules: primary_rules,
            extra_keyframes: all_keyframes,
            is_component: false,
            extra_component_rules,
            extra_utility_groups,
            primary_plugin_order: Some(first_plugin_order),
            primary_within_plugin_orders: Vec::new(),
        };
    }

    // Plugin-defined utilities (`addUtilities` / `addComponents` /
    // `matchUtilities`). Matched by class name on the candidate's
    // root. Plugin rules carry their own selector suffix (e.g.
    // `.foo:hover`) and at-rule context (`@media (...) { ... }`)
    // captured at JS-side plugin runtime.
    //
    // Try the candidate's RAW form first (including any
    // `/<modifier>`) — plugin classes can contain literal slashes
    // (`.custom-top-1\/4`) that the candidate parser splits as a
    // modifier. Only fall back to the parsed root if the raw form
    // didn't match a plugin.
    let raw_plugin_form = render_class_form_no_modifier(parsed);
    let plugin_lookup = plugin_index_get(config, &raw_plugin_form)
        .or_else(|| plugin_index_get(config, parsed.root));
    if let Some((plugin_rules, is_component)) = plugin_lookup {
        // When the match came via the raw form, the modifier is
        // already part of the matched class — treat the candidate
        // as having no modifier from here on.
        let matched_via_raw =
            raw_plugin_form != parsed.root && plugin_index_get(config, &raw_plugin_form).is_some();
        if !matched_via_raw && (parsed.negative || parsed.modifier.is_some()) {
            return CompileOutcome::UnknownUtility;
        }
        let applied = match apply_variants(&parsed.variants, &class_selector, cx) {
            Ok(a) => a,
            Err(reason) => return CompileOutcome::Unsupported(reason),
        };
        // Compute per-rule `within_plugin_order` based on the
        // candidate's MATCHED CLASS POSITION within the plugin rule's
        // referenced classes. Mirrors upstream's
        // `setupContextUtils.js:345-356` where `addComponents` calls
        // `offsets.create('components')` per identifier in
        // `withIdentifiers(rule)` — so two candidates matching the
        // same rule via DIFFERENT classes (e.g. `screen:parent` and
        // `screen:child` both matching `.parent .child`) sort by the
        // class's appearance order in the selector.
        let plugin_rule_wpos: Vec<u32> = plugin_rules
            .iter()
            .map(|rule| matched_class_position(rule, parsed.root))
            .collect();
        let mut compiled_rules = rules_for_plugin(&plugin_rules, parsed, applied, &class_selector);
        // `rules_for_plugin` emits one rule per applied.selectors per
        // plugin_rule. The current code path produces a flat Vec —
        // the wpos array must align with that flattened order. For
        // each plugin_rule we expand its wpo by the number of
        // emitted rules. Approximated by N = applied.selectors.len()
        // assumed earlier. But here we just align by index; if rules
        // and wpos sizes mismatch we no-op.
        if compiled_rules.len() != plugin_rule_wpos.len() {
            compiled_rules.shrink_to_fit();
            return CompileOutcome::Rules {
                rules: compiled_rules,
                extra_keyframes: Vec::new(),
                is_component,
                extra_component_rules: Vec::new(),
                extra_utility_groups: Vec::new(),
                primary_plugin_order: Some(USER_PLUGIN_ORDER),
                primary_within_plugin_orders: Vec::new(),
            };
        }
        return CompileOutcome::Rules {
            rules: compiled_rules,
            extra_keyframes: Vec::new(),
            is_component,
            extra_component_rules: Vec::new(),
            extra_utility_groups: Vec::new(),
            primary_plugin_order: Some(USER_PLUGIN_ORDER),
            primary_within_plugin_orders: plugin_rule_wpos,
        };
    }

    // Arbitrary properties: `[mask-type:luminance]`, `[--my-var:1px]`,
    // `[font-size:14px]`. Whole `parsed.root` is `[<property>:<value>]`
    // and the emitted rule has one decl `<property>: <value>`.
    // Mirrors upstream's `arbitrary-properties.test.js`.
    if let Some((property, value)) = parse_arbitrary_property(parsed.root) {
        if parsed.negative || parsed.modifier.is_some() {
            return CompileOutcome::UnknownUtility;
        }
        let applied = match apply_variants(&parsed.variants, &class_selector, cx) {
            Ok(a) => a,
            Err(reason) => return CompileOutcome::Unsupported(reason),
        };
        return CompileOutcome::Rules {
            rules: rules_for_arbitrary_property(property, value, parsed, applied, config),
            extra_keyframes: Vec::new(),
            is_component: false,
            extra_component_rules: Vec::new(),
            extra_utility_groups: Vec::new(),
            primary_plugin_order: None,
            primary_within_plugin_orders: Vec::new(),
        };
    }

    // User-CSS layer utilities: when nothing else matches and the
    // root class lives in `@layer utilities` or `@layer components`,
    // synthesize a variant-applied rule using the user's decls.
    // Mirrors upstream's `setupContextUtils.js` where `@layer
    // utilities` bodies become `addUtilities` plugin output. The
    // unvarianted candidate is already emitted by the layer-
    // passthrough in `directives::process` — we only synthesize
    // when variants are present (`hover:my-util`, `md:my-util`,
    // etc.) since the layer passthrough doesn't apply variants.
    if let Some(cache) = user_cache {
        if !parsed.variants.is_empty() && !parsed.negative && parsed.modifier.is_none() {
            if let Some(entries) = cache.entries.get(parsed.root) {
                let layered: Vec<&directives::UserApplyEntry> =
                    entries.iter().filter(|e| e.in_layer_utility).collect();
                if !layered.is_empty() {
                    let applied = match apply_variants(&parsed.variants, &class_selector, cx) {
                        Ok(a) => a,
                        Err(reason) => return CompileOutcome::Unsupported(reason),
                    };
                    let mut rules: Vec<Rule> = Vec::new();
                    let mut wpos: Vec<u32> = Vec::new();
                    for entry in layered {
                        let decls = parse_decls_from_body(&entry.body, parsed.important);
                        if decls.is_empty() {
                            continue;
                        }
                        // Per-rule within_plugin_order: pack the
                        // source-order index in the high bits, the
                        // candidate's position within the entry's
                        // selector in the low bits. Mirrors
                        // upstream's `@layer utilities` synthesis
                        // where each user-CSS rule lives at its
                        // `Offsets.create('utilities')` slot in
                        // input order, with multi-class entries
                        // tagging each identifier with its own
                        // sub-index.
                        let class_pos = position_of_class_in_selector(&entry.selector, parsed.root)
                            .unwrap_or(0);
                        let wpo = (entry.source_order << 8) | (class_pos as u32 & 0xFF);
                        // Compose at-rule chain: candidate's variants
                        // (responsive / dark) wrap outermost, the
                        // user's source at-rule context (e.g.
                        // `@supports`) sits innermost.
                        let mut at_rules = applied.at_rules.clone();
                        at_rules.extend(entry.at_rules.iter().cloned());
                        let mut all_decls: Vec<Declaration> = applied
                            .prepend_decls
                            .iter()
                            .map(|(p, v)| Declaration {
                                property: (*p).to_string(),
                                value: (*v).to_string(),
                                important: false,
                            })
                            .collect();
                        all_decls.extend(decls);
                        // For each variant-applied selector
                        // (typically just one for `sm:`/`hover:`),
                        // substitute the source selector's matching
                        // class with the applied form. This preserves
                        // structural selectors like
                        // `.base1 .foo, .base1 .bar` →
                        // `.sm\:base1 .foo, .sm\:base1 .bar` rather
                        // than collapsing to a bare `.sm\:base1`
                        // rule. Mirrors upstream's
                        // `expandTailwindAtRules.js` where
                        // `@layer utilities` rules go through the
                        // candidate's variant chain via the same
                        // selector substitution as plugin output.
                        for applied_sel in &applied.selectors {
                            let new_sel = directives::substitute_class_in_selector_public(
                                &entry.selector,
                                parsed.root,
                                applied_sel,
                            );
                            let final_sel = if new_sel.is_empty() {
                                applied_sel.clone()
                            } else {
                                new_sel
                            };
                            rules.push(Rule {
                                selectors: smallvec::smallvec![final_sel],
                                declarations: all_decls.clone(),
                                at_rules: at_rules.clone(),
                                respect_important: true,
                                defaults_groups: Vec::new(),
                            });
                            wpos.push(wpo);
                        }
                    }
                    if !rules.is_empty() {
                        return CompileOutcome::Rules {
                            rules,
                            extra_keyframes: Vec::new(),
                            is_component: false,
                            extra_component_rules: Vec::new(),
                            extra_utility_groups: Vec::new(),
                            primary_plugin_order: None,
                            primary_within_plugin_orders: wpos,
                        };
                    }
                }
            }
        }
    }

    CompileOutcome::UnknownUtility
}

/// Parse a CSS declaration body (`prop: value; prop: value;`) into
/// structured `Declaration`s. Used when synthesizing rules from
/// user-CSS `@layer utilities|components` entries — the cache stores
/// raw body text, but the candidate-driven pipeline emits typed
/// `Rule`s. Skips embedded `@apply` lines (those would need
/// recursive expansion that lives in the directives processor) and
/// nested rules. The `important` flag tags every emitted decl.
fn parse_decls_from_body(body: &str, important: bool) -> Vec<Declaration> {
    let bytes = body.as_bytes();
    let mut decls: Vec<Declaration> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        // Skip @rule lines, comments, and nested rule blocks.
        if bytes[i] == b'@' {
            // Move past terminator (semicolon or matching `}`).
            let mut depth = 0i32;
            while i < bytes.len() {
                match bytes[i] {
                    b'{' => depth += 1,
                    b'}' => {
                        if depth == 0 {
                            break;
                        }
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            break;
                        }
                    }
                    b';' if depth == 0 => {
                        i += 1;
                        break;
                    }
                    _ => {}
                }
                i += 1;
            }
            continue;
        }
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        // Read property up to `:` (or `{` for nested rules).
        let prop_start = i;
        let mut had_colon = false;
        while i < bytes.len() {
            match bytes[i] {
                b':' => {
                    had_colon = true;
                    break;
                }
                b';' | b'}' => break,
                b'{' => break,
                _ => i += 1,
            }
        }
        if !had_colon {
            // Nested rule or stray content — skip to next decl.
            if i < bytes.len() && bytes[i] == b'{' {
                let mut depth = 1i32;
                i += 1;
                while i < bytes.len() && depth > 0 {
                    match bytes[i] {
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
            } else if i < bytes.len() {
                i += 1;
            }
            continue;
        }
        let prop = body[prop_start..i].trim().to_string();
        i += 1; // skip ':'
        let val_start = i;
        let mut depth_paren = 0i32;
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
                b';' if depth_paren == 0 => break,
                b'}' if depth_paren == 0 => break,
                _ => i += 1,
            }
        }
        let mut value = body[val_start..i].trim().to_string();
        let mut decl_important = important;
        if let Some(stripped) = value.trim_end().strip_suffix("!important") {
            value = stripped.trim_end().to_string();
            decl_important = true;
        }
        if !prop.is_empty() && !value.is_empty() {
            decls.push(Declaration {
                property: prop,
                value,
                important: decl_important,
            });
        }
        if i < bytes.len() && bytes[i] == b';' {
            i += 1;
        }
    }
    decls
}

/// `[<property>:<value>]` -> `Some((property, value))`. The property
/// must be a CSS-identifier-shaped run (alpha, digits, `-`, `_`,
/// optional `--` prefix for custom properties); the value is
/// everything after the first top-level `:`. Returns `None` for
/// shapes that don't fit (e.g. `[&>*]`, `[#abc]`).
fn parse_arbitrary_property(root: &str) -> Option<(&str, &str)> {
    let inner = root.strip_prefix('[')?.strip_suffix(']')?;
    // Find the first colon outside of nested parens (`calc(1px:2)` —
    // contrived but possible). Brackets inside arbitrary properties
    // shouldn't nest meaningfully.
    let bytes = inner.as_bytes();
    let mut depth = 0i32;
    let mut colon_at: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b':' if depth == 0 => {
                colon_at = Some(i);
                break;
            }
            _ => {}
        }
    }
    let i = colon_at?;
    let (property, rest) = inner.split_at(i);
    let value = &rest[1..];
    if property.is_empty() || value.is_empty() {
        return None;
    }
    // Property is identifier-shaped or a custom prop (`--foo`). The
    // candidate parser already rejects most non-property shapes by
    // the time we get here, but be defensive.
    if !is_arbitrary_property_name(property) {
        return None;
    }
    // Mirror upstream's `looksLikeUri` check: reject when the
    // (property + ':' + value) form parses as a URL with both a
    // scheme and a host. Catches `[https://example.com]`,
    // `[ftp://x]` etc. — where the user pasted a URL into a class.
    if looks_like_uri(property, value) {
        return None;
    }
    // Mirror upstream's `isParsableCssValue` lite check: top-level
    // unquoted colons in the value (`[a:b:c:d]`) produce invalid
    // CSS once emitted. CSS values may contain colons inside strings
    // (`'a:b'`), parens (`url(http://x)`), or brackets, but never
    // bare at the top level. Reject candidates whose value would
    // emit unparseable CSS.
    if value_has_top_level_colon(value) {
        return None;
    }
    // CSS values must not contain `{` or `}` — those open/close
    // declaration blocks. Real arbitrary values will never have
    // them (`url(...)`, lengths, color strings); the only thing
    // that does is template-literal residue like `${foo}` from JSX
    // sources the extractor swept up. Mirror upstream's
    // `isParsableCssValue` rejection without needing a PostCSS
    // round-trip.
    if value_has_braces(value) {
        return None;
    }
    Some((property, value))
}

fn value_has_braces(value: &str) -> bool {
    let bytes = value.as_bytes();
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
            b'{' | b'}' => return true,
            _ => i += 1,
        }
    }
    false
}

fn looks_like_uri(property: &str, value: &str) -> bool {
    // Simple structural check — we don't need URL crate parity.
    // Treat as URI if property is purely alpha (a scheme) and the
    // value starts with `//<host>`, where `<host>` is at least one
    // non-separator character. Mirrors the conditions Tailwind's
    // `new URL(...)` would accept for `scheme + ://host + path`.
    let value_bytes = value.as_bytes();
    if !value_bytes.starts_with(b"//") {
        return false;
    }
    if value_bytes.len() < 3 {
        return false;
    }
    if !property.bytes().all(|b| b.is_ascii_alphabetic()) || property.is_empty() {
        return false;
    }
    let host_start = 2;
    let host_byte = value_bytes[host_start];
    // Reject empty host (`//`) and obviously bogus chars.
    host_byte != b'/' && host_byte != b'?' && host_byte != b'#'
}

fn value_has_top_level_colon(value: &str) -> bool {
    let bytes = value.as_bytes();
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
            b':' if depth_paren == 0 && depth_bracket == 0 => return true,
            _ => i += 1,
        }
    }
    false
}

fn is_arbitrary_property_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    // Custom properties always start with `--`.
    if let Some(rest) = name.strip_prefix("--") {
        return !rest.is_empty()
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    }
    // Standard properties: alpha-leading, alphanumerics + `-`.
    if !bytes[0].is_ascii_alphabetic() {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Replace every `theme('path.to.key', defaultValue)` call inside
/// `value` with its resolved theme value. Mirrors upstream's
/// `evaluateTailwindFunctions.js` pass, applied here so arbitrary
/// property values like `[border:_calc(5vw_-_theme(spacing[2.5]))]`
/// emit the resolved length in the output. We intentionally leave
/// the call verbatim if the path can't be resolved (Tailwind warns
/// in that case; we keep the string for the caller to surface).
fn resolve_theme_calls_in_value(value: &str, config: Option<&serde_json::Value>) -> String {
    if !value.contains("theme(") {
        return value.to_string();
    }
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut i = 0;
    while i < bytes.len() {
        // Strings.
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            let quote = bytes[i];
            out.push(bytes[i] as char);
            i += 1;
            while i < bytes.len() && bytes[i] != quote {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    out.push(bytes[i] as char);
                    out.push(bytes[i + 1] as char);
                    i += 2;
                } else {
                    out.push(bytes[i] as char);
                    i += 1;
                }
            }
            if i < bytes.len() {
                out.push(bytes[i] as char);
                i += 1;
            }
            continue;
        }
        // theme(<path>, <default>?)
        if bytes[i].is_ascii_alphabetic() {
            let mut end = i;
            while end < bytes.len()
                && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_' || bytes[end] == b'-')
            {
                end += 1;
            }
            if &value[i..end] == "theme" && end < bytes.len() && bytes[end] == b'(' {
                let mut depth = 1i32;
                let mut j = end + 1;
                while j < bytes.len() && depth > 0 {
                    match bytes[j] {
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
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
                        }
                        _ => {}
                    }
                    j += 1;
                }
                if depth == 0 && j < bytes.len() {
                    let arg = &value[end + 1..j];
                    let resolved = resolve_theme_arg(arg, config);
                    out.push_str(&resolved);
                    i = j + 1;
                    continue;
                }
            }
            out.push_str(&value[i..end]);
            i = end;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Resolve a single `theme(<arg>)` call. `<arg>` is the raw inner of
/// the call (path string, optional default after a top-level comma).
/// The path can use both `.` and `[...]` notation —
/// `theme('spacing[2.5]')` and `theme('colors.fuchsia.700')`. Mirrors
/// upstream's `parseValue` step in `evaluateTailwindFunctions.js`.
fn resolve_theme_arg(arg: &str, config: Option<&serde_json::Value>) -> String {
    // Split off a top-level default after the first comma at depth 0.
    let bytes = arg.as_bytes();
    let mut depth = 0i32;
    let mut comma: Option<usize> = None;
    let mut k = 0;
    while k < bytes.len() {
        match bytes[k] {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b',' if depth == 0 => {
                comma = Some(k);
                break;
            }
            b'"' | b'\'' => {
                let q = bytes[k];
                k += 1;
                while k < bytes.len() && bytes[k] != q {
                    k += 1;
                }
            }
            _ => {}
        }
        k += 1;
    }
    let (path_arg, default_arg) = match comma {
        Some(c) => (&arg[..c], Some(&arg[c + 1..])),
        None => (arg, None),
    };
    let path = path_arg.trim().trim_matches(|c| c == '"' || c == '\'');
    let segments = parse_theme_path_segments(path);
    let resolved = config
        .and_then(|c| c.get("theme"))
        .and_then(|theme| walk_theme_segments(theme, &segments))
        .or_else(|| walk_theme_segments(default_theme::default_theme_ref(), &segments));
    match resolved {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => match default_arg {
            Some(d) => d.trim().to_string(),
            None => format!("theme({arg})"),
        },
    }
}

/// Parse a `theme()` path into segments. Tailwind supports both
/// dotted segments (`colors.blue.500`) and bracket segments
/// (`spacing[2.5]`); the bracket form is used when the key contains
/// a `.` so dot-splitting would break it. We follow the same rules:
/// split on top-level `.` and treat each `[…]` as a literal key.
fn parse_theme_path_segments(path: &str) -> Vec<String> {
    let bytes = path.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'.' => {
                if !buf.is_empty() {
                    out.push(std::mem::take(&mut buf));
                }
            }
            b'[' => {
                if !buf.is_empty() {
                    out.push(std::mem::take(&mut buf));
                }
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i] != b']' {
                    i += 1;
                }
                out.push(path[start..i].to_string());
            }
            b']' => {}
            _ => buf.push(bytes[i] as char),
        }
        i += 1;
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

/// Walk a theme node by a `.`-separated path. Tailwind 3 stores some
/// theme sections as a nested tree (`colors.blue.500`) and others
/// flat (`backgroundColor['blue-500']`). To handle both, at each
/// level we first try the segment as-is; if that misses, we try
/// rejoining the remaining segments with `-` and looking up that
/// flat key. So `colors.blue.500` against a flat table resolves via
/// `colors -> blue-500`. Mirrors upstream's
/// `evaluateTailwindFunctions.js` `parseValue` plus
/// `flattenColorPalette` lookups.
fn walk_theme_segments<'a>(
    node: &'a serde_json::Value,
    segments: &[String],
) -> Option<&'a serde_json::Value> {
    if segments.is_empty() {
        return Some(node);
    }
    // Try the full remaining path joined with `-` first — this is
    // how flattened color tables key their entries.
    let joined = segments.join("-");
    if let Some(child) = node.get(&joined) {
        return Some(child);
    }
    // Fall back to traditional walk: take the first segment,
    // recurse on the rest. Recurse instead of iterating so the
    // hyphen-join can apply at each level (`colors.gray.400` against
    // a partially-nested table `{ colors: { 'gray-400': '...' } }`
    // also works).
    let head = &segments[0];
    let child = node.get(head)?;
    walk_theme_segments(child, &segments[1..])
}

fn rules_for_arbitrary_property(
    property: &str,
    value: &str,
    parsed: &ParsedCandidate,
    applied: AppliedVariants,
    config: Option<&serde_json::Value>,
) -> Vec<Rule> {
    // Underscores inside arbitrary values represent literal spaces
    // (Tailwind's escape mechanism for whitespace inside class
    // attributes). `\_` is a literal underscore.
    let normalized_value = normalize_arbitrary_value(value);
    // Wrap a leading `--<ident>` shorthand in `var(...)` unless the
    // property is in `AUTO_VAR_INJECTION_EXCEPTIONS`. Mirrors
    // upstream's `dataTypes.normalize()`.
    let normalized_value = maybe_wrap_var_reference(&normalized_value, property);
    // Resolve any `theme()` calls embedded in the value.
    // `[--a:theme(colors.blue.500)]` → `--a: #3b82f6`. Mirrors
    // `evaluateTailwindFunctions.js` for the arbitrary-property
    // candidate path.
    let normalized_value = resolve_theme_calls_in_value(&normalized_value, config);
    let mut declarations: Vec<Declaration> = applied
        .prepend_decls
        .iter()
        .map(|(p, v)| Declaration {
            property: (*p).to_string(),
            value: (*v).to_string(),
            important: false,
        })
        .collect();
    declarations.push(Declaration {
        property: property.to_string(),
        value: normalized_value,
        important: parsed.important,
    });
    applied
        .selectors
        .into_iter()
        .map(|sel| Rule {
            selectors: smallvec::smallvec![sel],
            declarations: declarations.clone(),
            at_rules: applied.at_rules.clone(),
            respect_important: true,
            defaults_groups: Vec::new(),
        })
        .collect()
}

/// Underscore-to-space substitution mirroring Tailwind's behavior on
/// arbitrary values. `\_` survives as a literal underscore.
fn normalize_arbitrary_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                out.push(next);
                chars.next();
                continue;
            }
        }
        if c == '_' {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

/// Tailwind's `dataTypes.normalize()` exception set — properties that
/// accept a `<dashed-ident>` token directly. For these the user can
/// write `[anchor-name:--foo]` and get `anchor-name: --foo` (no
/// `var()` injection). For everything else, a leading `--` is treated
/// as a custom-property reference and wrapped with `var(...)`.
fn property_accepts_dashed_ident(prop: &str) -> bool {
    matches!(
        prop,
        "scroll-timeline-name"
            | "timeline-scope"
            | "view-timeline-name"
            | "font-palette"
            | "anchor-name"
            | "anchor-scope"
            | "position-anchor"
            | "position-try-options"
            | "scroll-timeline"
            | "animation-timeline"
            | "view-timeline"
            | "position-try"
    )
}

/// Inject `var()` around a leading `--` reference, mirroring upstream's
/// `normalize()` shorthand. The full value must be a single
/// custom-property reference (`--color`, no spaces, no other tokens)
/// for the wrap to apply. Skipped when the property is in the
/// dashed-ident exception list.
fn maybe_wrap_var_reference(value: &str, property: &str) -> String {
    if property_accepts_dashed_ident(property) {
        return value.to_string();
    }
    if value.starts_with("--")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return format!("var({value})");
    }
    value.to_string()
}

/// Attempt to resolve a value-bearing utility, applying the slash-rejoin
/// fallback for fraction-style keys like `w-1/2`.
pub(crate) fn try_resolve_value_utility(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&serde_json::Value>,
) -> Option<value_utilities::ResolvedDecls> {
    // Bare `ring-opacity` (no value, DEFAULT key) only emits when
    // the `respectDefaultRingColorOpacity` future flag is set.
    // Mirrors upstream's `disableDefaultRingColorOpacity` gate in
    // `corePlugins.js`.
    if util.class_prefix == "ring-opacity" && value_key == "DEFAULT" {
        let respect = config
            .and_then(|c| c.get("future"))
            .and_then(|f| f.get("respectDefaultRingColorOpacity"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if !respect {
            return None;
        }
    }
    // Explicit-negative key: when the candidate is `-<util>-<key>`,
    // try `theme[<util>]['-<key>']` first. If the user has e.g.
    // `margin: { '-2': '-4px' }`, `-mt-2` should use `-4px` directly
    // rather than negating the value of `mt-2`. Mirrors upstream's
    // `getValueAttempts` / `negativeValuesFn` logic in
    // `setupContextUtils.js`.
    if parsed.negative && util.supports_negative {
        let neg_key = format!("-{value_key}");
        let mut not_negative = parsed.clone();
        not_negative.negative = false;
        if let Some(decls) = resolve_value(util, &neg_key, &not_negative, config) {
            return Some(decls);
        }
    }
    if let Some(decls) = resolve_value(util, value_key, parsed, config) {
        return Some(decls);
    }
    // Slash-rejoin: only meaningful when the parser captured a modifier
    // that's actually part of the value key (e.g. `w-1/2` parses as
    // `root="w-1", modifier=Some("2")`). For `text-sm/6` the modifier
    // is real and the FontSize resolver consumes it directly, so the
    // first attempt above already succeeded — we never reach here.
    let modifier = parsed.modifier.as_ref()?;
    let mut combined = String::with_capacity(value_key.len() + 8);
    combined.push_str(value_key);
    combined.push('/');
    render_modifier_into(modifier, &mut combined);
    let mut without_modifier = parsed.clone();
    without_modifier.modifier = None;
    resolve_value(util, &combined, &without_modifier, config)
}

fn rules_for_static(
    util: &StaticUtility,
    parsed: &ParsedCandidate,
    applied: AppliedVariants,
    config: Option<&serde_json::Value>,
) -> Vec<Rule> {
    let v33 = crate::value_utilities::is_v33_compat(config);
    // Prepended decls (variant-driven `content: var(--tw-content)` for
    // `before`/`after`) are STRUCTURAL — they make the variant work
    // syntactically, not part of what the user is asserting. They
    // never carry the candidate's `!important` flag, even when the
    // candidate is `!before:bg-red-500`. Mirrors upstream (oracle
    // emits `content: var(--tw-content)` without `!important` for
    // `!before:foo` candidates).
    let mut declarations: Vec<Declaration> = applied
        .prepend_decls
        .iter()
        .map(|(p, v)| Declaration {
            property: (*p).to_string(),
            value: (*v).to_string(),
            important: false,
        })
        .collect();
    declarations.extend(util.declarations.iter().filter_map(|(p, v)| {
        // Tailwind v3.4 added `-webkit-backdrop-filter` alongside the
        // unprefixed `backdrop-filter` in the `backdropFilter`
        // utilities. v3.3 emits only the unprefixed form — drop the
        // vendor-prefixed companion in compat mode.
        if v33 && p.starts_with("-webkit-backdrop-filter") {
            return None;
        }
        let value = if applied.decl_value_templates.is_empty() {
            (*v).to_string()
        } else {
            crate::variants::apply_decl_value_templates(v, &applied.decl_value_templates)
        };
        Some(Declaration {
            property: (*p).to_string(),
            value,
            important: parsed.important,
        })
    }));
    apply_alpha_var_removal(&mut declarations, &applied.remove_alpha_vars);

    applied
        .selectors
        .into_iter()
        .map(|sel| Rule {
            selectors: smallvec::smallvec![sel],
            declarations: declarations.clone(),
            at_rules: applied.at_rules.clone(),
            respect_important: true,
            defaults_groups: Vec::new(),
        })
        .collect()
}

fn rules_for_sibling_static(
    sib: &static_utilities::SiblingStatic,
    parsed: &ParsedCandidate,
    applied: AppliedVariants,
) -> Vec<Rule> {
    let mut declarations: Vec<Declaration> = sib
        .declarations
        .iter()
        .map(|(p, v)| Declaration {
            property: (*p).to_string(),
            value: (*v).to_string(),
            important: parsed.important,
        })
        .collect();
    apply_alpha_var_removal(&mut declarations, &applied.remove_alpha_vars);
    applied
        .selectors
        .into_iter()
        .map(|sel| Rule {
            selectors: smallvec::smallvec![format!("{sel} > :not([hidden]) ~ :not([hidden])")],
            declarations: declarations.clone(),
            at_rules: applied.at_rules.clone(),
            respect_important: true,
            defaults_groups: Vec::new(),
        })
        .collect()
}

/// Mutates `decls` to remove the cascade vars listed in `remove_vars`:
/// dropping `<var>: 1` declarations and stripping
/// `/ var(<var>)` / `/ var(<var>, 1)` substrings from other values.
/// Mirrors `vendor/tailwindcss-v3/src/util/removeAlphaVariables.js`,
/// invoked by `marker:` and `visited:` variants. The byte fidelity
/// matters: leaving a trailing space inside the color function (so
/// `rgb(R G B / var(--tw-text-opacity, 1))` becomes `rgb(R G B )`)
/// matches the oracle's output.
fn apply_alpha_var_removal(decls: &mut Vec<Declaration>, remove_vars: &[&'static str]) {
    if remove_vars.is_empty() {
        return;
    }
    decls.retain(|d| !remove_vars.iter().any(|v| d.property == *v));
    for decl in decls.iter_mut() {
        for var in remove_vars {
            let with_one = format!("/ var({var}, 1)");
            let bare = format!("/ var({var})");
            if decl.value.contains(&with_one) {
                decl.value = decl.value.replace(&with_one, "");
            }
            if decl.value.contains(&bare) {
                decl.value = decl.value.replace(&bare, "");
            }
        }
    }
}

fn rules_for_value_decls(
    resolved: value_utilities::ResolvedDecls,
    parsed: &ParsedCandidate,
    applied: AppliedVariants,
    config: Option<&serde_json::Value>,
    cx: &VariantContext,
) -> Vec<Rule> {
    // Prepended decls (variant-driven `content: var(--tw-content)`)
    // are structural. Skip a prepended decl whose property is
    // already in the resolver's output — `before:content-['hi']`
    // resolves to a `content: var(--tw-content)` decl which would
    // duplicate the variant's prepend. Mirrors upstream's behavior
    // (PostCSS-level dedup that we approximate at emit time).
    let resolver_props: rustc_hash::FxHashSet<&str> =
        resolved.decls.iter().map(|(p, _)| p.as_str()).collect();
    // Cascade-var defaults are tagged as `@defaults <group>;`
    // markers when `experimental.optimizeUniversalDefaults` is on
    // AND `@tailwind base` is in the input. A post-pass collects
    // markers across the whole sheet, groups by id, and emits one
    // shared defaults rule per (group, isolation-bucket, at-rule
    // context) tuple at the start of the sheet — mirroring
    // upstream's `resolveDefaultsAtRules.js`. Without the flag,
    // the universal `*, ::before, ::after` block emits these
    // defaults globally instead.
    let defaults_groups: Vec<String> =
        if optimize_universal_defaults_enabled(config) && cx.has_tailwind_base {
            resolved
                .defaults_groups
                .iter()
                .map(|g| (*g).to_string())
                .collect()
        } else {
            Vec::new()
        };
    let mut declarations: Vec<Declaration> = applied
        .prepend_decls
        .iter()
        .filter(|(p, _)| !resolver_props.contains(*p))
        .map(|(p, v)| Declaration {
            property: (*p).to_string(),
            value: (*v).to_string(),
            important: false,
        })
        .collect();
    for (prop, value) in resolved.decls {
        // Apply any function-form `addVariant` `walkDecls` value
        // transforms captured during variant resolution. e.g. a
        // plugin with `decl.value = `calc(0 + ${decl.value})`` and
        // a `font-bold` candidate emits `font-weight: calc(0 + 700)`
        // when the variant fires.
        let transformed = if applied.decl_value_templates.is_empty() {
            value
        } else {
            crate::variants::apply_decl_value_templates(&value, &applied.decl_value_templates)
        };
        declarations.push(Declaration {
            property: prop,
            value: transformed,
            important: parsed.important,
        });
    }
    apply_alpha_var_removal(&mut declarations, &applied.remove_alpha_vars);
    let mut out: Vec<Rule> = Vec::with_capacity(applied.selectors.len());
    for sel in applied.selectors {
        // Sibling-pair plugins (`space-x`, `divide-x`) tag their
        // result with a selector suffix — we append it AFTER variants
        // run so `hover:space-x-4` becomes
        // `.hover\:space-x-4:hover > :not([hidden]) ~ :not([hidden])`.
        let final_sel = match resolved.selector_suffix {
            Some(suffix) => format!("{sel}{suffix}"),
            None => sel,
        };
        out.push(Rule {
            selectors: smallvec::smallvec![final_sel],
            declarations: declarations.clone(),
            at_rules: applied.at_rules.clone(),
            respect_important: true,
            defaults_groups: defaults_groups.clone(),
        });
    }
    out
}

/// Return the textual form of the candidate that should appear inside
/// the selector (before escaping). The parser preserves the original
/// candidate in `parsed.raw`, and that text IS the class name in
/// Tailwind v3 — `escape_class_name` handles the inevitable `:` `[`
/// `/` `!` `=` etc.
///
/// Using `parsed.raw` directly (instead of reconstructing from the
/// parsed pieces) is important when a prefix + negative are both in
/// play. Tailwind accepts BOTH orderings — `-tw-mx-0` and `tw--mx-0`
/// — and preserves whichever the user wrote in their markup as the
/// class selector. Reconstruction would canonicalize to one form and
/// drop rules a user might have written in the other.
fn render_class_form(parsed: &ParsedCandidate) -> String {
    parsed.raw.to_string()
}

/// Pick the comma-escape style for a candidate's class selector.
///
/// Tailwind v3 has a quirk: when `prefix` is configured, the
/// `prefixSelector` helper runs postcss-selector-parser on the
/// already-escaped class and writes back to `.value`. That re-escape
/// produces literal `\,` for commas — `escapeCommas` is only called
/// inside `escapeClassName`, not inside `prefixSelector`. So bare-
/// prefixed candidates like `tw-bg-[rgba(0,0,0)]` end up with `\,` in
/// the final selector, while candidates with variants (which go
/// through `escapeClassName` end-to-end) keep `\2c `, and arbitrary
/// properties `[prop:val]` (no class name to prefix) also keep `\2c `.
///
/// We mirror that behavior here so projects with `prefix: 'tw-'` get
/// byte-identical output. Present in both v3.3 and v3.4 — not gated
/// on `tailwindVersion`.
fn comma_style(
    config: Option<&serde_json::Value>,
    class_form: &str,
    has_variants: bool,
) -> CommaEscapeStyle {
    let prefix = config
        .and_then(|c| c.get("prefix"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let is_arbitrary_property = class_form.starts_with('[');
    if !prefix.is_empty() && !has_variants && !is_arbitrary_property {
        CommaEscapeStyle::Literal
    } else {
        CommaEscapeStyle::Numeric
    }
}

fn render_modifier_into(modifier: &galeforce_parser::Modifier, out: &mut String) {
    match modifier {
        galeforce_parser::Modifier::Named(n) => out.push_str(n),
        galeforce_parser::Modifier::Arbitrary(a) => {
            out.push('[');
            out.push_str(a);
            out.push(']');
        }
    }
}

/// Per-candidate sort metadata, computed once before iterating the
/// candidate's emitted rules (a candidate can yield multiple rules
/// when variants produce parallel selectors).
///
/// After the v3 `Offsets` port (Phase 3-6), only `plugin_order` and
/// `within_plugin_order` feed into the sort axis (`RuleOffset.index`).
/// The legacy `variant_primary` / `variant_secondary` / `prefix_hash`
/// / `numeric` fields were retired in Phase 7 — the bigint-bitmask
/// `variants` field on `RuleOffset` (paired with `recalculateVariant
/// Offsets` for arbitrary-variant alphabetic remapping) drives all
/// cross-variant ordering now.
#[derive(Clone)]
struct CandidateSortMeta {
    /// Plugin registration order index (0-based) per upstream's
    /// `corePlugins.js`. Used to bias sort so e.g. `resize` (~65)
    /// emits before `boxShadow` (~150). Unknown plugins use
    /// `u32::MAX` so they sort to the back.
    plugin_order: u32,
    /// Index into `STATIC_UTILITIES` for static-utility candidates,
    /// otherwise `u32::MAX`. Carries within-plugin emission order
    /// for cases where multiple statics share a `plugin_order`
    /// (`display` registers 21+ utilities in a fixed order that
    /// determines cascade — most notably `hidden` last).
    within_plugin_order: u32,
}

fn candidate_sort_meta(parsed: &ParsedCandidate) -> CandidateSortMeta {
    let mut plugin_order = lookup_plugin_order_for_root(parsed.root, parsed);
    // Arbitrary variants (`[&_.foo]:tw-text-red-400`,
    // `[.foo_&]:tw-bg-white`) get distinct variant-bit offsets in
    // upstream's `Offsets` system. Tailwind v3's
    // `recalculateVariantOffsets` re-assigns those bits in
    // alphabetic order of the variant text, so the EMISSION order
    // is driven by variant text alone — `plugin_order` doesn't
    // tie-break across two arbitrary variants. We mirror that by
    // zeroing `plugin_order` whenever the candidate has any
    // arbitrary variant in its chain, so the sort falls through to
    // `input_index` (which carries the alphabetic-pre-sort rank of
    // the full candidate name, including the variant text).
    let has_arbitrary_variant = parsed
        .variants
        .iter()
        .any(|v| matches!(v, galeforce_parser::ParsedVariant::Arbitrary(_)));
    if has_arbitrary_variant {
        plugin_order = 0;
    }
    // Within-plugin emission order: first try the static-utility
    // table (covers `display: block, inline-block, …, hidden` and
    // every other `addUtilities` plugin). For VALUE utilities
    // (`inset`, `inset-x`, `top`, `bottom` — all under the `inset`
    // corePlugin), fall back to the value-utility table's index of
    // the matching class_prefix. This is what makes `inset` emit
    // before `inset-x` emit before `top`, mirroring upstream's
    // `createUtilityPlugin` sub-entry order.
    let within_plugin_order = if has_arbitrary_variant {
        0
    } else if let Some(idx) = static_utilities::static_table_index(parsed.root) {
        idx
    } else if let Some(sib_idx) = static_utilities::sibling_static_table_index(parsed.root) {
        // Sibling-statics (e.g. `space-y-reverse`, `divide-x-reverse`,
        // `divide-solid`) come from `addUtilities(...)` inside a
        // corePlugin AFTER its `matchUtilities(...)` call. Bias their
        // `within_plugin_order` past the value-utility range so they
        // emit after `.space-x-4` / `.divide-x-2` in the same plugin
        // family. The 0x10000 base is bigger than any
        // `value_table_index` (the value table has ~250 entries).
        0x1_0000 + sib_idx
    } else {
        // Walk to the matched value-utility prefix. Use the existing
        // `find_value_utilities` lookup so we don't duplicate the
        // longest-prefix-wins logic. Pick the first match — within
        // the same plugin family they all share an index (the
        // prefix's table position).
        value_utilities::find_value_utilities(parsed.root)
            .first()
            .and_then(|(util, _value_key)| value_utilities::value_table_index(util.class_prefix))
            .unwrap_or(u32::MAX)
    };
    CandidateSortMeta {
        plugin_order,
        within_plugin_order,
    }
}

/// Resolve a candidate's root to the registration index of the
/// plugin that owns it. Walks the static + value utility tables to
/// find a `plugin` tag, then looks up that name in
/// `PLUGIN_ORDER`. Returns `u32::MAX` for plugin-defined utilities
/// or unknown roots.
fn lookup_plugin_order_for_root(root: &str, _parsed: &ParsedCandidate) -> u32 {
    if let Some(util) = find_static(root) {
        return plugin_order_index(util.plugin);
    }
    if find_sibling_static(root).is_some() {
        // Sibling-pair statics (`space-x-reverse`, `divide-x-reverse`)
        // don't carry a plugin tag. Default to `space`'s index for
        // `space-*` and `divideWidth`'s for `divide-*` — close enough
        // to upstream's ordering for these few cases.
        if root.starts_with("space-") {
            return plugin_order_index("space");
        }
        if root.starts_with("divide-") {
            return plugin_order_index("divideWidth");
        }
        return u32::MAX;
    }
    if let Some((util, _)) = find_value_utilities(root).into_iter().next() {
        return plugin_order_index(util.plugin);
    }
    u32::MAX
}

/// Lookup the registration index for a plugin name. Mirrors the
/// definition order of `corePlugins` in
/// `vendor/tailwindcss-v3/src/corePlugins.js`. Used as the
/// dominant axis in `SortKey` so emitted utilities follow upstream's
/// emission order. Plugins not in the table sort last.
fn plugin_order_index(name: &str) -> u32 {
    PLUGIN_ORDER
        .iter()
        .position(|n| *n == name)
        .map_or(u32::MAX, |i| i as u32)
}

/// `plugin_order` sentinel for user-plugin output (`addUtilities` /
/// `addComponents` / `matchUtilities`). Sits past the last core plugin
/// slot so user candidates emit AFTER every core utility — mirroring
/// upstream Tailwind v3 where user plugins register after core plugins
/// in `setupContextUtils.js`.
///
/// Without this, user-plugin candidates fall through to the value-utility
/// prefix lookup and inherit the SHARED-prefix core plugin's order
/// (e.g. `font-section-title` from a user plugin would be tagged with
/// `fontWeight` since `font-` is a `fontWeight`/`fontFamily` prefix).
/// That places user output INSIDE the core cluster and breaks last-wins
/// cascade for `font` shorthand vs `font-weight` overrides.
const USER_PLUGIN_ORDER: u32 = PLUGIN_ORDER.len() as u32;

const PLUGIN_ORDER: &[&str] = &[
    "container",
    "accessibility",
    "pointerEvents",
    "visibility",
    "position",
    "inset",
    "isolation",
    "zIndex",
    "order",
    "gridColumn",
    "gridColumnStart",
    "gridColumnEnd",
    "gridRow",
    "gridRowStart",
    "gridRowEnd",
    "float",
    "clear",
    "margin",
    "boxSizing",
    "lineClamp",
    "display",
    "aspectRatio",
    "size",
    "height",
    "maxHeight",
    "minHeight",
    "width",
    "minWidth",
    "maxWidth",
    "flex",
    "flexShrink",
    "flexGrow",
    "flexBasis",
    "tableLayout",
    "captionSide",
    "borderCollapse",
    "borderSpacing",
    "transformOrigin",
    "translate",
    "rotate",
    "skew",
    "scale",
    "transform",
    "animation",
    "cursor",
    "touchAction",
    "userSelect",
    "resize",
    "scrollSnapType",
    "scrollSnapAlign",
    "scrollSnapStop",
    "scrollMargin",
    "scrollPadding",
    "listStylePosition",
    "listStyleType",
    "listStyleImage",
    "appearance",
    "columns",
    "breakBefore",
    "breakInside",
    "breakAfter",
    "gridAutoColumns",
    "gridAutoFlow",
    "gridAutoRows",
    "gridTemplateColumns",
    "gridTemplateRows",
    "flexDirection",
    "flexWrap",
    "placeContent",
    "placeItems",
    "alignContent",
    "alignItems",
    "justifyContent",
    "justifyItems",
    "gap",
    "space",
    "divideWidth",
    "divideStyle",
    "divideColor",
    "divideOpacity",
    "placeSelf",
    "alignSelf",
    "justifySelf",
    "overflow",
    "overscrollBehavior",
    "scrollBehavior",
    "textOverflow",
    "hyphens",
    "whitespace",
    "textWrap",
    "wordBreak",
    "borderRadius",
    "borderWidth",
    "borderStyle",
    "borderColor",
    "borderOpacity",
    "backgroundColor",
    "backgroundOpacity",
    "backgroundImage",
    "gradientColorStops",
    "boxDecorationBreak",
    "backgroundSize",
    "backgroundAttachment",
    "backgroundClip",
    "backgroundPosition",
    "backgroundRepeat",
    "backgroundOrigin",
    "fill",
    "stroke",
    "strokeWidth",
    "objectFit",
    "objectPosition",
    "padding",
    "textAlign",
    "textIndent",
    "verticalAlign",
    "fontFamily",
    "fontSize",
    "fontWeight",
    "textTransform",
    "fontStyle",
    "fontVariantNumeric",
    "lineHeight",
    "letterSpacing",
    "textColor",
    "textOpacity",
    "textDecoration",
    "textDecorationColor",
    "textDecorationStyle",
    "textDecorationThickness",
    "textUnderlineOffset",
    "fontSmoothing",
    "placeholderColor",
    "placeholderOpacity",
    "caretColor",
    "accentColor",
    "opacity",
    "backgroundBlendMode",
    "mixBlendMode",
    "boxShadow",
    "boxShadowColor",
    "outlineStyle",
    "outlineWidth",
    "outlineOffset",
    "outlineColor",
    "ringWidth",
    "ringColor",
    "ringOpacity",
    "ringOffsetWidth",
    "ringOffsetColor",
    "blur",
    "brightness",
    "contrast",
    "dropShadow",
    "grayscale",
    "hueRotate",
    "invert",
    "saturate",
    "sepia",
    "filter",
    "backdropBlur",
    "backdropBrightness",
    "backdropContrast",
    "backdropGrayscale",
    "backdropHueRotate",
    "backdropInvert",
    "backdropOpacity",
    "backdropSaturate",
    "backdropSepia",
    "backdropFilter",
    "transitionProperty",
    "transitionDelay",
    "transitionDuration",
    "transitionTimingFunction",
    "willChange",
    "contain",
    "content",
    "forcedColorAdjust",
];

/// Build a `RuleOffset` for a single emitted rule. Mirrors
/// upstream's `applyVariantOffset` chain applied to a fresh
/// utility-layer offset, packed with our (plugin_order,
/// within_plugin_order, input_index) tiebreaker into `index`.
///
/// All arbitrary variants must already be registered in `offsets`
/// (do that pre-pass in `compile()` before any parallel work, so
/// `offsets` is effectively read-only here).
fn build_rule_offset(
    parsed: &ParsedCandidate,
    rule: &Rule,
    meta: &CandidateSortMeta,
    offsets: &offsets::Offsets,
    input_index: u32,
) -> offsets::RuleOffset {
    use offsets::{Layer, RuleOffset, VariantOption};
    let mut variants_bits: offsets::VarBits = offsets::VAR_BITS_ZERO;
    let options: smallvec::SmallVec<[VariantOption; 4]> = smallvec::SmallVec::new();
    // Walk variants left-to-right (mirrors upstream's
    // `generateRules.js:405-416` `applyVariantOffset` chain).
    for v in &parsed.variants {
        match v {
            galeforce_parser::ParsedVariant::Named(n) => {
                variants_bits = offsets::var_or(variants_bits, resolve_variant_bits(n, offsets));
            }
            galeforce_parser::ParsedVariant::Arbitrary(t) => {
                let key = format!("[{}]", t);
                if let Some(bit) = offsets.variant_bit(&key) {
                    variants_bits = offsets::var_or(variants_bits, bit);
                }
            }
        }
    }
    // Layer assignment: if the rule was wrapped in an at-rule OR has
    // any variants applied, it lives in `Layer::Variants`. parent_layer
    // captures the rule's ORIGINAL layer (currently always Utilities
    // for user candidates; will need extension for components later).
    let has_variants = !parsed.variants.is_empty() || !rule.at_rules.is_empty();
    let (layer, parent_layer) = if has_variants {
        (Layer::Variants, Layer::Utilities)
    } else {
        (Layer::Utilities, Layer::Utilities)
    };
    // Pack (plugin_order, within_plugin_order, min_width, input_index)
    // into the u64 `index` field.
    //
    // Layout (high-to-low bit):
    //   plugin_order:        bits 56-63  (8 bits  — 256 plugins; ~180 in PLUGIN_ORDER)
    //   within_plugin_order: bits 36-55  (20 bits — sibling-static base `0x1_0000+sib_idx` fits)
    //   min_width:           bits 24-35  (12 bits — px breakpoint, max ~4095)
    //   input_index:         bits  0-23  (24 bits — 16M candidates; was 12 bits → overflowed)
    //
    // The previous layout gave `input_index` only 12 bits (4096
    // distinct values). Mid-size projects routinely scan 5–25k
    // candidates; once the corpus exceeds 4096, distinct candidates
    // collide on `input_index & 0xFFF` and the final stable sort
    // falls back on whatever order `par_iter().collect()` emits.
    // That scrambles the relative order of same-plugin/same-wpo
    // candidates beyond the first 4096 — e.g. `data-[X]:outline`
    // variants at input_indexes 12224 / 12241 / 12293 collide with
    // EARLIER candidates at the same low-12-bit mask. Widening to
    // 24 bits covers realistic project sizes; `plugin_order`
    // shrinks 16→8 bits (more than enough for the ~180-entry
    // `PLUGIN_ORDER` table) and `within_plugin_order` shrinks 24→20
    // bits (sibling-static base `0x1_0000+sib_idx` peaks ~65 545 ≈
    // 17 bits, leaving headroom for theme-key index growth).
    let plugin_order = meta.plugin_order.min(0xFF) as u64;
    let wpo = meta.within_plugin_order.min(0xF_FFFF) as u64;
    let min_width = rule
        .at_rules
        .iter()
        .find_map(|a| extract_min_width_px(a))
        .unwrap_or(0)
        .min(0xFFF) as u64;
    let index =
        (plugin_order << 56) | (wpo << 36) | (min_width << 24) | ((input_index as u64) & 0xFF_FFFF);
    // Arbitrary property detection: parsed.root starts with `[` and
    // ends with `]` and contains a `:` (CSS-decl shape).
    //
    // Tailwind v3 uses the FULL candidate (`parsed.root`, e.g.
    // `[--value:1]`) as the `property` key in `sortArbitraryProperties`
    // (see `vendor/tailwindcss-v3/src/lib/generateRules.js:522` —
    // `context.offsets.arbitraryProperty(classCandidate)`). Storing
    // just the property NAME (`--value`) makes
    // `[--value:1]` and `[--value:2]` collide on `property_offset`,
    // forcing the tiebreaker down to `index` (candidate-iteration
    // order) and inverting Tailwind's order for cases like
    // `hover:file:[--value:1]` vs `file:hover:[--value:2]` where the
    // INNER VALUE distinguishes them.
    let (arbitrary, property) = if parsed.root.starts_with('[') && parsed.root.ends_with(']') {
        let inner = &parsed.root[1..parsed.root.len() - 1];
        if inner.contains(':') {
            (1u8, parsed.root.to_string())
        } else {
            (0u8, String::new())
        }
    } else {
        (0u8, String::new())
    };
    RuleOffset {
        layer,
        parent_layer,
        arbitrary,
        variants: variants_bits,
        parallel_index: 0,
        index,
        property_offset: 0,
        property,
        options,
    }
}

/// Emit a stylesheet and interleave `@keyframes` blocks at the
/// position of their first consuming `.animate-X` rule. Mirrors
/// upstream's animation plugin emission shape
/// (`vendor/tailwindcss-v3/src/corePlugins.js:1022-1052`) where
/// each `animate-spin` candidate yields a parallel rule pair —
/// `@keyframes` at `parallelIndex=0`, `.animate-X` at the next
/// parallel slot. After sort by `RuleOffset.parallel_index`, the
/// keyframes block sits immediately before its consumer. We
/// achieve the same shape post-sort by scanning emitted rules for
/// the first match per keyframe name and splicing the
/// `@keyframes` block in at that slot.
fn emit_stylesheet_with_keyframes(sheet: &Stylesheet, keyframes: &[KeyframeBlock]) -> String {
    if keyframes.is_empty() {
        return emit_stylesheet(sheet, &EmitOptions::default());
    }
    // Index keyframes by name (FIRST occurrence wins for the body
    // since the same `@keyframes <name>` block is identical
    // regardless of which candidate produced it).
    let mut by_name: rustc_hash::FxHashMap<&str, &KeyframeBlock> = rustc_hash::FxHashMap::default();
    for kb in keyframes {
        by_name.entry(kb.name.as_str()).or_insert(kb);
    }
    // For each rule, scan its `animation:` declaration to find which
    // keyframe names it references — `.animate-multiple` references
    // `bounce` and `pulse`, not `multiple`. Per-consumer emission:
    // every consumer rule gets a fresh `@keyframes` block before it,
    // even when multiple consumers reference the same keyframe.
    // Mirrors the live oracle, which postcss-flattens to one
    // `@keyframes` per consumer (verified against `should properly
    // handle keyframes with multiple variants` where 5 consumers
    // produce 5 `@keyframes` blocks for two unique names).
    let mut out = String::new();
    for rule in &sheet.rules {
        let mut to_emit_here: Vec<&KeyframeBlock> = Vec::new();
        let mut seen_in_rule: rustc_hash::FxHashSet<&str> = rustc_hash::FxHashSet::default();
        for decl in &rule.declarations {
            if decl.property != "animation" {
                continue;
            }
            for animation_part in split_top_level(&decl.value, ',') {
                for token in split_top_level(animation_part, ' ') {
                    let tok = token.trim();
                    if tok.is_empty() {
                        continue;
                    }
                    if let Some(kb) = by_name.get(tok) {
                        // De-dupe WITHIN a single rule so the same
                        // keyframe isn't repeated when one consumer
                        // references it twice (e.g. theme misconfig).
                        // ACROSS rules we never dedupe — per-consumer
                        // emission is the upstream shape.
                        if seen_in_rule.insert(tok) {
                            to_emit_here.push(*kb);
                        }
                    }
                }
            }
        }
        for kb in &to_emit_here {
            let mut wrapped = format!("@keyframes {} {{ {} }}", kb.name, kb.body);
            // Wrap the keyframes in the SAME at-rule context as the
            // consuming rule — `lg:animate-spin` must have its
            // `@keyframes spin` inside `@media (min-width: 1024px)`.
            for at in rule.at_rules.iter().rev() {
                wrapped = format!("{at} {{ {wrapped} }}");
            }
            out.push_str(&wrapped);
        }
        let one_sheet = Stylesheet {
            rules: vec![rule.clone()],
        };
        out.push_str(&emit_stylesheet(&one_sheet, &EmitOptions::default()));
    }
    out
}

/// Split a string on `delim` characters that are NOT nested inside
/// parentheses, brackets, or braces. Mirrors upstream's
/// `parseAnimationValue` COMMA/SPACE regexes which use
/// `,(?![^(]*\))` / `\ +(?![^(]*\))` — comma/space outside parens.
fn split_top_level(input: &str, delim: char) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut depth: i32 = 0;
    let mut start = 0usize;
    for (i, ch) in input.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            c if c == delim && depth == 0 => {
                out.push(&input[start..i]);
                start = i + ch.len_utf8();
            }
            _ => {}
        }
        // Avoid panic on malformed input.
        if depth < 0 {
            depth = 0;
        }
        let _ = bytes; // suppress unused warning when bytes isn't used below.
    }
    out.push(&input[start..]);
    out
}

/// Strip a matchVariant family prefix (`group-`, `peer-`, `aria-`,
/// `data-`, `has-`, `supports-`, `min-`, `max-`) and return the
/// family name itself. Used so a candidate like `aria-checked` or
/// `aria-[busy=true]` resolves to the `aria` family bit rather than
/// the literal full variant string.
fn strip_family_prefix(name: &str) -> Option<String> {
    for fam in &[
        "group-",
        "peer-",
        "aria-",
        "data-",
        "has-",
        "supports-",
        "min-",
        "max-",
    ] {
        if name.starts_with(fam) {
            return Some(fam.trim_end_matches('-').to_string());
        }
    }
    None
}

/// Resolve a variant name to its bitmask. Mirrors upstream's
/// `applyVariant` lookup chain:
///
///   1. Direct `variantMap.has(variant)` lookup.
///   2. Strip a trailing `/modifier` and retry (modifiers don't have
///      their own bit; they're cosmetic on the selector).
///   3. For `<base>-[<X>]` shapes, lookup `<base>` and OR in the
///      arbitrary inner `[X]` bit (mirrors `generateRules.js:202-222`
///      where upstream extracts the arbitrary value, leaving the
///      named base in `variant`).
///
/// Returns 0 when no bits resolve so the caller can `|=` unconditionally.
/// Find the byte position of `.<class>` within a CSS selector string,
/// then return the count of CLASSES appearing before it. Used as a
/// secondary `within_plugin_order` axis for multi-class user-CSS
/// `@layer utilities` rules (`.foo.bar.baz`, `:is(.foo, .bar, .baz)`,
/// `html:has(.foo)`) — candidates that matched the class appearing
/// EARLIER in the selector sort first. Mirrors upstream's
/// `withIdentifiers(rule)` iteration order in
/// `setupContextUtils.js:345` where each registered identifier gets a
/// monotonically increasing `offsets.create('utilities')` index.
fn position_of_class_in_selector(selector: &str, class: &str) -> Option<u8> {
    let needle = format!(".{class}");
    let mut count: u8 = 0;
    let bytes = selector.as_bytes();
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        // Scan for `.` then check whether the candidate matches at
        // this position. Word-boundary check: the byte after the
        // class name must NOT be a class-name character (letter,
        // digit, `-`, `\\`).
        if bytes[i] == b'.' && selector[i..].starts_with(&needle) {
            let end = i + needle.len();
            let boundary_ok = end == bytes.len()
                || matches!(
                    bytes[end],
                    b'.' | b' '
                        | b','
                        | b'>'
                        | b'~'
                        | b'+'
                        | b'#'
                        | b'['
                        | b':'
                        | b')'
                        | b'\t'
                        | b'\n'
                );
            if boundary_ok {
                return Some(count);
            }
        }
        if bytes[i] == b'.' {
            // Skip past this class to count it.
            let mut j = i + 1;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'-' || bytes[j] == b'_')
            {
                j += 1;
            }
            if j > i + 1 {
                count = count.saturating_add(1);
                i = j;
                continue;
            }
        }
        i += 1;
    }
    None
}

/// Find the position of `candidate_root` within the plugin rule's
/// `referenced_classes()` list. Used to assign a stable
/// `within_plugin_order` for multi-class plugin rules (`.parent .child`
/// is registered under BOTH `parent` and `child` — upstream's
/// `addComponents` calls `offsets.create('components')` per identifier
/// in `withIdentifiers(rule)`, so candidates matching the earlier
/// identifier sort first). Falls back to 0 when the candidate doesn't
/// appear in `referenced_classes` (the lookup hit a primary match
/// without the alias path).
fn matched_class_position(rule: &plugins::PluginRule, candidate_root: &str) -> u32 {
    for (i, c) in rule.referenced_classes().iter().enumerate() {
        let unescaped = strip_css_escapes(c);
        if c == candidate_root || unescaped.as_str() == candidate_root {
            return i as u32;
        }
    }
    0
}

/// Extract the `min-width` from an at-rule string like
/// `@media (min-width: 768px)`. Returns pixel value as u32 (rem
/// converted at 16px = 1rem). Returns `None` for at-rules that don't
/// contain `min-width:`. Used by `build_rule_offset` to break ties
/// between same-candidate multi-rule plugins like `container`.
fn extract_min_width_px(at_rule: &str) -> Option<u32> {
    let start = at_rule.find("min-width:")? + "min-width:".len();
    let rest = at_rule[start..].trim_start();
    let mut digits = String::new();
    for c in rest.chars() {
        if c.is_ascii_digit() || c == '.' {
            digits.push(c);
        } else {
            break;
        }
    }
    if digits.is_empty() {
        return None;
    }
    let n: f32 = digits.parse().ok()?;
    let after = &rest[digits.len()..];
    let unit = after.trim_start();
    Some(if unit.starts_with("rem") {
        (n * 16.0) as u32
    } else {
        n as u32
    })
}

fn resolve_variant_bits(name: &str, offsets: &offsets::Offsets) -> offsets::VarBits {
    if let Some(bit) = offsets.variant_bit(name) {
        return bit;
    }
    let bare = name.split('/').next().unwrap_or(name);
    if bare != name {
        if let Some(bit) = offsets.variant_bit(bare) {
            return bit;
        }
    }
    // `<base>-[<X>]` shape: drop the trailing `[<X>]` and look up
    // `<base>`. Mirrors upstream's `generateRules.js:202-222` which
    // extracts the arbitrary value into `args.value` and uses the
    // base form's offset (the value influences sort only via the
    // optional `options.sort` callback — aria/data/supports/has have
    // none, so all `data-[X]` candidates share the `data` bit and
    // tie-break on `index`).
    if bare.ends_with(']') {
        if let Some(bracket_open) = bare.rfind('[') {
            if bracket_open > 0 && bare.as_bytes()[bracket_open - 1] == b'-' {
                let base = &bare[..bracket_open - 1];
                return offsets.variant_bit(base).unwrap_or(offsets::VAR_BITS_ZERO);
            }
        }
    }
    // Fallback: family-prefix split for unregistered composites.
    let Some(family) = strip_family_prefix(bare) else {
        return offsets::VAR_BITS_ZERO;
    };
    let family_bit = offsets
        .variant_bit(&family)
        .unwrap_or(offsets::VAR_BITS_ZERO);
    let inner = bare.strip_prefix(&format!("{family}-")).unwrap_or("");
    let inner_bits = if inner.is_empty() {
        offsets::VAR_BITS_ZERO
    } else if inner.starts_with('[') && inner.ends_with(']') {
        offsets.variant_bit(inner).unwrap_or(offsets::VAR_BITS_ZERO)
    } else {
        resolve_variant_bits(inner, offsets)
    };
    offsets::var_or(family_bit, inner_bits)
}

/// Look up plugin-defined utilities matching `class_name`. Returns
/// `None` when the candidate isn't plugin-defined OR when the config
/// has no `__pluginOutput`. The boolean second tuple element is
/// `true` iff at least one match originated from `addComponents` —
/// rules need to be routed to the `@tailwind components;` slot in
/// that case. Components and utilities are not allowed to share a
/// class name in upstream's plugin runtime, so the flag is
/// unambiguous in practice.
///
/// Each call walks the plugin output afresh — for projects with
/// plugins we re-deserialize per candidate, which is wasteful but
/// keeps `compile_candidate`'s signature unchanged for now. Optimize
/// if profiling shows it as a hot spot once plugin-heavy projects
/// start landing.
fn plugin_index_get(
    config: Option<&serde_json::Value>,
    class_name: &str,
) -> Option<(Vec<plugins::PluginRule>, bool)> {
    let output = plugins::read_plugin_output(config);
    // Plugin output may have selectors prefixed with the user's
    // `prefix` config (since `addUtilities` / `addComponents`
    // respect `prefix` by default). The candidate parser strips
    // the prefix into `parsed.prefix`, leaving `parsed.root`
    // un-prefixed. Match plugin rules by EITHER form so prefixed
    // plugin output still resolves.
    let prefix: Option<&str> = config
        .and_then(|c| c.get("prefix"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let prefixed_form: Option<String> = prefix.map(|p| format!("{p}{class_name}"));
    let matches_class = |r: &plugins::PluginRule| -> bool {
        // First try the rule's primary class (selector-derived).
        if let Some(c) = r.primary_class() {
            if c == class_name {
                return true;
            }
            if prefixed_form.as_deref().map(|p| c == p).unwrap_or(false) {
                return true;
            }
            // The plugin may have used the escape helper
            // (`e('custom-top-1/4') -> 'custom-top-1\/4'`),
            // producing a primary class with backslash escapes
            // that don't appear in the unescaped candidate
            // root. Compare again after stripping CSS escape
            // backslashes so the candidate `custom-top-1/4`
            // matches the plugin's `custom-top-1\/4`.
            if c.contains('\\') {
                let unescaped = strip_css_escapes(c);
                if unescaped == class_name {
                    return true;
                }
                if let Some(p) = prefixed_form.as_deref() {
                    if unescaped == p {
                        return true;
                    }
                }
            }
        }
        // Fall back to the materialized-match group tag set by
        // the JS runner. This covers companion rules (e.g.
        // `@keyframes <name>`) that share a `matchUtilities`
        // invocation with the class rule but have no class
        // selector themselves.
        //
        // When `group_class` is set, the rule belongs to a
        // `matchUtilities`/`matchComponents` materialization —
        // upstream's `setupContextUtils.js` registers those rules
        // ONLY under the plugin's `classPrefix` (the group), not
        // under any nested-selector class. So we skip the
        // referenced-classes alias path entirely: a candidate
        // matches iff it equals the group tag.
        if let Some(g) = r.group_class.as_deref() {
            if g == class_name {
                return true;
            }
            if prefixed_form.as_deref().map(|p| g == p).unwrap_or(false) {
                return true;
            }
            return false;
        }
        // Alias-match: any class referenced in the selector
        // (including ones inside `:is()`/`:where()`/`:has()`,
        // excluding `:not(...)` exclusion clauses) triggers the
        // rule. Mirrors upstream's `setupContextUtils.js`
        // candidate-extraction step where every class in a plugin
        // selector becomes a candidate-map key. The accompanying
        // `rules_for_plugin` substitution drops selectors that
        // don't contain the matched class so multi-selector
        // forms don't end up with leftover non-substituted entries.
        for c in r.referenced_classes() {
            if c == class_name {
                return true;
            }
            if prefixed_form.as_deref().map(|p| c == p).unwrap_or(false) {
                return true;
            }
            if c.contains('\\') {
                let unescaped = strip_css_escapes(&c);
                if unescaped == class_name {
                    return true;
                }
                if let Some(p) = prefixed_form.as_deref() {
                    if unescaped == p {
                        return true;
                    }
                }
            }
        }
        false
    };
    let utility_matches: Vec<plugins::PluginRule> = output
        .utilities
        .iter()
        .filter(|r| matches_class(r))
        .cloned()
        .collect();
    let component_matches: Vec<plugins::PluginRule> = output
        .components
        .iter()
        .filter(|r| matches_class(r))
        .cloned()
        .collect();
    if utility_matches.is_empty() && component_matches.is_empty() {
        return None;
    }
    let is_component = !component_matches.is_empty();
    let mut combined = utility_matches;
    combined.extend(component_matches);
    Some((combined, is_component))
}

/// Build rules for a plugin-defined utility match. Each captured
/// `PluginRule` becomes a `Rule`; selectors are rebuilt by
/// substituting the original `.<class-name>` portion with the
/// candidate's escaped `class_selector` (so variants can compose
/// against the candidate's actual emitted name, not the raw class).
fn rules_for_plugin(
    plugin_rules: &[plugins::PluginRule],
    parsed: &ParsedCandidate,
    applied: AppliedVariants,
    class_selector: &str,
) -> Vec<Rule> {
    let mut out: Vec<Rule> = Vec::new();
    // Build the list of candidate forms to look for in plugin rule
    // selectors. The candidate parser stripped any user-config
    // prefix into `parsed.prefix` and modifier into `parsed.modifier`;
    // plugin selectors however carry the prefix+modifier already
    // applied. We try each combination so the substitution target
    // anchors against the actual literal in the selector.
    let mod_form = render_class_form_no_modifier(parsed);
    let prefix_str = parsed.prefix.unwrap_or("");
    let prefixed_root = if !prefix_str.is_empty() {
        Some(format!("{prefix_str}{}", parsed.root))
    } else {
        None
    };
    let prefixed_mod = if !prefix_str.is_empty() && parsed.modifier.is_some() {
        Some(format!("{prefix_str}{mod_form}"))
    } else {
        None
    };
    let mut candidate_forms: Vec<&str> = Vec::with_capacity(4);
    candidate_forms.push(parsed.root);
    if mod_form != parsed.root {
        candidate_forms.push(&mod_form);
    }
    if let Some(p) = prefixed_root.as_deref() {
        candidate_forms.push(p);
    }
    if let Some(p) = prefixed_mod.as_deref() {
        candidate_forms.push(p);
    }
    for rule in plugin_rules {
        let primary_default = rule.primary_class().unwrap_or(parsed.root);
        // Find which class in the rule's selector matched the
        // candidate, and whether that's the rule's primary class
        // or a referenced alias class.
        let (target, is_alias_match) = match alias_substitution_target(rule, &candidate_forms) {
            Some((s, alias)) => (s, alias),
            None => (primary_default, false),
        };
        let primary = target;
        // Per-selector filter+substitute (upstream's
        // `replaceSelector` walk) only kicks in when the candidate
        // carries a variant chain OR the `!important` modifier — the
        // candidate's escaped form THEN differs from the literal
        // class in the rule, so the multi-selector entries that
        // can't accept the substitution drop. Plain candidates
        // (matching primary OR alias) emit the rule selectors
        // verbatim with no filter; the bare class IS the literal
        // already in the rule.
        let needs_filter = parsed.important || !parsed.variants.is_empty();
        let is_alias_match = is_alias_match && needs_filter;
        let declarations: Vec<Declaration> = rule
            .declarations
            .iter()
            .map(|d| Declaration {
                property: d.property.clone(),
                value: d.value.clone(),
                important: d.important || parsed.important,
            })
            .collect();
        for variant_sel in &applied.selectors {
            let plugin_sel = if is_alias_match {
                substitute_class_in_selector_list(&rule.selector, primary, variant_sel)
                    .unwrap_or_else(|| rule.selector.clone())
            } else {
                substitute_primary_class_in_selector_list(&rule.selector, primary, variant_sel)
            };
            // Compose at-rules: candidate's responsive/dark variant
            // wrappers come outermost; the plugin's at-rule context
            // is innermost.
            let mut at_rules = applied.at_rules.clone();
            at_rules.extend(rule.at_rules.iter().cloned());
            // Variant-prepended decls (content for ::before etc.)
            // need to land first.
            let mut all_decls: Vec<Declaration> = applied
                .prepend_decls
                .iter()
                .map(|(p, v)| Declaration {
                    property: (*p).to_string(),
                    value: (*v).to_string(),
                    important: false,
                })
                .collect();
            all_decls.extend(declarations.clone());
            out.push(Rule {
                selectors: smallvec::smallvec![plugin_sel],
                declarations: all_decls,
                at_rules,
                respect_important: rule.respect_important,
                defaults_groups: Vec::new(),
            });
        }
    }
    let _ = class_selector;
    out
}

/// Pick the substitution target inside `rule.selector` based on
/// what the candidate ACTUALLY matched — `candidate_root`,
/// `candidate_with_modifier`, or any of their prefixed forms.
/// Returns `(slice, is_alias)` where `is_alias` is true when the
/// matched class is NOT the rule's primary class. `None` means we
/// couldn't confidently locate a substitution target — caller
/// should fall back to the primary-class assumption.
fn alias_substitution_target<'a>(
    rule: &'a plugins::PluginRule,
    candidate_forms: &[&str],
) -> Option<(&'a str, bool)> {
    let primary = rule.primary_class();
    let mut best: Option<&'a str> = None;
    let mut best_is_primary = false;
    for c in rule.referenced_classes() {
        let c_unescaped = strip_css_escapes(&c);
        let mut hit = false;
        for f in candidate_forms {
            if &c == f || c_unescaped.as_str() == *f {
                hit = true;
                break;
            }
        }
        if !hit {
            continue;
        }
        if let Some(idx) = find_class_in_selector(&rule.selector, &c) {
            let slice = &rule.selector[idx..idx + c.len()];
            let is_primary = primary == Some(c.as_str());
            if is_primary {
                best = Some(slice);
                best_is_primary = true;
            } else if best.is_none() {
                best = Some(slice);
                best_is_primary = false;
            }
        }
    }
    best.map(|s| (s, !best_is_primary))
}

/// Substitute `.<primary>` everywhere it appears in the selector
/// list with `replacement`, keeping every comma-separated entry.
/// When the rule's selector has nothing class-leading (`html`,
/// `body > *`), emit verbatim. Used when the candidate matches
/// the rule's PRIMARY class — upstream leaves multi-selector lists
/// intact in that case.
fn substitute_primary_class_in_selector_list(
    selector: &str,
    primary: &str,
    replacement: &str,
) -> String {
    let original = format!(".{primary}");
    if selector == original {
        return replacement.to_string();
    }
    if let Some(stripped) = selector.strip_prefix(&original) {
        let mut s = String::with_capacity(replacement.len() + stripped.len());
        s.push_str(replacement);
        s.push_str(stripped);
        return s;
    }
    if let Some(replaced) = replace_class_in_wrapped_selector(selector, primary, replacement) {
        return replaced;
    }
    // Multi-selector form — substitute in EACH comma-separated entry
    // that contains the class token, leaving non-matching entries
    // alone (they still emit because they belong to the same rule).
    let parts = split_top_level_selectors(selector);
    if parts.len() > 1 {
        let mut out_parts: Vec<String> = Vec::with_capacity(parts.len());
        let mut any_hit = false;
        for part in &parts {
            let (replaced, hit) = substitute_class_once(part, primary, replacement);
            if hit {
                any_hit = true;
                out_parts.push(replaced);
            } else {
                out_parts.push(part.clone());
            }
        }
        if any_hit {
            return out_parts.join(", ");
        }
    }
    selector.to_string()
}

/// Walk a comma-separated selector list and substitute every
/// occurrence of `.<class_name>` in each selector with `replacement`,
/// dropping selectors that didn't receive a substitution. Returns the
/// joined surviving selectors, or `None` when no selector matched
/// (caller falls back to emitting the rule verbatim).
///
/// Mirrors upstream's `replaceSelector` walk in
/// `expandApplyAtRules.js`. The traversal is byte-level (not a real
/// selector parser): pseudo-class wrappers like `:where(.btn)` /
/// `:is(.btn)` / `:has(.btn:hover)` are handled by recursing through
/// their contents — they're nestable, but the substitution rules are
/// the same. `:not()` is recursed too so a class inside it doesn't
/// accidentally count, but those classes are also valid substitution
/// targets when the candidate matches them.
fn substitute_class_in_selector_list(
    selector: &str,
    class_name: &str,
    replacement: &str,
) -> Option<String> {
    let parts = split_top_level_selectors(selector);
    if parts.is_empty() {
        return None;
    }
    let mut kept: Vec<String> = Vec::with_capacity(parts.len());
    for part in &parts {
        let (replaced, hit) = substitute_class_once(part, class_name, replacement);
        if hit {
            kept.push(replaced);
        }
    }
    if kept.is_empty() {
        return None;
    }
    // Re-join with `, `.
    Some(kept.join(", "))
}

/// Substitute the FIRST `.<class_name>` token in `selector` (walking
/// into pseudo-class wrappers) with `replacement`. Returns the new
/// selector and a hit flag indicating whether substitution happened.
/// `class_name` is the unescaped form (e.g. `btn`); the substitution
/// matches the literal class name in the selector text.
fn substitute_class_once(selector: &str, class_name: &str, replacement: &str) -> (String, bool) {
    let bytes = selector.as_bytes();
    let needle = class_name.as_bytes();
    let mut out = String::with_capacity(selector.len() + replacement.len());
    let mut i = 0;
    let mut hit = false;
    while i < bytes.len() {
        if hit {
            // After the first substitution, copy the rest verbatim.
            out.push_str(&selector[i..]);
            break;
        }
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => {
                out.push_str(&selector[i..i + 2]);
                i += 2;
            }
            b'"' | b'\'' => {
                let q = bytes[i];
                let start = i;
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
                out.push_str(&selector[start..i]);
            }
            b'.' if matches_class_token(&bytes[i + 1..], needle) => {
                // Substitute the class token. `replacement` already
                // includes the leading `.`, so skip it from the source.
                out.push_str(replacement);
                i += 1 + needle.len();
                hit = true;
            }
            _ => {
                out.push(bytes[i] as char);
                i += 1;
            }
        }
    }
    (out, hit)
}

/// True when `rest` starts with `needle` followed by a class-token
/// boundary (end-of-string or a non-identifier character).
fn matches_class_token(rest: &[u8], needle: &[u8]) -> bool {
    if rest.len() < needle.len() {
        return false;
    }
    if &rest[..needle.len()] != needle {
        return false;
    }
    match rest.get(needle.len()).copied() {
        None => true,
        Some(b) => !(b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
    }
}

/// Pick the class within `rule.selector` to substitute against the
/// candidate. Prefers a referenced class whose unescaped form
/// equals the candidate's root (so `.outer:is(.w-full)` substitutes
/// Find the byte offset of the class name `class_name` (immediately
/// preceded by `.`) inside `selector`. Returns the offset of the
/// FIRST character of the class name (skipping the dot).
fn find_class_in_selector(selector: &str, class_name: &str) -> Option<usize> {
    let bytes = selector.as_bytes();
    let needle = class_name.as_bytes();
    let mut i = 0;
    while i + 1 + needle.len() <= bytes.len() {
        if bytes[i] == b'.' && &bytes[i + 1..i + 1 + needle.len()] == needle {
            // Boundary check.
            let after = bytes.get(i + 1 + needle.len()).copied();
            let is_boundary = match after {
                None => true,
                Some(b) => !(b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            };
            if is_boundary {
                return Some(i + 1);
            }
        }
        i += 1;
    }
    None
}

fn optimize_universal_defaults_enabled(config: Option<&serde_json::Value>) -> bool {
    let Some(exp) = config.and_then(|c| c.get("experimental")) else {
        return false;
    };
    // Tailwind accepts both shapes:
    //   - object: `experimental: { optimizeUniversalDefaults: true }`
    //   - string: `experimental: 'all'` — enables every flag.
    match exp {
        serde_json::Value::String(s) => s == "all",
        serde_json::Value::Object(_) => exp
            .get("optimizeUniversalDefaults")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        _ => false,
    }
}

/// Locate `.<primary>` inside a pseudo-class wrapper
/// (`:where(...)`, `:is(...)`, `:has(...)`) and replace it with
/// `replacement`. Returns the replaced selector string when the
/// substitution succeeds (so the caller can use it instead of the
/// raw plugin selector), or `None` when no replacement happened.
/// Used so plugin selectors of the form `:where(.btn)` substitute
/// to `:where(.<candidate-class>)` for variant + important-modifier
/// candidates.
fn replace_class_in_wrapped_selector(
    selector: &str,
    primary: &str,
    replacement: &str,
) -> Option<String> {
    let needle_owned = format!(".{primary}");
    let needle = needle_owned.as_str();
    let bytes = selector.as_bytes();
    let mut depth_paren = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => {
                i += 2;
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
            _ => {}
        }
        // Only substitute INSIDE a pseudo-class wrapper — outside,
        // the leading-`.` path handles the substitution.
        if depth_paren > 0 && selector[i..].starts_with(needle) {
            // Verify the match ends at a class-token boundary so we
            // don't replace `.btn` inside `.btnsuffix`.
            let end = i + needle.len();
            let next = bytes.get(end).copied();
            let is_boundary = match next {
                None => true,
                Some(b) => !(b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            };
            if is_boundary {
                let mut out = String::with_capacity(selector.len() + replacement.len());
                out.push_str(&selector[..i]);
                out.push_str(replacement);
                out.push_str(&selector[end..]);
                return Some(out);
            }
        }
        i += 1;
    }
    None
}

/// Resolved `important` config. Mirrors upstream's three modes:
/// `important: true` → mark every emitted decl `!important`;
/// `important: '<sel>'` → wrap each rule's selectors with the prefix;
/// otherwise no-op.
enum ImportantConfig<'a> {
    Off,
    Bang,
    SelectorPrefix(&'a str),
}

/// Pull flat-string entries out of `config.safelist`. Pattern-form
/// entries (`{ pattern: /regex/, variants: [...] }`) are skipped —
/// matching them properly requires enumerating every candidate the
/// utility tables could produce, which we don't yet support. Documented
/// as a carry-over.
fn read_safelist_strings(config: Option<&serde_json::Value>) -> Vec<String> {
    let Some(arr) = config
        .and_then(|c| c.get("safelist"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// `corePlugins` config filter. Two shapes per upstream:
///
///   - **Array**: `corePlugins: ['display', 'padding']` — *only* the
///     listed plugins are enabled. Everything else is disabled.
///   - **Object**: `corePlugins: { display: false, padding: false }` —
///     everything is enabled by default; named plugins are disabled
///     (or explicitly enabled via `true`).
///   - **Missing**: all plugins enabled. Default.
///
/// The `preflight` flag is intentionally NOT consulted here — it's
/// already wired through the directive processor's
/// `preflight_enabled` and only affects the `@tailwind base` slot.
enum CorePluginsFilter {
    All,
    Whitelist(rustc_hash::FxHashSet<String>),
    Blacklist(rustc_hash::FxHashSet<String>),
}

impl CorePluginsFilter {
    fn from_config(config: Option<&serde_json::Value>) -> Self {
        let Some(val) = config.and_then(|c| c.get("corePlugins")) else {
            return Self::All;
        };
        match val {
            serde_json::Value::Array(arr) => {
                let set: rustc_hash::FxHashSet<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect();
                Self::Whitelist(set)
            }
            serde_json::Value::Object(map) => {
                let disabled: rustc_hash::FxHashSet<String> = map
                    .iter()
                    .filter_map(|(k, v)| {
                        if matches!(v, serde_json::Value::Bool(false)) {
                            Some(k.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                Self::Blacklist(disabled)
            }
            _ => Self::All,
        }
    }

    fn is_enabled(&self, plugin: &str) -> bool {
        match self {
            Self::All => true,
            Self::Whitelist(set) => set.contains(plugin),
            Self::Blacklist(set) => !set.contains(plugin),
        }
    }
}

fn read_blocklist(config: Option<&serde_json::Value>) -> Vec<&str> {
    let Some(arr) = config
        .and_then(|c| c.get("blocklist"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    arr.iter().filter_map(|v| v.as_str()).collect()
}

fn read_important_config(config: Option<&serde_json::Value>) -> ImportantConfig<'_> {
    let Some(cfg) = config else {
        return ImportantConfig::Off;
    };
    match cfg.get("important") {
        Some(serde_json::Value::Bool(true)) => ImportantConfig::Bang,
        Some(serde_json::Value::String(s)) if !s.is_empty() => ImportantConfig::SelectorPrefix(s),
        _ => ImportantConfig::Off,
    }
}

/// Apply the global `important` config to every rule in `rules`.
/// `Bang` flips the `important` flag on every declaration; the
/// selector form prefixes each selector with `<sel> ` (descendant
/// combinator). For variant-prepended structural decls
/// (`content: var(--tw-content)` from `before:`/`after:`) the bang
/// form still applies — upstream emits those as `!important` too
/// when `important: true` is set; tested against the oracle.
fn apply_important_mode(rules: &mut [Rule], mode: &ImportantConfig) {
    match mode {
        ImportantConfig::Off => {}
        ImportantConfig::Bang => {
            for rule in rules.iter_mut() {
                if !rule.respect_important {
                    continue;
                }
                for decl in &mut rule.declarations {
                    decl.important = true;
                }
            }
        }
        ImportantConfig::SelectorPrefix(sel) => {
            for rule in rules.iter_mut() {
                if !rule.respect_important {
                    continue;
                }
                for s in rule.selectors.iter_mut() {
                    // Plugin-emitted rules can carry a multi-selector
                    // string (`.a, .b`) in a single `selectors` slot;
                    // prefixing the whole string only affects the
                    // first selector. Split, prefix each, rejoin.
                    let parts = split_top_level_selectors(s);
                    let mut joined = String::with_capacity(s.len() + parts.len() * (sel.len() + 1));
                    for (i, part) in parts.iter().enumerate() {
                        if i > 0 {
                            joined.push_str(", ");
                        }
                        joined.push_str(sel);
                        joined.push(' ');
                        // Mirror upstream `applyImportantSelector.js`:
                        // selectors with a top-level combinator
                        // (descendant / child / sibling) get wrapped
                        // in `:is()` so the prefix is independent of
                        // DOM position. Combinators inside pseudos
                        // like `:where()` don't count and pass through
                        // unwrapped.
                        if has_top_level_combinator(part) {
                            joined.push_str(":is(");
                            joined.push_str(part);
                            joined.push(')');
                        } else {
                            joined.push_str(part);
                        }
                    }
                    *s = joined;
                }
            }
        }
    }
}

/// Direct port of `collapseDuplicateDeclarations.js` from
/// `vendor/tailwindcss-v3/src/lib/`. Within a single rule body, when
/// the same property appears multiple times:
/// - identical-value duplicates → keep only the last;
/// - same-unit-but-different-value duplicates → keep only the last
///   in each unit group (e.g. `height: 100px; height: 200px;` →
///   `height: 200px;`);
/// - different-unit duplicates → leave all (legacy fallback pattern);
/// - non-numeric-and-not-identical → leave all.
fn collapse_duplicate_decls(body: &str) -> String {
    // Parse `body` into ordered (prop, value, raw_text) triples.
    let mut decls: Vec<(String, String, String)> = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Skip whitespace.
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        // Comment.
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            let start = i;
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            decls.push((String::new(), String::new(), body[start..i].to_string()));
            continue;
        }
        // Read property up to `:`.
        let prop_start = i;
        while i < bytes.len() && bytes[i] != b':' && bytes[i] != b';' && bytes[i] != b'}' {
            if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
                continue;
            }
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b':' {
            // Stray text — emit verbatim.
            if prop_start < i {
                decls.push((
                    String::new(),
                    String::new(),
                    body[prop_start..i].to_string(),
                ));
            }
            if i < bytes.len() {
                decls.push((String::new(), String::new(), body[i..i + 1].to_string()));
                i += 1;
            }
            continue;
        }
        let prop = body[prop_start..i].trim().to_string();
        i += 1; // skip ':'
                // Read value up to top-level `;` or `}`.
        let val_start = i;
        let mut depth_paren = 0i32;
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
                b';' if depth_paren == 0 => break,
                b'}' if depth_paren == 0 => break,
                _ => i += 1,
            }
        }
        let value = body[val_start..i].trim().to_string();
        let raw = format!("{prop}: {value};");
        decls.push((prop, value, raw));
        if i < bytes.len() && bytes[i] == b';' {
            i += 1;
        }
    }

    // First pass: drop earlier duplicates with exact same value.
    // (Walk in source order; keep last seen index.)
    let mut last_seen: rustc_hash::FxHashMap<(String, String), usize> =
        rustc_hash::FxHashMap::default();
    let mut keep: Vec<bool> = vec![true; decls.len()];
    for (idx, (prop, value, _)) in decls.iter().enumerate() {
        if prop.is_empty() {
            continue;
        }
        if let Some(&prev) = last_seen.get(&(prop.clone(), value.clone())) {
            keep[prev] = false;
        }
        last_seen.insert((prop.clone(), value.clone()), idx);
    }

    // Second pass: for each property with multiple non-identical
    // values, group by unit (e.g. 'px', 'rem'); keep only the last
    // in each unit group.
    let mut by_prop: rustc_hash::FxHashMap<String, Vec<usize>> = rustc_hash::FxHashMap::default();
    for (idx, (prop, _, _)) in decls.iter().enumerate() {
        if !keep[idx] || prop.is_empty() {
            continue;
        }
        by_prop.entry(prop.clone()).or_default().push(idx);
    }
    for indices in by_prop.values() {
        if indices.len() < 2 {
            continue;
        }
        let mut by_unit: rustc_hash::FxHashMap<String, Vec<usize>> =
            rustc_hash::FxHashMap::default();
        for &idx in indices {
            let value = &decls[idx].1;
            let unit = match resolve_unit(value) {
                Some(u) => u,
                None => continue,
            };
            by_unit.entry(unit).or_default().push(idx);
        }
        for unit_indices in by_unit.values() {
            if unit_indices.len() < 2 {
                continue;
            }
            // Drop all but the last. They're already in source order.
            for &idx in &unit_indices[..unit_indices.len() - 1] {
                keep[idx] = false;
            }
        }
    }

    let mut out = String::with_capacity(body.len());
    for (idx, (_, _, raw)) in decls.iter().enumerate() {
        if !keep[idx] {
            continue;
        }
        if !out.is_empty() && !out.ends_with(' ') && !out.ends_with('\n') {
            out.push(' ');
        }
        out.push_str(raw);
    }
    out
}

/// Mirrors upstream's `resolveUnit` regex
/// `/^-?\d*.?\d+([\w%]+)?$/`. Returns the unit string ("px", "rem",
/// "%", etc.) or "" for a unitless number, or `None` for non-numeric
/// values (like `var(--x)` or `calc(...)`).
fn resolve_unit(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut i = 0;
    if i < bytes.len() && bytes[i] == b'-' {
        i += 1;
    }
    let digits_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i == digits_start {
        return None; // No digits at all.
    }
    let unit_start = i;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'%') {
        i += 1;
    }
    if i != bytes.len() {
        return None; // Trailing junk — not a pure numeric value.
    }
    Some(input[unit_start..].to_string())
}

/// Direct port of `collapseAdjacentRules.js` from
/// `vendor/tailwindcss-v3/src/lib/`. Walks the CSS string once,
/// merging adjacent rules whose selector matches (whitespace
/// normalized) and adjacent at-rule blocks whose name+params match.
/// Recurses into at-rule blocks so a `@media (...)` block also gets
/// Resolve `@defaults <id>;` markers across the sheet — port of
/// upstream's `vendor/tailwindcss-v3/src/lib/resolveDefaultsAtRules.js`.
///
/// Each utility-using rule that depends on a cascade-var defaults
/// group emits an `@defaults <id>;` marker inside its body during
/// emit. This pass:
///
/// 1. Walks the sheet recursively, stripping `@defaults` markers
///    from rule bodies.
/// 2. For each rule with markers, registers the rule's selector
///    against `(group_id, isolation_bucket)` at the rule's at-rule
///    context level. Selectors containing `:has`, `:-`, or `::-`
///    get their own bucket (keyed by the selector itself); other
///    selectors share `__DEFAULT__`. Mirrors the upstream
///    `selectorGroupName` logic.
/// 3. Prepends a shared rule per `(group, bucket)` to the matching
///    container — top-level groups go at the top of the sheet,
///    `@media`-scoped groups go at the top of that `@media` block.
///
/// The shared rule's body comes from `value_utilities::defaults_for_group`
/// (our static table of `(var, default)` pairs per group).
///
/// Only runs work when `experimental.optimizeUniversalDefaults` is
/// on AND `@tailwind base` is present — both are the conditions under
/// which `rules_for_value_decls` tags markers in the first place.
/// When the flag is off the markers are never written, so this pass
/// is a no-op.
fn resolve_defaults_at_rules_pass(css: &str) -> String {
    if !css.contains("@defaults") {
        return css.to_string();
    }
    let items = parse_top_level_items(css);
    let processed = resolve_defaults_in_level(items);
    let mut out = String::with_capacity(css.len());
    emit_collapsed_items(&processed, &mut out);
    out
}

/// `(bucket_key, selectors)` — bucket key is either `__DEFAULT__`
/// (shared bucket) or the isolating selector itself.
type DefaultsBucket = (String, Vec<String>);
/// `(group_id, buckets)` — preserves insertion order of buckets so
/// upstream's first-seen-bucket-first emit order is mirrored.
type DefaultsGroup = (String, Vec<DefaultsBucket>);

/// One container-level pass: strip `@defaults` markers from any
/// rule bodies AT THIS LEVEL, gather the (group, bucket, selector)
/// triples, recurse into nested at-rule blocks, then prepend the
/// shared defaults rules to the start of the (cleaned) item list.
///
/// Body source for each shared rule:
///   1. A user-written `@defaults <id> { decls }` BLOCK at this
///      level (collected and stripped from output).
///   2. Otherwise `value_utilities::defaults_for_group(<id>)` — the
///      hardcoded table for built-in groups (transform, ring-width,
///      box-shadow). Mirrors upstream's `addDefaults('group', { ... })`
///      JS API which writes the BLOCK form internally.
fn resolve_defaults_in_level(items: Vec<CssItem>) -> Vec<CssItem> {
    // Insertion-order Vec of groups; each group has insertion-order
    // buckets. Upstream emits the first-seen bucket first across
    // both axes — we mirror that.
    let mut groups: Vec<DefaultsGroup> = Vec::new();
    // User-supplied bodies for `@defaults <id> { decls }` blocks
    // detected at this level. Maps id -> serialized body.
    let mut user_bodies: Vec<(String, String)> = Vec::new();
    let mut new_items: Vec<CssItem> = Vec::with_capacity(items.len());
    for item in items {
        match item {
            CssItem::Rule { selector, body } => {
                let (clean_body, marker_groups) = extract_defaults_markers(&body);
                if !marker_groups.is_empty() {
                    // Each comma-separated selector contributes its
                    // reduced form to the bucket. Upstream's
                    // `extractElementSelector` returns one entry per
                    // top-level selector, isolation-bucketed
                    // independently.
                    let parts = split_top_level_commas_owned(&selector);
                    for part in parts {
                        let reduced = reduce_to_element_selector(&part);
                        let bucket_key = if selector_isolates_defaults_for_pass(&reduced) {
                            reduced.clone()
                        } else {
                            "__DEFAULT__".to_string()
                        };
                        for group in &marker_groups {
                            let group_entry = match groups.iter_mut().find(|(g, _)| g == group) {
                                Some(e) => e,
                                None => {
                                    groups.push((group.clone(), Vec::new()));
                                    groups.last_mut().unwrap()
                                }
                            };
                            let bucket =
                                match group_entry.1.iter_mut().find(|(b, _)| *b == bucket_key) {
                                    Some(b) => b,
                                    None => {
                                        group_entry.1.push((bucket_key.clone(), Vec::new()));
                                        group_entry.1.last_mut().unwrap()
                                    }
                                };
                            if !bucket.1.contains(&reduced) {
                                bucket.1.push(reduced.clone());
                            }
                        }
                    }
                }
                new_items.push(CssItem::Rule {
                    selector,
                    body: clean_body,
                });
            }
            CssItem::AtBlock {
                name,
                params,
                items: inner,
            } => {
                if name == "defaults" {
                    // User-written `@defaults <id> { decls }` block.
                    // Serialize inner items into a body string and
                    // register it; the block itself is dropped from
                    // output (mirrors upstream's `universal.remove()`
                    // after it builds the shared rules).
                    let mut body = String::new();
                    emit_collapsed_items(&inner, &mut body);
                    user_bodies.push((params, body.trim().to_string()));
                    continue;
                }
                let inner = resolve_defaults_in_level(inner);
                new_items.push(CssItem::AtBlock {
                    name,
                    params,
                    items: inner,
                });
            }
            other => new_items.push(other),
        }
    }
    if groups.is_empty() {
        return new_items;
    }
    let mut shared: Vec<CssItem> = Vec::new();
    for (group, buckets) in groups {
        // Prefer user-supplied body; fall back to hardcoded table.
        let body = if let Some((_, body)) = user_bodies.iter().find(|(g, _)| *g == group) {
            body.clone()
        } else {
            let decls = value_utilities::defaults_for_group(&group);
            if decls.is_empty() {
                continue;
            }
            let mut s = String::new();
            for (i, (var, default)) in decls.iter().enumerate() {
                if i > 0 {
                    s.push(' ');
                }
                s.push_str(var);
                s.push_str(": ");
                s.push_str(default);
                s.push(';');
            }
            s
        };
        for (_, selectors) in buckets {
            if selectors.is_empty() {
                continue;
            }
            let selector = selectors.join(", ");
            shared.push(CssItem::Rule {
                selector,
                body: body.clone(),
            });
        }
    }
    let mut result: Vec<CssItem> = Vec::with_capacity(shared.len() + new_items.len());
    result.extend(shared);
    result.extend(new_items);
    result
}

/// Split a selector list by top-level commas. Mirrors
/// `directives.rs::split_top_level_commas` but returns owned
/// `String`s and is private to this pass.
fn split_top_level_commas_owned(selector: &str) -> Vec<String> {
    let bytes = selector.as_bytes();
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
            b',' if depth_paren == 0 && depth_bracket == 0 => {
                parts.push(selector[start..i].trim().to_string());
                start = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    parts.push(selector[start..].trim().to_string());
    parts
}

/// Reduce a single (non-comma) selector to its lowest-specificity
/// form for the cascade-var defaults bucket. Mirrors upstream's
/// `minimumImpactSelector` in `resolveDefaultsAtRules.js`.
///
/// Steps:
/// 1. Take the LAST compound (after the final descendant ` `
///    combinator) — `extractElementSelector`'s `s.split(...).pop()`.
/// 2. Tokenize that compound into nodes.
/// 3. Drop empty pseudo-classes (keep pseudo-elements + parametric
///    pseudos like `:not(...)`).
/// 4. Find the LAST tag/class/id/attribute node and use it as the
///    "best" anchor — IDs convert to `[id='x']` to drop their
///    specificity from 100 to 10.
/// 5. Concatenate that anchor with everything to its right
///    (typically just trailing pseudo-elements).
fn reduce_to_element_selector(selector: &str) -> String {
    let last_compound = take_last_compound(selector);
    let nodes = tokenize_compound(&last_compound);
    let filtered: Vec<&SelToken> = nodes
        .iter()
        .filter(|n| match n {
            SelToken::Pseudo {
                has_subnodes,
                is_element,
                ..
            } => *has_subnodes || *is_element,
            _ => true,
        })
        .collect();
    let mut split_idx: Option<usize> = None;
    for (i, n) in filtered.iter().enumerate().rev() {
        if matches!(
            n,
            SelToken::Tag(_) | SelToken::Class(_) | SelToken::Id(_) | SelToken::Attribute(_)
        ) {
            split_idx = Some(i);
            break;
        }
    }
    let Some(idx) = split_idx else {
        // No anchor — emit the filtered tokens verbatim.
        return filtered
            .iter()
            .map(|n| n.text())
            .collect::<String>()
            .trim()
            .to_string();
    };
    let anchor = match filtered[idx] {
        SelToken::Id(name) => {
            // Match postcss-selector-parser's serialization: when
            // `quoteMark` is set but the value is a valid CSS
            // identifier, the parser omits quotes. We mirror that
            // by quoting only when the value contains a character
            // that wouldn't be safe bare.
            if name.bytes().all(is_ident_byte) {
                format!("[id={name}]")
            } else {
                format!("[id=\"{name}\"]")
            }
        }
        other => other.text().to_string(),
    };
    let mut out = anchor;
    for n in &filtered[idx + 1..] {
        out.push_str(&n.text());
    }
    out.trim().to_string()
}

/// Take the last descendant-combinator separated compound from a
/// (single, comma-free) selector. e.g. `.a .b > .c` -> `.b > .c`.
/// Splits only on whitespace combinators that appear at top level
/// (not inside `(`, `[`, etc.).
fn take_last_compound(selector: &str) -> String {
    let bytes = selector.as_bytes();
    let mut last_start = 0usize;
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
            b' ' | b'\t' if depth_paren == 0 && depth_bracket == 0 => {
                // Skip a run of whitespace; if the preceding
                // non-whitespace char isn't a combinator (`>`/`+`/
                // `~`), this whitespace IS a descendant combinator.
                let was_combinator = (last_start..i)
                    .rev()
                    .find(|&j| !bytes[j].is_ascii_whitespace())
                    .map(|j| matches!(bytes[j], b'>' | b'+' | b'~'))
                    .unwrap_or(false);
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                if !was_combinator && i < bytes.len() && !matches!(bytes[i], b'>' | b'+' | b'~') {
                    last_start = i;
                }
            }
            _ => i += 1,
        }
    }
    selector[last_start..].trim().to_string()
}

#[derive(Debug, Clone)]
enum SelToken {
    Tag(String),
    Class(String),
    Id(String),
    Attribute(String),
    Pseudo {
        text: String,
        has_subnodes: bool,
        is_element: bool,
    },
    Combinator(String),
    Universal,
}

impl SelToken {
    fn text(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Self::Tag(s) => std::borrow::Cow::Borrowed(s),
            Self::Class(s) => std::borrow::Cow::Owned(format!(".{s}")),
            Self::Id(s) => std::borrow::Cow::Owned(format!("#{s}")),
            Self::Attribute(s) => std::borrow::Cow::Borrowed(s),
            Self::Pseudo { text, .. } => std::borrow::Cow::Borrowed(text),
            Self::Combinator(s) => std::borrow::Cow::Borrowed(s),
            Self::Universal => std::borrow::Cow::Borrowed("*"),
        }
    }
}

/// Tokenize a single compound selector into nodes. Recognizes:
/// tag, class, id, attribute (`[…]`), pseudo (`:foo`, `::foo`,
/// `:foo(...)`), universal (`*`), and combinators (`>` / `+` / `~`).
fn tokenize_compound(input: &str) -> Vec<SelToken> {
    let bytes = input.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        match b {
            b'.' => {
                let start = i + 1;
                let j = read_ident_with_escapes(bytes, start);
                tokens.push(SelToken::Class(input[start..j].to_string()));
                i = j;
            }
            b'#' => {
                let start = i + 1;
                let j = read_ident_with_escapes(bytes, start);
                tokens.push(SelToken::Id(input[start..j].to_string()));
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
                tokens.push(SelToken::Attribute(input[i..j].to_string()));
                i = j;
            }
            b':' => {
                let is_element_double = i + 1 < bytes.len() && bytes[i + 1] == b':';
                let name_start = if is_element_double { i + 2 } else { i + 1 };
                let j = read_ident_with_escapes(bytes, name_start);
                let name = &input[name_start..j];
                let legacy_element =
                    matches!(name, "before" | "after" | "first-line" | "first-letter")
                        && !is_element_double;
                let is_element = is_element_double || legacy_element;
                let mut has_subnodes = false;
                let mut k = j;
                if k < bytes.len() && bytes[k] == b'(' {
                    has_subnodes = true;
                    let mut depth = 1i32;
                    k += 1;
                    while k < bytes.len() && depth > 0 {
                        match bytes[k] {
                            b'\\' if k + 1 < bytes.len() => k += 2,
                            b'(' => {
                                depth += 1;
                                k += 1;
                            }
                            b')' => {
                                depth -= 1;
                                k += 1;
                            }
                            _ => k += 1,
                        }
                    }
                }
                tokens.push(SelToken::Pseudo {
                    text: input[i..k].to_string(),
                    has_subnodes,
                    is_element,
                });
                i = k;
            }
            b'*' => {
                tokens.push(SelToken::Universal);
                i += 1;
            }
            b'>' | b'+' | b'~' => {
                tokens.push(SelToken::Combinator(input[i..i + 1].to_string()));
                i += 1;
            }
            _ if is_ident_byte(b) => {
                let start = i;
                let j = read_ident_with_escapes(bytes, i);
                tokens.push(SelToken::Tag(input[start..j].to_string()));
                i = j;
            }
            _ => i += 1,
        }
    }
    tokens
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

/// Advance over an identifier including CSS escape sequences:
/// `\<char>` consumes both bytes as a single ident character.
/// Mirrors `cssesc({ isIdentifier: true })`'s output. Used for class
/// names like `has-\[\:checked\]\:ring-1` so we don't split on the
/// escaped `[` / `:` / `]`.
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

/// Strip `@defaults <id>;` markers from a rule body and return the
/// cleaned body plus the list of group ids found. Markers are
/// recognized at any position in the body — leading whitespace is
/// preserved as-is, and trailing whitespace/newline after a stripped
/// marker is consumed so the cleaned body doesn't grow extra blank
/// lines.
fn extract_defaults_markers(body: &str) -> (String, Vec<String>) {
    if !body.contains("@defaults") {
        return (body.to_string(), Vec::new());
    }
    let mut groups: Vec<String> = Vec::new();
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(idx) = rest.find("@defaults") {
        // Confirm `@defaults` is followed by whitespace (not e.g.
        // `@defaultsBoo`). The valid CSS shape after `@defaults`
        // is whitespace then identifier then `;`.
        let after = &rest[idx + "@defaults".len()..];
        let next = after.chars().next();
        if !matches!(next, Some(c) if c.is_ascii_whitespace()) {
            // Not a real marker — copy up to and including the
            // `@defaults` token, continue past it.
            let advance = idx + "@defaults".len();
            out.push_str(&rest[..advance]);
            rest = &rest[advance..];
            continue;
        }
        out.push_str(&rest[..idx]);
        // Find terminator: `;` for bare marker, `{` for block
        // (block form unsupported — pass through).
        let term_pos = after.find([';', '{']);
        let Some(t) = term_pos else {
            // No terminator — preserve the rest verbatim and stop.
            out.push_str(&rest[idx..]);
            rest = "";
            break;
        };
        let term_byte = after.as_bytes()[t];
        if term_byte == b'{' {
            // Block form — leave verbatim, advance past the
            // `@defaults` token (the block body will be re-walked
            // by the outer loop if reached).
            let advance = idx + "@defaults".len();
            out.push_str(&rest[idx..advance]);
            rest = &rest[advance..];
            continue;
        }
        // `;` terminator — record id, strip marker.
        let name = after[..t].trim();
        if !name.is_empty() {
            groups.push(name.to_string());
        }
        let mut tail_start = idx + "@defaults".len() + t + 1;
        let bytes = rest.as_bytes();
        while tail_start < bytes.len() && matches!(bytes[tail_start], b' ' | b'\t' | b'\n' | b'\r')
        {
            tail_start += 1;
        }
        rest = &rest[tail_start..];
    }
    out.push_str(rest);
    (out, groups)
}

/// Selectors with vendor-prefixed pseudos (`:-moz-…`, `::-webkit-…`)
/// or `:has(...)` get their own per-selector bucket — merging them
/// with sibling rules can cause the entire shared block to be
/// rejected by browsers that don't support the construct. Mirrors
/// upstream's `selectorGroupName` keying in
/// `resolveDefaultsAtRules.js`.
fn selector_isolates_defaults_for_pass(selector: &str) -> bool {
    selector.contains(":has") || selector.contains(":-") || selector.contains("::-")
}

fn collapse_adjacent_rules_pass(css: &str) -> String {
    // Parse into a flat list of items at the top level, recursing
    // into at-rule bodies. `@font-face` rules opt out of the merge
    // (mirrors upstream).
    let items = parse_top_level_items(css);
    let mut out = String::with_capacity(css.len());
    emit_collapsed_items(&items, &mut out);
    out
}

#[derive(Debug)]
enum CssItem {
    Comment(String),
    /// A rule: `<selector> { <body> }`. Body is the raw declarations
    /// text, unparsed.
    Rule {
        selector: String,
        body: String,
    },
    /// An at-rule with a block body (`@media`, `@supports`, etc.)
    /// holding its own list of items.
    AtBlock {
        name: String,
        params: String,
        items: Vec<CssItem>,
    },
    /// A simple `@<name> <params>;` rule (`@import`, `@charset`).
    AtSimple {
        text: String,
    },
    /// Raw whitespace / unparseable bytes — emitted verbatim.
    Other(String),
}

fn parse_top_level_items(input: &str) -> Vec<CssItem> {
    let bytes = input.as_bytes();
    let mut items = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // Whitespace.
        if bytes[i].is_ascii_whitespace() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            items.push(CssItem::Other(input[start..i].to_string()));
            continue;
        }
        // Comment.
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            let start = i;
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            items.push(CssItem::Comment(input[start..i].to_string()));
            continue;
        }
        // At-rule.
        if bytes[i] == b'@' {
            let start = i;
            // Find either `;` (simple) or `{` (block) at top level.
            let mut j = i + 1;
            let mut depth_paren = 0i32;
            let mut depth_bracket = 0i32;
            while j < bytes.len() {
                match bytes[j] {
                    b'\\' if j + 1 < bytes.len() => j += 2,
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
                    b'(' => {
                        depth_paren += 1;
                        j += 1;
                    }
                    b')' => {
                        depth_paren -= 1;
                        j += 1;
                    }
                    b'[' => {
                        depth_bracket += 1;
                        j += 1;
                    }
                    b']' => {
                        depth_bracket -= 1;
                        j += 1;
                    }
                    b';' if depth_paren == 0 && depth_bracket == 0 => break,
                    b'{' if depth_paren == 0 && depth_bracket == 0 => break,
                    _ => j += 1,
                }
            }
            if j < bytes.len() && bytes[j] == b';' {
                items.push(CssItem::AtSimple {
                    text: input[start..j + 1].to_string(),
                });
                i = j + 1;
                continue;
            }
            if j < bytes.len() && bytes[j] == b'{' {
                let header = &input[start + 1..j]; // skip '@'
                let header_trimmed = header.trim();
                let (name, params) = match header_trimmed.find(char::is_whitespace) {
                    Some(idx) => {
                        let n = &header_trimmed[..idx];
                        let p = header_trimmed[idx..].trim();
                        (n.to_string(), p.to_string())
                    }
                    None => (header_trimmed.to_string(), String::new()),
                };
                let body_start = j + 1;
                let body_end = find_matching_close_brace_top(bytes, j);
                let body = &input[body_start..body_end];
                items.push(CssItem::AtBlock {
                    name,
                    params,
                    items: parse_top_level_items(body),
                });
                i = body_end + 1;
                continue;
            }
            // Couldn't find terminator — emit verbatim.
            items.push(CssItem::Other(input[start..].to_string()));
            return items;
        }
        // Plain rule: <selector> { <body> }.
        let start = i;
        let mut j = i;
        let mut depth_paren = 0i32;
        let mut depth_bracket = 0i32;
        while j < bytes.len() {
            match bytes[j] {
                b'\\' if j + 1 < bytes.len() => j += 2,
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
                b'(' => {
                    depth_paren += 1;
                    j += 1;
                }
                b')' => {
                    depth_paren -= 1;
                    j += 1;
                }
                b'[' => {
                    depth_bracket += 1;
                    j += 1;
                }
                b']' => {
                    depth_bracket -= 1;
                    j += 1;
                }
                b'{' if depth_paren == 0 && depth_bracket == 0 => break,
                b'}' if depth_paren == 0 && depth_bracket == 0 => {
                    // Stray `}` — emit and advance. The post-loop
                    // `i = (j + 1).min(...)` overwrites correctly.
                    items.push(CssItem::Other(input[start..j].to_string()));
                    items.push(CssItem::Other(input[j..j + 1].to_string()));
                    break;
                }
                _ => j += 1,
            }
        }
        if j >= bytes.len() {
            items.push(CssItem::Other(input[start..].to_string()));
            return items;
        }
        if bytes[j] != b'{' {
            // Hit a stray `}` already handled above.
            i = (j + 1).min(bytes.len());
            continue;
        }
        let body_start = j + 1;
        let body_end = find_matching_close_brace_top(bytes, j);
        let selector = input[start..j].trim().to_string();
        let body = input[body_start..body_end].to_string();
        items.push(CssItem::Rule { selector, body });
        i = body_end + 1;
    }
    items
}

fn find_matching_close_brace_top(bytes: &[u8], open: usize) -> usize {
    debug_assert_eq!(bytes[open], b'{');
    let mut depth = 1i32;
    let mut i = open + 1;
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
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    bytes.len()
}

fn emit_collapsed_items(items: &[CssItem], out: &mut String) {
    let mut i = 0;
    while i < items.len() {
        match &items[i] {
            CssItem::Other(s) | CssItem::Comment(s) => {
                out.push_str(s);
                i += 1;
            }
            CssItem::AtSimple { text } => {
                // Mirror upstream `collapseAdjacentRules.js`: an at-rule
                // without children (`@import`, `@charset`) deduplicates
                // when an identical adjacent at-rule follows. Identity
                // is name+params with whitespace normalized.
                out.push_str(text);
                let key = normalize_at_simple_key(text);
                let mut j = i + 1;
                while j < items.len() {
                    match &items[j] {
                        CssItem::Other(s) if s.trim().is_empty() => j += 1,
                        CssItem::AtSimple { text: t2 } if normalize_at_simple_key(t2) == key => {
                            j += 1;
                        }
                        _ => break,
                    }
                }
                i = j;
            }
            CssItem::Rule { selector, body } => {
                let mut combined = body.clone();
                let key = normalize_selector_key(selector);
                let mut j = i + 1;
                // Skip non-significant items between adjacent rules
                // — but only whitespace. Comments BREAK the merge
                // since they could be load-bearing.
                while j < items.len() {
                    match &items[j] {
                        CssItem::Other(s) if s.trim().is_empty() => j += 1,
                        CssItem::Rule {
                            selector: s2,
                            body: b2,
                        } if normalize_selector_key(s2) == key => {
                            if !combined.trim_end().ends_with(';') && !combined.trim().is_empty() {
                                combined.push(';');
                            }
                            combined.push(' ');
                            combined.push_str(b2);
                            j += 1;
                        }
                        _ => break,
                    }
                }
                let collapsed_body = collapse_duplicate_decls(&combined);
                out.push_str(selector);
                out.push_str(" { ");
                out.push_str(&collapsed_body);
                out.push_str(" }\n");
                i = j;
            }
            CssItem::AtBlock {
                name,
                params,
                items: inner,
            } => {
                if name == "font-face" || name == "keyframes" || name == "-webkit-keyframes" {
                    // Don't merge:
                    // * `@font-face` — different fonts share a
                    //   pseudo-selector but different identities;
                    // * `@keyframes` — upstream's `animation` plugin
                    //   re-emits the same `@keyframes <name>` per
                    //   candidate, and the conformance harness keys
                    //   each as a separate `(context, selector)`
                    //   pair. Merging would collapse the rule count
                    //   below the oracle's. Keyframes also have
                    //   percentage-style children that aren't
                    //   selectors — running our merger on them
                    //   would introduce nonsense.
                    out.push('@');
                    out.push_str(name);
                    if !params.is_empty() {
                        out.push(' ');
                        out.push_str(params);
                    }
                    out.push_str(" { ");
                    emit_collapsed_items(inner, out);
                    out.push_str(" }\n");
                    i += 1;
                    continue;
                }
                let mut combined_items: Vec<CssItem> = inner
                    .iter()
                    .map(|item| match item {
                        CssItem::Other(s) => CssItem::Other(s.clone()),
                        CssItem::Comment(s) => CssItem::Comment(s.clone()),
                        CssItem::AtSimple { text } => CssItem::AtSimple { text: text.clone() },
                        CssItem::Rule { selector, body } => CssItem::Rule {
                            selector: selector.clone(),
                            body: body.clone(),
                        },
                        CssItem::AtBlock {
                            name,
                            params,
                            items: nested,
                        } => CssItem::AtBlock {
                            name: name.clone(),
                            params: params.clone(),
                            items: nested.clone_items(),
                        },
                    })
                    .collect();
                let key = (name.clone(), normalize_at_params(params));
                let mut j = i + 1;
                while j < items.len() {
                    match &items[j] {
                        CssItem::Other(s) if s.trim().is_empty() => j += 1,
                        CssItem::AtBlock {
                            name: n2,
                            params: p2,
                            items: inner2,
                        } if name != "font-face"
                            && (n2.clone(), normalize_at_params(p2)) == key =>
                        {
                            for item in inner2.iter() {
                                combined_items.push(match item {
                                    CssItem::Other(s) => CssItem::Other(s.clone()),
                                    CssItem::Comment(s) => CssItem::Comment(s.clone()),
                                    CssItem::AtSimple { text } => {
                                        CssItem::AtSimple { text: text.clone() }
                                    }
                                    CssItem::Rule { selector, body } => CssItem::Rule {
                                        selector: selector.clone(),
                                        body: body.clone(),
                                    },
                                    CssItem::AtBlock {
                                        name,
                                        params,
                                        items: nested,
                                    } => CssItem::AtBlock {
                                        name: name.clone(),
                                        params: params.clone(),
                                        items: nested.clone_items(),
                                    },
                                });
                            }
                            j += 1;
                        }
                        _ => break,
                    }
                }
                out.push('@');
                out.push_str(name);
                if !params.is_empty() {
                    out.push(' ');
                    out.push_str(params);
                }
                out.push_str(" { ");
                emit_collapsed_items(&combined_items, out);
                out.push_str(" }\n");
                i = j;
            }
        }
    }
}

trait CloneItems {
    fn clone_items(&self) -> Vec<CssItem>;
}

impl CloneItems for [CssItem] {
    fn clone_items(&self) -> Vec<CssItem> {
        self.iter()
            .map(|item| match item {
                CssItem::Other(s) => CssItem::Other(s.clone()),
                CssItem::Comment(s) => CssItem::Comment(s.clone()),
                CssItem::AtSimple { text } => CssItem::AtSimple { text: text.clone() },
                CssItem::Rule { selector, body } => CssItem::Rule {
                    selector: selector.clone(),
                    body: body.clone(),
                },
                CssItem::AtBlock {
                    name,
                    params,
                    items,
                } => CssItem::AtBlock {
                    name: name.clone(),
                    params: params.clone(),
                    items: items.clone_items(),
                },
            })
            .collect()
    }
}

fn normalize_selector_key(selector: &str) -> String {
    selector.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_at_params(params: &str) -> String {
    params.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_at_simple_key(text: &str) -> String {
    let trimmed = text.trim().trim_end_matches(';');
    trimmed.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Reconstruct the candidate's class form WITHOUT variants /
/// `!` / negation prefix, but WITH the modifier merged back in.
/// Used for plugin lookups so a modifier like `/4` that's part of
/// a plugin's class name (`custom-top-1/4`) finds a match —
/// otherwise the candidate parser splits it off as opacity and
/// the plugin lookup misses.
fn render_class_form_no_modifier(parsed: &ParsedCandidate) -> String {
    match parsed.modifier.as_ref() {
        Some(galeforce_parser::Modifier::Named(s)) => format!("{}/{}", parsed.root, s),
        Some(galeforce_parser::Modifier::Arbitrary(s)) => format!("{}/[{}]", parsed.root, s),
        None => parsed.root.to_string(),
    }
}

/// Strip CSS escape backslashes (`\\`) from a class identifier.
/// The plugin escape helper turns `custom-top-1/4` into
/// `custom-top-1\/4` for use in selectors; we need the
/// unescaped form to match against the parsed candidate root.
fn strip_css_escapes(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            out.push(bytes[i + 1] as char);
            i += 2;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Split a selector string on top-level commas only — those not
/// inside `()` or `[]`. Used by `apply_important_mode` to prefix
/// each comma-separated selector individually so plugin rules with
/// `.a, .b { ... }` produce `<sel> .a, <sel> .b`.
/// True when the selector contains a top-level combinator (` `,
/// `>`, `+`, `~`) — i.e. a descendant / child / sibling combinator
/// outside of any pseudo-class wrapper or attribute bracket.
/// Mirrors upstream's check in `applyImportantSelector.js`: only
/// top-level combinators force a `:is()` wrap.
fn has_top_level_combinator(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    let mut i = 0;
    let mut prev_non_ws = 0u8;
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
                continue;
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
                // A whitespace combinator only counts when it sits
                // between two simple-selector tokens. Leading or
                // trailing whitespace doesn't, and consecutive
                // whitespace collapses to one.
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

fn split_top_level_selectors(input: &str) -> Vec<String> {
    let bytes = input.as_bytes();
    let mut parts = Vec::new();
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
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
    if parts.is_empty() {
        parts.push(input.to_string());
    }
    parts
}

/// Pull a min-width string from a single screen entry. Accepts:
/// - `"768px"` — bare string.
/// - `{ min: '768px' }` / `{ 'min-width': '768px' }` — object form.
/// - `{ min: 'X', max: 'Y' }` — combined form (returns the min).
/// - `{ max: '...' }` / `{ raw: '...' }` — returns `None`.
fn screen_min_width(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(map) => map
            .get("min")
            .and_then(|x| x.as_str())
            .or_else(|| map.get("min-width").and_then(|x| x.as_str()))
            .map(str::to_string),
        _ => None,
    }
}

/// Build rules for the `container` utility. Direct port of upstream's
/// `corePlugins.js` `container` plugin: emits a base rule with
/// `width: 100%`, optional `padding-{left,right}` and `margin-{left,
/// right}: auto` from `theme.container.{padding, center}`, plus one
/// `@media (min-width: <screen>)` rule per configured screen with
/// `max-width: <screen>` (and per-screen padding overrides if
/// `theme.container.padding` is an object).
fn rules_for_container(
    class_selector: &str,
    cx: &VariantContext,
    config: Option<&serde_json::Value>,
    important: bool,
) -> Vec<Rule> {
    let mut rules: Vec<Rule> = Vec::new();
    let container_cfg = config.and_then(|c| c.pointer("/theme/container"));
    let center = container_cfg
        .and_then(|c| c.get("center"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    // padding: string | { DEFAULT, sm, md, lg, xl, 2xl, ... }
    let padding_default: Option<String> =
        container_cfg
            .and_then(|c| c.get("padding"))
            .and_then(|p| match p {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Object(o) => o
                    .get("DEFAULT")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                _ => None,
            });
    let padding_per_screen = container_cfg
        .and_then(|c| c.get("padding"))
        .and_then(|p| p.as_object())
        .cloned()
        .unwrap_or_default();

    // Base rule.
    let mut base_decls: Vec<Declaration> = vec![Declaration {
        property: "width".to_string(),
        value: "100%".to_string(),
        important,
    }];
    if let Some(p) = &padding_default {
        base_decls.push(Declaration {
            property: "padding-right".to_string(),
            value: p.clone(),
            important,
        });
        base_decls.push(Declaration {
            property: "padding-left".to_string(),
            value: p.clone(),
            important,
        });
    }
    if center {
        base_decls.push(Declaration {
            property: "margin-right".to_string(),
            value: "auto".to_string(),
            important,
        });
        base_decls.push(Declaration {
            property: "margin-left".to_string(),
            value: "auto".to_string(),
            important,
        });
    }
    rules.push(Rule {
        selectors: smallvec::smallvec![class_selector.to_string()],
        declarations: base_decls,
        at_rules: Vec::new(),
        // Container is a component (`is_component: true` upstream).
        // Components default to `respectImportant: false`.
        respect_important: false,
        defaults_groups: Vec::new(),
    });

    // `theme.container.screens` overrides the global `theme.screens`
    // for container's per-screen `max-width` rules. Mirrors
    // upstream's `containerPlugin.js`. Accepts both shapes:
    //   - object: `{ sm: '400px', md: '500px' }` → name + value pairs
    //   - array : `['400px', '500px']` → values self-key as the name
    //     (only the min-width matters for the emitted `@media` rule)
    // Container-specific screens table, when present, overrides the
    // global `theme.screens` for the container's per-breakpoint
    // max-width rules. Pulls a min-width from each entry — string
    // values are taken as-is; `{ min, max }` / `{ 'min-width' }`
    // objects yield their min only (the max is dropped because
    // container only cares about the "this breakpoint and up"
    // boundary). Raw `{ raw: '...' }` entries are skipped.
    let container_screens: Option<Vec<(String, String)>> = container_cfg
        .and_then(|c| c.get("screens"))
        .and_then(|v| match v {
            serde_json::Value::Object(obj) => Some(
                obj.iter()
                    .filter_map(|(k, v)| screen_min_width(v).map(|mw| (k.clone(), mw)))
                    .collect(),
            ),
            serde_json::Value::Array(arr) => Some(
                arr.iter()
                    .filter_map(|v| screen_min_width(v).map(|mw| (mw.clone(), mw)))
                    .collect(),
            ),
            _ => None,
        });
    let screens_iter: Vec<(String, String)> = match container_screens {
        Some(list) => list,
        None => {
            // Build min-width entries from the raw config screens
            // (not `cx.screens`, which collapses object-form screens
            // into a media-body string for variant emission). For
            // container we want the min-width only.
            let theme_screens = config.and_then(|c| c.pointer("/theme/screens"));
            match theme_screens {
                Some(serde_json::Value::Object(obj)) => obj
                    .iter()
                    .filter_map(|(k, v)| screen_min_width(v).map(|mw| (k.clone(), mw)))
                    .collect(),
                Some(serde_json::Value::Array(arr)) => arr
                    .iter()
                    .filter_map(|v| screen_min_width(v).map(|mw| (mw.clone(), mw)))
                    .collect(),
                _ => cx
                    .screens
                    .iter()
                    .filter(|s| !s.is_raw)
                    .map(|s| (s.name.clone(), s.value.clone()))
                    .collect(),
            }
        }
    };

    // Per-screen rules.
    for (name, value) in &screens_iter {
        let mut decls = vec![Declaration {
            property: "max-width".to_string(),
            value: value.clone(),
            important,
        }];
        if let Some(per_screen) = padding_per_screen.get(name).and_then(|v| v.as_str()) {
            decls.push(Declaration {
                property: "padding-right".to_string(),
                value: per_screen.to_string(),
                important,
            });
            decls.push(Declaration {
                property: "padding-left".to_string(),
                value: per_screen.to_string(),
                important,
            });
        }
        rules.push(Rule {
            selectors: smallvec::smallvec![class_selector.to_string()],
            declarations: decls,
            at_rules: vec![format!("@media (min-width: {value})")],
            // Per-screen container rules are also components.
            respect_important: false,
            defaults_groups: Vec::new(),
        });
    }

    rules
}

/// Editor-extension reflection API. Mirrors Tailwind's
/// `getClassList()` and `getVariants()` — used by IntelliSense /
/// LSP integrations to populate class-name and variant
/// autocomplete in HTML/JSX contexts.
///
/// MVP scope: returns only the static utility surface (every
/// registered name in `STATIC_UTILITIES` + sibling-pair statics)
/// and the canonical built-in variants + any plugin variants that
/// rode through `__pluginOutput`. Value-bearing utilities
/// (`mt-1`, `bg-red-500`, etc.) are NOT enumerated — that requires
/// walking every theme key cross-product, which is large and
/// infrequently needed (most editors do prefix-match autocomplete
/// from typed input). Carry-over.
pub fn list_class_names(config: Option<&serde_json::Value>) -> Vec<String> {
    let mut out: Vec<String> = static_utilities::static_names()
        .chain(static_utilities::sibling_static_names())
        .map(str::to_string)
        .collect();
    // Plugin-defined utilities + components.
    let plugin_output = plugins::read_plugin_output(config);
    for rule in plugin_output
        .utilities
        .iter()
        .chain(plugin_output.components.iter())
    {
        if let Some(class) = rule.primary_class() {
            out.push(class.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Built-in variant names + any plugin-defined variants. Returns
/// the bare names; format strings are intentionally not exposed
/// (an editor only needs the variant identifier to autocomplete).
pub fn list_variant_names(config: Option<&serde_json::Value>) -> Vec<String> {
    let mut out: Vec<String> = built_in_variant_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    let plugin_output = plugins::read_plugin_output(config);
    for v in &plugin_output.variants {
        out.push(v.name.clone());
    }
    // Default screens. We could read from config but for simplicity
    // expose the canonical Tailwind set; a config override doesn't
    // typically remove these — it just adjusts breakpoints.
    for s in ["sm", "md", "lg", "xl", "2xl"] {
        out.push(s.to_string());
    }
    out.sort();
    out.dedup();
    out
}

fn built_in_variant_names() -> &'static [&'static str] {
    // Ordered roughly by frequency of use in real templates so the
    // dedup pass produces a stable canonical order. Keep in sync
    // with `resolve_variant` in `variants.rs` when new built-ins
    // land.
    &[
        "hover",
        "focus",
        "focus-visible",
        "focus-within",
        "active",
        "visited",
        "target",
        "first",
        "last",
        "only",
        "odd",
        "even",
        "first-of-type",
        "last-of-type",
        "only-of-type",
        "empty",
        "disabled",
        "enabled",
        "checked",
        "indeterminate",
        "default",
        "required",
        "valid",
        "invalid",
        "in-range",
        "out-of-range",
        "placeholder-shown",
        "autofill",
        "read-only",
        "open",
        "before",
        "after",
        "placeholder",
        "marker",
        "selection",
        "file",
        "backdrop",
        "first-letter",
        "first-line",
        "ltr",
        "rtl",
        "dark",
        "motion-safe",
        "motion-reduce",
        "print",
        "portrait",
        "landscape",
        "contrast-more",
        "contrast-less",
        "forced-colors",
        "*",
        "has",
        "group",
        "peer",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use galeforce_core::FeatureFlags;

    fn opts(candidates: &[&str]) -> CompileOptions {
        CompileOptions {
            candidates: candidates.iter().map(|s| (*s).to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn color_emits_var_opacity_with_1_fallback_by_default() {
        // Default behavior — matches Tailwind v3.4.19 (our pinned target).
        // The `, 1` fallback was added in v3.4 and is what 619 conformance
        // fixtures verify.
        let result = compile(&opts(&["bg-red-500"]));
        assert!(
            result.output.css.contains("var(--tw-bg-opacity, 1)"),
            "expected `var(--tw-bg-opacity, 1)` in default-mode output, got: {}",
            result.output.css,
        );
    }

    #[test]
    fn color_resolver_walks_deeply_nested_palette() {
        // Tailwind's `flattenColorPalette` flattens arbitrary-depth
        // color objects: `colors.severity.log.critical` becomes the
        // valid candidate `bg-severity-log-critical`. Our resolver
        // must walk every level of nesting, not just one.
        let config = serde_json::json!({
            "theme": {
                "colors": {
                    "severity": {
                        "log": {
                            "critical": "var(--c-severity-log-critical)",
                            "verbose": "var(--c-severity-log-verbose)",
                        },
                        "security": {
                            "high": "var(--c-severity-security-high)",
                        },
                    },
                },
            },
        });
        let result = compile(&CompileOptions {
            candidates: vec![
                "bg-severity-log-critical".to_string(),
                "bg-severity-log-verbose".to_string(),
                "bg-severity-security-high".to_string(),
            ],
            config: Some(config),
            ..Default::default()
        });
        assert!(
            result.output.css.contains(".bg-severity-log-critical"),
            "expected `.bg-severity-log-critical` rule, got: {}",
            result.output.css,
        );
        assert!(
            result.output.css.contains("var(--c-severity-log-critical)"),
            "expected CSS variable from nested theme, got: {}",
            result.output.css,
        );
        assert!(
            result.output.css.contains(".bg-severity-security-high"),
            "expected 3-level deep `.bg-severity-security-high`, got: {}",
            result.output.css,
        );
    }

    #[test]
    fn arbitrary_shadow_with_var_color_replaces_var() {
        // Real-world bug from a workspace using `tw-shadow-[inset_0_0_0_1px_var(--c-border-subtle)]`.
        //
        // Tailwind v3 splits the box-shadow into `--tw-shadow` (the
        // literal value the user wrote) and `--tw-shadow-colored` (a
        // form where the color is replaced with
        // `var(--tw-shadow-color)` so `shadow-red-500` can override
        // the color via a later utility). When the color slot is a
        // `var(...)` call (e.g. `var(--c-border-subtle)`), Tailwind
        // REPLACES that var with `var(--tw-shadow-color)` in
        // `--tw-shadow-colored`. Our previous behavior appended
        // `var(--tw-shadow-color)` because the tokenizer didn't
        // recognize the `var()` as a color token — leaving the
        // user's color in `--tw-shadow-colored` (wrong) AND
        // appending the shadow-color var alongside it (also wrong).
        let result = compile(&opts(&["shadow-[inset_0_0_0_1px_var(--c-border-subtle)]"]));
        let css = &result.output.css;
        assert!(
            css.contains("--tw-shadow-colored: inset 0 0 0 1px var(--tw-shadow-color)"),
            "expected the var() to be REPLACED with var(--tw-shadow-color) in --tw-shadow-colored, got: {css}",
        );
        // --tw-shadow keeps the literal value the user wrote (the
        // var() reference is what they want to render by default).
        assert!(
            css.contains("--tw-shadow: inset 0 0 0 1px var(--c-border-subtle)"),
            "expected --tw-shadow to keep the literal var(), got: {css}",
        );
    }

    #[test]
    fn math_comma_space_default_adds_space() {
        // Default (Tailwind v3.4+) — comma in a math function value
        // gets a trailing space: `min(420px, 50vh)`.
        let result = compile(&opts(&["w-[min(420px,50vh)]"]));
        assert!(
            result.output.css.contains("min(420px, 50vh)"),
            "expected `min(420px, 50vh)` in default output, got: {}",
            result.output.css,
        );
    }

    #[test]
    fn math_comma_space_disabled_in_v33_compat_mode() {
        // `features.compat.tailwind_version = V33` keeps math-fn
        // commas tight (`min(420px,50vh)`), matching Tailwind v3.3.x
        // byte-for-byte.
        let result = compile(&CompileOptions {
            candidates: vec!["w-[min(420px,50vh)]".to_string()],
            features: FeatureFlags {
                compat: galeforce_core::CompatFlags {
                    tailwind_version: galeforce_core::TailwindVersion::V33,
                },
                ..Default::default()
            },
            ..Default::default()
        });
        assert!(
            result.output.css.contains("min(420px,50vh)"),
            "expected tight `min(420px,50vh)`, got: {}",
            result.output.css,
        );
        assert!(
            !result.output.css.contains("min(420px, 50vh)"),
            "v3.3 compat must NOT emit space after comma, got: {}",
            result.output.css,
        );
    }

    #[test]
    fn comma_escape_numeric_by_default() {
        // Default (Tailwind v3.4+) — comma in an arbitrary value
        // becomes `\2c ` in the selector.
        let result = compile(&opts(&["top-[var(--app-top,0)]"]));
        assert!(
            result.output.css.contains("\\2c "),
            "expected `\\2c ` in default output, got: {}",
            result.output.css,
        );
        assert!(
            !result
                .output
                .css
                .contains(".top-\\[var\\(--app-top\\,0\\)\\]"),
            "default mode must NOT emit literal `\\,` form, got: {}",
            result.output.css,
        );
    }

    #[test]
    fn comma_escape_numeric_in_v33_compat_mode() {
        // Both Tailwind v3.3 and v3.4 escape commas in arbitrary-value
        // selectors with the numeric `\2c ` form (via cssesc). v3.3
        // compat mode must NOT regress to literal `\,` — the live
        // tailwindcss@3.3.2 output uses `\2c ` too.
        let result = compile(&CompileOptions {
            candidates: vec!["top-[var(--app-top,0)]".to_string()],
            features: FeatureFlags {
                compat: galeforce_core::CompatFlags {
                    tailwind_version: galeforce_core::TailwindVersion::V33,
                },
                ..Default::default()
            },
            ..Default::default()
        });
        assert!(
            result.output.css.contains("\\2c "),
            "expected numeric `\\2c ` form in v3.3 mode, got: {}",
            result.output.css,
        );
        assert!(
            !result
                .output
                .css
                .contains(".top-\\[var\\(--app-top\\,0\\)\\]"),
            "v3.3 mode must NOT emit literal `\\,` form, got: {}",
            result.output.css,
        );
    }

    #[test]
    fn color_emits_var_opacity_without_fallback_in_v33_compat_mode() {
        // `features.compat.tailwind_version = V33` matches Tailwind
        // v3.3.x — no `, 1` fallback in the `var()` call. The
        // `--tw-bg-opacity: 1` declaration is always emitted
        // alongside, so runtime behavior is unchanged.
        let result = compile(&CompileOptions {
            candidates: vec!["bg-red-500".to_string()],
            features: FeatureFlags {
                compat: galeforce_core::CompatFlags {
                    tailwind_version: galeforce_core::TailwindVersion::V33,
                },
                ..Default::default()
            },
            ..Default::default()
        });
        assert!(
            result.output.css.contains("var(--tw-bg-opacity)"),
            "expected legacy `var(--tw-bg-opacity)` (no fallback), got: {}",
            result.output.css,
        );
        assert!(
            !result.output.css.contains("var(--tw-bg-opacity, 1)"),
            "legacy mode should NOT contain `, 1` fallback, got: {}",
            result.output.css,
        );
        // The `--tw-bg-opacity: 1` declaration must still be there so
        // runtime behavior matches.
        assert!(
            result.output.css.contains("--tw-bg-opacity: 1"),
            "legacy mode must still set --tw-bg-opacity: 1 alongside, got: {}",
            result.output.css,
        );
    }

    #[test]
    fn backdrop_blur_skips_webkit_prefix_in_v33_compat_mode() {
        // Tailwind v3.4 added `-webkit-backdrop-filter` alongside the
        // unprefixed `backdrop-filter` for Safari compat (autoprefixer
        // historically did this — v3.4 baked it into the plugin). v3.3
        // emits only the unprefixed form. v3.3 compat must mirror that
        // exactly so byte-identical diffs against a `tailwindcss@3.3.x`
        // baseline don't regress.
        let result = compile(&CompileOptions {
            candidates: vec!["backdrop-blur-sm".to_string()],
            features: FeatureFlags {
                compat: galeforce_core::CompatFlags {
                    tailwind_version: galeforce_core::TailwindVersion::V33,
                },
                ..Default::default()
            },
            ..Default::default()
        });
        assert!(
            result.output.css.contains("backdrop-filter:"),
            "expected unprefixed `backdrop-filter:` in v3.3 mode, got: {}",
            result.output.css,
        );
        assert!(
            !result.output.css.contains("-webkit-backdrop-filter"),
            "v3.3 mode must NOT emit `-webkit-backdrop-filter`, got: {}",
            result.output.css,
        );
    }

    #[test]
    fn backdrop_blur_keeps_webkit_prefix_in_v34_default() {
        // Conformance is gated by v3.4, so the default path must still
        // emit BOTH the vendor-prefixed and unprefixed forms.
        let result = compile(&opts(&["backdrop-blur-sm"]));
        assert!(
            result.output.css.contains("-webkit-backdrop-filter"),
            "expected `-webkit-backdrop-filter` in default v3.4 mode, got: {}",
            result.output.css,
        );
        assert!(
            result.output.css.contains("backdrop-filter:"),
            "expected unprefixed `backdrop-filter:` alongside in default mode, got: {}",
            result.output.css,
        );
    }

    #[test]
    fn static_backdrop_filter_skips_webkit_prefix_in_v33_compat_mode() {
        // The static `backdrop-filter` / `backdrop-filter-none`
        // utilities follow the same rule.
        let result = compile(&CompileOptions {
            candidates: vec!["backdrop-filter".to_string()],
            features: FeatureFlags {
                compat: galeforce_core::CompatFlags {
                    tailwind_version: galeforce_core::TailwindVersion::V33,
                },
                ..Default::default()
            },
            ..Default::default()
        });
        assert!(
            !result.output.css.contains("-webkit-backdrop-filter"),
            "v3.3 mode static `backdrop-filter` must NOT emit webkit prefix, got: {}",
            result.output.css,
        );
    }

    #[test]
    fn prefixed_bare_class_emits_literal_comma() {
        // Tailwind v3's `prefixSelector` runs postcss-selector-parser
        // on the already-escaped class, which re-escapes commas as
        // literal `\,` and skips the `escapeCommas` post-pass. So
        // `tw-bg-[rgba(0,0,0)]` becomes `.tw-bg-\[rgba\(0\,0\,0\)\]`,
        // NOT `\2c `. This is a Tailwind v3 quirk (present in both
        // 3.3 and 3.4) — we mirror it for byte-identical output.
        let result = compile(&CompileOptions {
            candidates: vec!["tw-bg-[rgba(0,0,0,0.3)]".to_string()],
            config: Some(serde_json::json!({ "prefix": "tw-" })),
            ..Default::default()
        });
        assert!(
            result
                .output
                .css
                .contains(".tw-bg-\\[rgba\\(0\\,0\\,0\\,0\\.3\\)\\]"),
            "expected prefixed-bare-class literal `\\,` form, got: {}",
            result.output.css,
        );
    }

    #[test]
    fn prefixed_class_with_variant_keeps_numeric_comma() {
        // With a variant like `hover:` or `dark:`, the candidate is
        // escaped via `escapeClassName` (which DOES run
        // `escapeCommas`), so commas stay as `\2c ` even when the
        // class is prefixed.
        let result = compile(&CompileOptions {
            candidates: vec!["hover:tw-bg-[rgba(0,0,0,0.3)]".to_string()],
            config: Some(serde_json::json!({ "prefix": "tw-" })),
            ..Default::default()
        });
        assert!(
            result.output.css.contains("\\2c "),
            "expected `\\2c ` with variant + prefix, got: {}",
            result.output.css,
        );
    }

    #[test]
    fn unprefixed_arbitrary_property_keeps_numeric_comma() {
        // Arbitrary properties `[prop:val]` are not prefixed (they
        // have no class name to prepend the prefix to), so they go
        // through the normal escape pipeline that runs
        // `escapeCommas` — `\2c ` form.
        let result = compile(&CompileOptions {
            candidates: vec!["[grid-template-columns:minmax(0,1fr)]".to_string()],
            config: Some(serde_json::json!({ "prefix": "tw-" })),
            ..Default::default()
        });
        assert!(
            result.output.css.contains("\\2c "),
            "expected `\\2c ` for arbitrary property even with prefix configured, got: {}",
            result.output.css,
        );
    }

    #[test]
    fn unprefixed_config_uses_numeric_comma() {
        // No `prefix` configured — `prefixSelector` short-circuits
        // and the class keeps its `escapeCommas`-applied `\2c `
        // form. (This is the default behavior the conformance
        // harness exercises against the oracle.)
        let result = compile(&opts(&["bg-[rgba(0,0,0,0.3)]"]));
        assert!(
            result.output.css.contains("\\2c "),
            "expected `\\2c ` with no prefix configured, got: {}",
            result.output.css,
        );
        assert!(
            !result
                .output
                .css
                .contains(".bg-\\[rgba\\(0\\,0\\,0\\,0\\.3\\)\\]"),
            "no-prefix mode must NOT emit literal `\\,` form, got: {}",
            result.output.css,
        );
    }

    #[test]
    fn flex_compiles_to_display_flex() {
        let result = compile(&opts(&["flex"]));
        assert!(
            result.diagnostics.is_empty(),
            "diagnostics: {:?}",
            result.diagnostics
        );
        assert_eq!(result.rule_count, 1);
        assert!(
            result.output.css.contains(".flex"),
            "css: {}",
            result.output.css
        );
        assert!(
            result.output.css.contains("display: flex"),
            "css: {}",
            result.output.css
        );
    }

    #[test]
    fn block_and_hidden_each_emit_a_rule() {
        let result = compile(&opts(&["block", "hidden"]));
        assert!(
            result.diagnostics.is_empty(),
            "diagnostics: {:?}",
            result.diagnostics
        );
        assert_eq!(result.rule_count, 2);
        assert!(result.output.css.contains(".block"));
        assert!(result.output.css.contains(".hidden"));
        assert!(result.output.css.contains("display: none"));
    }

    #[test]
    fn unknown_utility_yields_diagnostic() {
        let result = compile(&opts(&["definitely-not-real"]));
        assert_eq!(result.rule_count, 0);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].code, "unknown-utility");
    }

    #[test]
    fn hover_variant_emits_pseudo_class_selector() {
        let result = compile(&opts(&["hover:flex"]));
        assert!(
            result.diagnostics.is_empty(),
            "diagnostics: {:?}",
            result.diagnostics
        );
        assert_eq!(result.rule_count, 1);
        assert!(
            result.output.css.contains(".hover\\:flex:hover"),
            "css: {}",
            result.output.css
        );
    }

    #[test]
    fn responsive_md_wraps_in_min_width_media() {
        let result = compile(&opts(&["md:flex"]));
        assert!(
            result.diagnostics.is_empty(),
            "diagnostics: {:?}",
            result.diagnostics
        );
        assert!(
            result.output.css.contains("@media (min-width: 768px)"),
            "css: {}",
            result.output.css
        );
        assert!(result.output.css.contains(".md\\:flex"));
    }

    #[test]
    fn marker_emits_two_rules_per_candidate() {
        let result = compile(&opts(&["marker:flex"]));
        assert!(
            result.diagnostics.is_empty(),
            "diagnostics: {:?}",
            result.diagnostics
        );
        // `marker` produces both `& *::marker` and `&::marker`.
        assert_eq!(result.rule_count, 2);
    }

    #[test]
    fn before_prepends_content_var() {
        let result = compile(&opts(&["before:flex"]));
        assert!(
            result.diagnostics.is_empty(),
            "diagnostics: {:?}",
            result.diagnostics
        );
        assert!(
            result.output.css.contains("content: var(--tw-content)"),
            "css: {}",
            result.output.css
        );
        assert!(result.output.css.contains("::before"));
    }

    #[test]
    fn group_hover_emits_descendant_selector() {
        let result = compile(&opts(&["group-hover:flex"]));
        assert!(
            result.diagnostics.is_empty(),
            "diagnostics: {:?}",
            result.diagnostics
        );
        assert!(
            result
                .output
                .css
                .contains(".group:hover .group-hover\\:flex"),
            "css: {}",
            result.output.css
        );
    }

    #[test]
    fn unknown_variant_yields_unsupported_diagnostic() {
        let result = compile(&opts(&["definitely-not-a-variant:flex"]));
        assert_eq!(result.rule_count, 0);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].code, "unsupported-candidate");
    }

    #[test]
    fn important_static_emits_bang_important() {
        let result = compile(&opts(&["!flex"]));
        assert!(
            result.diagnostics.is_empty(),
            "diagnostics: {:?}",
            result.diagnostics
        );
        assert!(
            result.output.css.contains("display: flex !important"),
            "css: {}",
            result.output.css
        );
        assert!(
            result.output.css.contains("\\!flex"),
            "css: {}",
            result.output.css
        );
    }

    #[test]
    fn sr_only_emits_nine_declarations() {
        let result = compile(&opts(&["sr-only"]));
        assert!(
            result.diagnostics.is_empty(),
            "diagnostics: {:?}",
            result.diagnostics
        );
        assert!(result.output.css.contains("clip: rect(0, 0, 0, 0)"));
        assert!(result.output.css.contains("white-space: nowrap"));
        assert!(result.output.css.contains("border-width: 0"));
    }

    /// Helper: extract emitted selectors in source order from CSS output.
    fn selectors_in_order(css: &str) -> Vec<String> {
        let mut out = Vec::new();
        for line in css.lines() {
            let trimmed = line.trim_start();
            if let Some(brace_pos) = trimmed.find('{') {
                let sel = trimmed[..brace_pos].trim();
                if !sel.is_empty() && !sel.starts_with('@') {
                    out.push(sel.to_string());
                }
            }
        }
        out
    }

    #[test]
    fn sort_within_padding_family_is_numeric() {
        // Input order is shuffled; output must be numerically ascending
        // so `class="p-2 p-4"` resolves to padding: 1rem under standard
        // last-rule-wins cascade semantics.
        let r = compile(&opts(&["p-4", "p-1", "p-8", "p-2"]));
        let sels = selectors_in_order(&r.output.css);
        let p_only: Vec<&str> = sels
            .iter()
            .filter(|s| s.starts_with(".p-"))
            .map(|s| s.as_str())
            .collect();
        assert_eq!(
            p_only,
            vec![".p-1", ".p-2", ".p-4", ".p-8"],
            "got: {sels:?}"
        );
    }

    #[test]
    fn sort_responsive_rules_come_after_unprefixed() {
        let r = compile(&opts(&["md:p-4", "p-4", "sm:p-4", "lg:p-4"]));
        let sels = selectors_in_order(&r.output.css);
        // Exact selector at_rules render across multiple lines, so
        // we just check the order positions.
        let p4_pos = sels.iter().position(|s| s == ".p-4").unwrap();
        let sm_pos = sels.iter().position(|s| s.contains("sm")).unwrap();
        let md_pos = sels.iter().position(|s| s.contains("md")).unwrap();
        let lg_pos = sels.iter().position(|s| s.contains("lg")).unwrap();
        assert!(
            p4_pos < sm_pos && sm_pos < md_pos && md_pos < lg_pos,
            "expected unprefixed -> sm -> md -> lg, got: {sels:?}"
        );
    }

    #[test]
    fn sort_pseudo_class_rules_come_between_unprefixed_and_responsive() {
        let r = compile(&opts(&["md:flex", "flex", "hover:flex"]));
        let sels = selectors_in_order(&r.output.css);
        let plain = sels.iter().position(|s| s == ".flex").unwrap();
        let hover = sels
            .iter()
            .position(|s| s.contains("hover") && s.contains(":hover"))
            .unwrap();
        let md = sels.iter().position(|s| s.contains("md")).unwrap();
        assert!(
            plain < hover && hover < md,
            "expected plain -> hover -> md, got: {sels:?}"
        );
    }

    #[test]
    fn display_hidden_emits_after_other_display_utilities() {
        // Tailwind v3's `display` corePlugin emits utilities in this
        // exact order: block, inline-block, inline, flex, inline-flex,
        // table, ..., flow-root, grid, inline-grid, contents, list-item,
        // hidden. So `class="flex hidden"` resolves to `display: none`
        // under standard last-rule-wins cascade — `hidden` wins.
        //
        // Our previous sort used `prefix_hash` as the within-plugin
        // tiebreaker, which is essentially random. The result placed
        // `.tw-hidden` BEFORE `.tw-flex` / `.tw-block`, so combining
        // them lost the `hidden` to whichever display class came
        // later alphabetically.
        let r = compile(&opts(&["flex", "block", "hidden", "inline-flex", "grid"]));
        let sels = selectors_in_order(&r.output.css);
        let positions: Vec<(String, usize)> = ["block", "flex", "inline-flex", "grid", "hidden"]
            .iter()
            .map(|name| {
                let dot = format!(".{name}");
                let pos = sels
                    .iter()
                    .position(|s| s == &dot)
                    .unwrap_or_else(|| panic!("missing {dot} in {sels:?}"));
                (dot, pos)
            })
            .collect();
        let hidden_pos = positions.last().unwrap().1;
        for (sel, pos) in &positions[..positions.len() - 1] {
            assert!(
                *pos < hidden_pos,
                "{sel} must come before .hidden, got positions: {positions:?}",
            );
        }
    }

    #[test]
    fn sort_negative_margin_orders_before_positive() {
        // Tailwind v3 emits utilities in lexicographic class-name
        // order via `matchUtilities`. Negatives sort BEFORE positives
        // because `'-' < 'm'`, AND within each polarity rules sort
        // alphabetically — `-mt-1, -mt-2, -mt-4` (since `1 < 2 < 4`
        // as chars). Probed against tailwindcss@3.4.19 oracle for
        // `class="mt-1 -mt-4"` → emission order
        // `-mt-1, -mt-4, mt-1, mt-4`.
        let r = compile(&opts(&["mt-1", "-mt-4", "-mt-1", "mt-4"]));
        let sels = selectors_in_order(&r.output.css);
        let mts: Vec<&str> = sels
            .iter()
            .filter(|s| s.contains("mt"))
            .map(|s| s.as_str())
            .collect();
        assert_eq!(
            mts,
            vec![".-mt-1", ".-mt-4", ".mt-1", ".mt-4"],
            "got: {sels:?}"
        );
    }

    #[test]
    fn user_plugin_utility_emits_after_core_with_shared_prefix() {
        // Real-world repro: a user plugin registers `.font-section-title`
        // via `addUtilities({ '.font-section-title': { font: '<shorthand>' } })`.
        // When the same template applies both `font-section-title` (custom)
        // and `font-bold` (core fontWeight), upstream Tailwind v3 emits
        // core utilities FIRST and user-plugin utilities LAST so the
        // user-plugin `font` shorthand can override `font-weight: 700`.
        //
        // Previously, galeforce routed the user candidate through the
        // shared `font-` value-utility prefix, tagging it with the
        // `fontWeight` plugin_order — so `font-section-title` and
        // `font-bold` clustered together with `font-section-title`
        // emitting BEFORE `font-bold`, inverting the cascade.
        //
        // The fix: user-plugin candidates carry a plugin_order sentinel
        // that places them AFTER every core plugin.
        let config = serde_json::json!({
            "__pluginOutput": {
                "utilities": [
                    {
                        "selector": ".font-section-title",
                        "declarations": [
                            { "property": "font", "value": "normal normal 600 16px/1.5 sans-serif" }
                        ],
                    }
                ],
                "components": [],
                "base": [],
                "variants": [],
            }
        });
        let r = compile(&CompileOptions {
            candidates: vec!["font-section-title".to_string(), "font-bold".to_string()],
            config: Some(config),
            ..Default::default()
        });
        let sels = selectors_in_order(&r.output.css);
        let font_sels: Vec<&str> = sels
            .iter()
            .filter(|s| s.contains("font-section-title") || s.contains("font-bold"))
            .map(|s| s.as_str())
            .collect();
        assert_eq!(
            font_sels,
            vec![".font-bold", ".font-section-title"],
            "core `font-bold` must emit before user-plugin `font-section-title`. got: {sels:?}"
        );
    }
}
