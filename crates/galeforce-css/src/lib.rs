//! CSS AST, selector escaping, and stylesheet emitter.
//!
//! This crate is the data layer between the compiler (which produces
//! `Rule`s) and the textual CSS the rest of the pipeline writes out. It
//! deliberately does NOT parse CSS input — that's the directive-processor's
//! job (see `todo.md` § 17).

#![allow(clippy::doc_markdown)]

mod emit;
mod escape;
mod nesting;

pub use emit::{emit_stylesheet, emit_stylesheet_into, EmitOptions};
pub use escape::{escape_class_name, escape_class_name_with, CommaEscapeStyle};
pub use nesting::{expand_nesting, NestingOptions};

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declaration {
    pub property: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub important: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub selectors: SmallVec<[String; 1]>,
    pub declarations: Vec<Declaration>,
    /// At-rule context this rule sits inside, outermost-first. Each entry
    /// is the full at-rule header without its body, e.g.
    /// `"@media (min-width: 768px)"`. Empty for top-level rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub at_rules: Vec<String>,
    /// Whether this rule should pick up the global `important: true`
    /// / `important: '<sel>'` config when it runs. Defaults to true.
    /// Plugin output uses this to opt out: `addComponents` defaults
    /// to `respectImportant: false`, and `addUtilities({...}, {
    /// respectImportant: false })` opts in. Mirrors upstream's
    /// per-rule `respectImportant` flag.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub respect_important: bool,
    /// Cascade-variable defaults groups this rule depends on. Each
    /// entry corresponds to an `@defaults <name>;` marker that the
    /// emitter renders at the start of the rule body. The post-pass
    /// `resolve_defaults_at_rules_pass` collects markers across the
    /// whole sheet, groups by id, and emits one shared defaults
    /// rule per group at the top — mirroring upstream's
    /// `resolveDefaultsAtRules.js`. Empty for rules that don't
    /// depend on any cascade-var defaults.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub defaults_groups: Vec<String>,
}

impl Default for Rule {
    fn default() -> Self {
        Self {
            selectors: SmallVec::new(),
            declarations: Vec::new(),
            at_rules: Vec::new(),
            respect_important: true,
            defaults_groups: Vec::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
}
