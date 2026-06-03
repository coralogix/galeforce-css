use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildMode {
    #[default]
    Development,
    Production,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    Base,
    Components,
    #[default]
    Utilities,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum DarkMode {
    Media,
    #[default]
    Class,
    /// Custom selector — e.g. `.theme-dark` or `[data-theme="dark"]`.
    Selector(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum ImportantMode {
    #[default]
    Off,
    On,
    /// Important is scoped under a selector — e.g. `#app`.
    Selector(String),
}

/// Which Tailwind v3 patch line GaleforceCSS should match byte-for-byte.
/// Defaults to v3.4 (our pinned conformance target). Selecting v3.3
/// enables a bundle of cosmetic output tweaks so the swap into a
/// project still pinned at `tailwindcss@3.3.x` produces an identical
/// CSS bundle:
///
/// - Color-utility `var(--tw-*-opacity)` calls emit WITHOUT the
///   `, 1` fallback that v3.4 added.
/// - Commas inside arbitrary-value selectors (e.g.
///   `.tw-h-[min(420px,50vh)]`) escape as `\,` instead of `\2c `.
/// - Commas inside math-function values stay tight
///   (`min(420px,50vh)`) instead of getting a trailing space
///   (`min(420px, 50vh)`).
///
/// Every one of these is byte-only — runtime CSS behavior is
/// identical. The switch exists so projects can swap GaleforceCSS in
/// without their built CSS bundle changing visibly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TailwindVersion {
    /// `tailwindcss@3.3.x`. Pre-`escapeCommas`, pre-opacity-fallback,
    /// pre-math-comma-space. Enable when your project is still on
    /// 3.3.x and the swap should produce a byte-identical bundle.
    #[serde(rename = "3.3")]
    V33,
    /// `tailwindcss@3.4.x`. The default; matches GaleforceCSS's pinned
    /// conformance oracle (`tailwindcss@3.4.19`).
    #[default]
    #[serde(rename = "3.4")]
    V34,
}

/// Per-call compatibility flags for matching legacy Tailwind output
/// byte-for-byte. Grouped under a single field on `FeatureFlags` so
/// future version-compat knobs can land here without proliferating
/// top-level options. Default matches the pinned target Tailwind
/// v3.4.19.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CompatFlags {
    /// Which Tailwind v3 patch line to match byte-for-byte. See
    /// [`TailwindVersion`] for the per-version behavior list.
    pub tailwind_version: TailwindVersion,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FeatureFlags {
    pub minify: bool,
    pub source_maps: bool,
    pub debug: bool,
    /// Browser targets as a structured map of minimum-supported versions,
    /// e.g. `{"chrome":"95","firefox":"94","safari":"15"}`. When set,
    /// Lightning CSS lowers and prefixes modern CSS features for the
    /// listed browsers. When `None`, no prefixing or lowering is applied
    /// — output is the modern CSS Tailwind emits unchanged.
    ///
    /// JS-side callers can resolve a browserslist query string with
    /// `browserslist` + `lightningcss-napi`'s `browserslistToTargets`,
    /// or hand-roll the map.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub targets: Option<serde_json::Value>,

    /// Version-compatibility flags. Default matches Tailwind v3.4.19;
    /// flip individual flags here to match older v3 patch versions
    /// byte-for-byte.
    #[serde(default)]
    pub compat: CompatFlags,

    /// Enable nested CSS expansion (a Rust port of
    /// `tailwindcss/nesting` + `postcss-nested@6.2.0`). When true,
    /// the compiler flattens `.foo { .bar { ... } }` and `.btn {
    /// &:hover { ... } }` shapes before the directive processor
    /// runs, so existing `@apply`/`@tailwind`/`@screen` machinery
    /// sees flat CSS. Default false — projects that don't author
    /// nested CSS pay zero cost.
    #[serde(default)]
    pub nesting: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CompileOptions {
    pub mode: BuildMode,
    pub features: FeatureFlags,
    /// Path to user CSS input (the file containing `@tailwind` etc.)
    pub input_css_path: Option<String>,
    /// Raw CSS input. If `None`, defaults to `@tailwind base; @tailwind components; @tailwind utilities;`
    pub input_css: Option<String>,
    /// Normalized config JSON produced by the Node loader.
    pub config: Option<serde_json::Value>,
    /// Pre-extracted candidate set. If empty, scanner output is used.
    pub candidates: Vec<String>,
    /// Content roots to walk + scan for candidates. The compiler dedupes
    /// the resulting tokens into `candidates`. Used by the Vite plugin
    /// and `galeforcecss build` so callers don't have to extract first.
    pub content: Vec<String>,
}
