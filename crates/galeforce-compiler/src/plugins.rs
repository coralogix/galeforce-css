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

//! User-plugin output handling.
//!
//! Tailwind v3 plugins are JS functions that call helper APIs
//! (`addUtilities`, `addComponents`, `addBase`, `addVariant`,
//! `matchUtilities`). We don't run plugins in Rust — `@coralogix/galeforcecss-
//! config-loader` invokes them in JS against a recording context and
//! captures the resulting structured data. The Rust compiler reads
//! that data from `config.__pluginOutput` and merges it with the
//! built-in utility/variant tables.
//!
//! Capture format (mirrors `packages/galeforcecss-config-loader/src/
//! plugin-runner.ts`):
//!
//! ```json
//! {
//!   "utilities": [{ "selector": ".foo", "declarations": [...], "atRules": [...] }],
//!   "components": [...],
//!   "base": [...],
//!   "variants": [{ "name": "hocus", "selectorFormats": ["&:hover", "&:focus"], "atRule": null }]
//! }
//! ```

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginOutput {
    pub utilities: Vec<PluginRule>,
    pub components: Vec<PluginRule>,
    pub base: Vec<PluginRule>,
    pub variants: Vec<PluginVariant>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginRule {
    pub selector: String,
    pub declarations: Vec<PluginDecl>,
    #[serde(default, rename = "atRules")]
    pub at_rules: Vec<String>,
    /// Plugin's `respectImportant` option from `addUtilities` /
    /// `addComponents`. Defaults to true (utilities); the
    /// JS-side runner sets it to false when the plugin opts out
    /// (`addUtilities({...}, { respectImportant: false })`) or
    /// when shipping `addComponents` rules (which default to
    /// false per upstream). Used by `apply_important_mode` to
    /// skip rules that should never pick up `important: true` /
    /// `important: '<sel>'` config.
    #[serde(default = "default_respect_important", rename = "respectImportant")]
    pub respect_important: bool,
    /// Group key set by the JS runner's `materializeMatch` so the
    /// Rust compiler can pull in companion rules emitted alongside
    /// a candidate's main class. The classic case is `matchUtilities`
    /// returning `{ '@keyframes <value>': {...}, animation: ... }`
    /// — the keyframes block has no class selector of its own, so
    /// `primary_class()` returns `None` and the per-class lookup
    /// would skip it. Tag both rules with the same `groupClass`
    /// (the candidate's class name) and the lookup includes both.
    #[serde(default, rename = "groupClass")]
    pub group_class: Option<String>,
}

fn default_respect_important() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginDecl {
    pub property: String,
    pub value: String,
    #[serde(default)]
    pub important: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginVariant {
    pub name: String,
    #[serde(default, rename = "selectorFormats")]
    pub selector_formats: Vec<String>,
    #[serde(default, rename = "atRule")]
    pub at_rule: Option<String>,
    /// Captured `decl.value` transform from a function-form
    /// addVariant that mutates declarations. Stored as a template
    /// where `{}` marks the original value (e.g. `calc(0 + {})`).
    /// Applied to every emitted decl in rules using this variant.
    /// `None` when the function didn't touch decl values.
    #[serde(default, rename = "declValueTemplate")]
    pub decl_value_template: Option<String>,
}

/// Read `config.__pluginOutput` if present. Returns the default
/// (empty) form if the field is missing or malformed — plugin output
/// is optional, the absence is the common case for projects without
/// plugins.
pub fn read_plugin_output(config: Option<&Value>) -> PluginOutput {
    let Some(node) = config.and_then(|c| c.get("__pluginOutput")) else {
        return PluginOutput::default();
    };
    let mut output: PluginOutput = serde_json::from_value(node.clone()).unwrap_or_default();
    // Plugin authors sometimes embed `!important` directly in the
    // declaration value (`{ transform: 'rotate(90deg) !important' }`)
    // rather than relying on Tailwind's `important` config. Lift
    // those into the structured `important: bool` flag so a later
    // `important: true` pass doesn't double-stack them and the
    // emitter renders a single `!important` per decl.
    for rule in output
        .utilities
        .iter_mut()
        .chain(output.components.iter_mut())
        .chain(output.base.iter_mut())
    {
        for decl in &mut rule.declarations {
            let trimmed_end = decl.value.trim_end();
            if let Some(rest) = trimmed_end.strip_suffix("!important") {
                let stripped = rest.trim_end();
                decl.value = stripped.to_string();
                decl.important = true;
            }
        }
    }
    output
}

impl PluginRule {
    /// Extract the bare class name from `selector`. Plugin rules
    /// typically have selectors like `.scrollbar-none` — we want
    /// `scrollbar-none` so we can match against the candidate's
    /// rendered class form. Returns `None` for selectors that don't
    /// start with `.<ident>` (e.g. `html`, `*`, `body > *`).
    ///
    /// Bracketed arbitrary-value runs (`.tab-[3.5]`,
    /// `.bg-[#0b14374d]`) are preserved as part of the class name
    /// so candidate-root lookups for arbitrary forms match. The
    /// scan tracks bracket nesting and consumes everything up to
    /// the matching `]`.
    pub fn primary_class(&self) -> Option<&str> {
        let s = self.selector.trim();
        // Plugins sometimes wrap the class in `:where(.btn)` /
        // `:is(.btn)` / `:has(.btn)` to drop specificity. Peel off
        // a single `:where`/`:is`/`:has` wrapper and try again
        // against the inner first selector. Leading `&` (nesting
        // marker that some plugins emit verbatim) gets skipped too.
        let s = s.strip_prefix('&').map(|s| s.trim_start()).unwrap_or(s);
        let unwrapped = strip_pseudo_wrapper(s).unwrap_or(s);
        let rest = unwrapped.strip_prefix('.')?;
        let bytes = rest.as_bytes();
        let mut i = 0;
        let mut depth = 0i32;
        while i < bytes.len() {
            let b = bytes[i];
            if b == b'\\' && i + 1 < bytes.len() {
                // Escaped character — consume both bytes.
                i += 2;
                continue;
            }
            if b == b'[' {
                depth += 1;
                i += 1;
                continue;
            }
            if b == b']' {
                if depth > 0 {
                    depth -= 1;
                    i += 1;
                    continue;
                }
                break;
            }
            if depth > 0 {
                // Inside brackets — anything goes (including spaces,
                // parens, commas inside arbitrary values).
                i += 1;
                continue;
            }
            if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' {
                i += 1;
                continue;
            }
            // Accept a single `/` as part of the class name when it
            // sits between two value-shaped runs at depth 0. Plugin
            // values can contain `/` (e.g. `test-1/foo`,
            // `test-[8]/[9]`) and the candidate parser splits them
            // out as a modifier — to compare apples to apples
            // we keep the slash in the primary class.
            if b == b'/' {
                i += 1;
                continue;
            }
            break;
        }
        if i == 0 {
            return None;
        }
        Some(&rest[..i])
    }
}

impl PluginRule {
    /// Collect every `.<class>` token referenced in the selector,
    /// including those inside `:is()`/`:where()`/`:has()`/`:not()`
    /// wrappers. Used by the candidate-lookup pass so a rule like
    /// `.outer:is(.w-full)` matches both `outer` AND `w-full`
    /// candidates — mirrors upstream's "any class in the selector
    /// can trigger this rule" behavior in `setupContextUtils.js`.
    pub fn referenced_classes(&self) -> Vec<String> {
        let s = self.selector.as_str();
        let bytes = s.as_bytes();
        let mut out: Vec<String> = Vec::new();
        let mut i = 0;
        // Track `:not(...)` depth so the classes inside it don't
        // count as candidate triggers. Mirrors upstream's
        // `setupContextUtils.js` candidate-extraction which skips
        // anything inside a `:not()` (those are exclusion classes,
        // not the rule's targets).
        let mut not_depth = 0i32;
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
                b':' => {
                    // Detect `:not(` opening — track its depth so we
                    // can skip classes inside.
                    if i + 5 <= bytes.len() && &bytes[i..i + 5] == b":not(" {
                        not_depth += 1;
                        i += 5;
                        continue;
                    }
                    i += 1;
                }
                b'(' if not_depth > 0 => {
                    not_depth += 1;
                    i += 1;
                }
                b')' if not_depth > 0 => {
                    not_depth -= 1;
                    i += 1;
                }
                b'.' if not_depth > 0 => {
                    // Skip the class inside :not(). Walk past its
                    // identifier so we don't accidentally re-enter
                    // the class branch on a stray `.`.
                    i += 1;
                    while i < bytes.len() {
                        let b = bytes[i];
                        if b == b'\\' && i + 1 < bytes.len() {
                            i += 2;
                            continue;
                        }
                        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'/' {
                            i += 1;
                            continue;
                        }
                        break;
                    }
                }
                b'.' => {
                    i += 1;
                    let start = i;
                    let mut depth = 0i32;
                    while i < bytes.len() {
                        let b = bytes[i];
                        if b == b'\\' && i + 1 < bytes.len() {
                            i += 2;
                            continue;
                        }
                        if b == b'[' {
                            depth += 1;
                            i += 1;
                            continue;
                        }
                        if b == b']' {
                            if depth > 0 {
                                depth -= 1;
                                i += 1;
                                continue;
                            }
                            break;
                        }
                        if depth > 0 {
                            i += 1;
                            continue;
                        }
                        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'/' {
                            i += 1;
                            continue;
                        }
                        break;
                    }
                    if i > start {
                        out.push(s[start..i].to_string());
                    }
                }
                _ => i += 1,
            }
        }
        out
    }
}

/// Try to peel `:where(<inner>)` / `:is(<inner>)` / `:has(<inner>)`
/// from `s`. Returns the inner selector (or its first comma-list
/// entry) when the WHOLE selector is the wrapper. Returns `None`
/// when `s` isn't shaped that way.
fn strip_pseudo_wrapper(s: &str) -> Option<&str> {
    for prefix in [":where(", ":is(", ":has("] {
        if let Some(rest) = s.strip_prefix(prefix) {
            // Find the matching close paren at depth 0.
            let bytes = rest.as_bytes();
            let mut depth = 1i32;
            let mut i = 0;
            while i < bytes.len() {
                match bytes[i] {
                    b'\\' if i + 1 < bytes.len() => i += 2,
                    b'(' => {
                        depth += 1;
                        i += 1;
                    }
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            // Wrapper covers the whole input only
                            // when nothing trails the close paren.
                            if i + 1 == bytes.len() {
                                let inner = &rest[..i];
                                // Take the first comma-list entry
                                // — `:where(.a, .b)` returns `.a`
                                // for the lookup; the rest still
                                // emit when matched.
                                let first = inner.split(',').next().unwrap_or(inner).trim();
                                return Some(first);
                            }
                            return None;
                        }
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            return None;
        }
    }
    None
}
