//! Value-bearing utilities (theme-driven and arbitrary).
//!
//! Mirrors Tailwind 3.4.19's `createUtilityPlugin` factory at
//! `vendor/tailwindcss-v3/src/util/createUtilityPlugin.js`. A plugin
//! declares:
//!
//!   - a `theme_key` (e.g. `"margin"`, `"padding"`, `"spacing"`) — the
//!     dot-path GaleforceCSS reads from to find the value table.
//!   - one or more class prefixes that share a property list (e.g.
//!     `"m" -> ["margin"]`, `"mx" -> ["margin-left", "margin-right"]`).
//!   - whether the family supports negative values (the candidate parser
//!     captures `parsed.negative`; if the family allows it, we prepend
//!     `-` to the resolved value).
//!
//! Resolution flow:
//!
//!   1. The compiler tries `find_static(root)` first. If that hits, done.
//!   2. Otherwise it walks `VALUE_UTILITIES`. The first prefix that
//!      matches `root` returns the property list and the *value key*
//!      (the slice after `<prefix>-`). For bare `m`/`p` etc. the value
//!      key is `"DEFAULT"`.
//!   3. The value key is resolved via the theme: arbitrary `[<…>]` is
//!      a literal value; a bare key is looked up in
//!      `theme.<theme_key>`.
//!   4. If the value resolves and `negative` is set, prepend `-` (only
//!      for families flagged `supports_negative`).
//!   5. Each property in the list becomes one declaration in the
//!      emitted rule.

#![allow(clippy::doc_markdown)]

use galeforce_parser::ParsedCandidate;
use serde_json::Value;

use crate::colors::parse_hex;

/// True when `s` is a hex literal (`#RRGGBBAA` or `#RGBA`) carrying its
/// own alpha channel. Used by `resolve_color` to short-circuit the
/// `--tw-*-opacity` wrapping path: when the user wrote
/// `bg-[#0b14374d]`, the alpha is already encoded and Tailwind emits
/// the hex verbatim.
fn hex_has_embedded_alpha(s: &str) -> bool {
    let Some(body) = s.strip_prefix('#') else {
        return false;
    };
    matches!(body.len(), 4 | 8) && body.bytes().all(|b| b.is_ascii_hexdigit())
}
use crate::default_theme::{default_theme_ref, lookup_theme};

#[derive(Clone, Copy, Debug)]
pub struct ValueUtility {
    /// Class-name prefix, without the `-<value>` part. e.g. `"m"`,
    /// `"mx"`, `"mt"`, `"px"`.
    pub class_prefix: &'static str,
    /// CSS properties this utility writes. All get the same resolved
    /// value when `kind == Simple`. Ignored by `FontSize` and
    /// `FontFamily` which produce their own property/value pairs.
    pub properties: &'static [&'static str],
    /// Theme dot-path for the value table.
    pub theme_key: &'static str,
    /// Whether the candidate may carry a leading `-` (e.g. `-mt-4`).
    pub supports_negative: bool,
    /// Resolver shape. Most plugins are `Simple` — one resolved value
    /// distributed across `properties`. Typography needs a richer
    /// pattern because:
    ///   * `theme.fontSize.<key>` is a tuple `[size, options]` whose
    ///     options object carries `lineHeight` / `letterSpacing` /
    ///     `fontWeight`. The plugin emits *multiple* declarations.
    ///   * `theme.fontFamily.<key>` is an array of strings that join
    ///     into a comma-separated `font-family` value.
    pub kind: ResolverKind,
    /// Plugin name. Used by the `corePlugins` config filter to
    /// selectively disable utility families.
    pub plugin: &'static str,
    /// Tailwind's `filterDefault: true` option on `createUtilityPlugin`.
    /// When true, the bare-prefix candidate (`<class>` with no
    /// `-<value>` suffix, mapped to theme key `DEFAULT`) is REJECTED.
    /// `transitionDuration` and `transitionTimingFunction` use this in
    /// upstream so `.duration` / `.ease` aren't emitted even though
    /// their theme has a `DEFAULT` key. See
    /// `vendor/tailwindcss-v3/src/corePlugins.js`.
    pub filter_default: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolverKind {
    /// Each `property` in `properties` gets the resolved value.
    Simple,
    /// `text-<key>` — see `corePlugins.js` `fontSize` plugin. Resolves
    /// a tuple `[size, options]` (options may itself be a string for
    /// the legacy `[size, lineHeight]` shape) into one or more
    /// declarations: `font-size`, optional `line-height`,
    /// `letter-spacing`, `font-weight`. Honors a slash modifier as a
    /// `line-height` override (`text-sm/6`).
    FontSize,
    /// `font-<key>` family path — see `corePlugins.js` `fontFamily`
    /// plugin. Resolves a string-or-array theme value into
    /// `font-family: <value or comma-joined>`. Optional
    /// `fontFeatureSettings` / `fontVariationSettings` from the second
    /// tuple element.
    FontFamily,
    /// Color value plugins (`bg-`, `text-`, `border-`, `decoration-`,
    /// `fill-`, `stroke-`, …). The resolved theme value is either a
    /// keyword (`transparent`, `currentColor`, `inherit`) or a hex
    /// triple. With no opacity modifier, emit two declarations:
    /// `<--tw-*-opacity>: 1` and `<property>: rgb(R G B / var(--tw-*-opacity, 1))`.
    /// With an opacity modifier (`/50`, `/[.31]`), emit one
    /// declaration with the alpha interpolated directly. Mirrors
    /// Tailwind's `withAlphaVariable.js`.
    Color {
        /// CSS property names to write. One value plugin can target
        /// multiple sides (`border-x-blue-500` writes both
        /// `border-left-color` and `border-right-color`).
        properties: &'static [&'static str],
        /// Custom-property name used for the opacity variable
        /// (`"--tw-bg-opacity"` etc.). When the candidate carries an
        /// opacity modifier we skip this and inline the alpha.
        opacity_var: &'static str,
        /// Whether to apply the `withAlphaVariable` wrapping. Most
        /// color plugins do (`bg-`, `text-`, `border-`); a few don't
        /// (`outline-color` in v3.4.19 emits the raw hex via
        /// `toColorValue`, no `--tw-*-opacity` variable). When false,
        /// the resolver writes the raw color value to the property
        /// and skips the opacity-variable indirection entirely.
        with_alpha_variable: bool,
    },
    /// Transform-shorthand utilities: `translate-x-4`, `rotate-45`,
    /// `scale-95`, `skew-x-12`. Per `corePlugins.js` the resolver:
    ///
    ///   1. Sets each `vars` entry to the resolved value.
    ///   2. Emits `transform: <CSS_TRANSFORM_VALUE>` so the new
    ///      variable composes with any others already set elsewhere.
    ///
    /// Implements the pattern Tailwind uses for the entire
    /// `transform` family. (Filter / backdrop-filter / shadow / ring
    /// follow the same shape with their own composed values; we add
    /// dedicated kinds for those when they land.)
    Transform {
        /// `--tw-*` custom-property names to assign the resolved
        /// value to. `scale` writes BOTH `--tw-scale-x` and
        /// `--tw-scale-y` from one input.
        vars: &'static [&'static str],
    },
    /// Filter / backdrop-filter family. Each utility writes one
    /// `--tw-<name>` var to `<fn>(<value>)` (with a special-case for
    /// `blur` / `backdrop-blur` whose empty theme value sets the var
    /// to a single space — Tailwind's way of suppressing `blur()`
    /// while keeping the composed filter chain valid). The shorthand
    /// `output_property: <composed>` re-asserts the full chain.
    Filter {
        /// Custom property name (e.g. `"--tw-blur"`).
        var: &'static str,
        /// CSS function name to wrap the value in
        /// (e.g. `"blur"`, `"brightness"`).
        wrap_fn: &'static str,
        /// Output properties (the composed shorthand is asserted on
        /// each). For `filter` this is `&["filter"]`. For
        /// `backdrop-filter` Tailwind emits BOTH `-webkit-backdrop-filter`
        /// AND `backdrop-filter` (vendor-prefix for older Safari) —
        /// `&["-webkit-backdrop-filter", "backdrop-filter"]`.
        output_properties: &'static [&'static str],
        /// Composed shorthand to assert on each output property.
        composed: &'static str,
        /// Allow empty theme value (the `blur-none` case) to set the
        /// var to `' '` instead of `<wrap>(<value>)`. Mirrors upstream
        /// `value.trim() === ''` branch.
        allow_empty: bool,
    },
    /// `transition` (transitionProperty plugin). Emits the
    /// `transition-property` decl, plus default
    /// `transition-timing-function` and `transition-duration` UNLESS
    /// the resolved value is `none`. Mirrors upstream:
    ///
    /// ```js
    /// 'transition-property': value,
    /// ...(value === 'none' ? {} : {
    ///   'transition-timing-function': defaultTimingFunction,
    ///   'transition-duration': defaultDuration,
    /// })
    /// ```
    TransitionProperty,
    /// `space-x-*` / `space-y-*` / `divide-x-*` / `divide-y-*`. The
    /// rule emits on `<class> > :not([hidden]) ~ :not([hidden])` and
    /// the body is a fixed template with `{}` substituted for the
    /// resolved value. Mirrors upstream `corePlugins.js` `space` and
    /// `divideWidth` plugins. Specifies a `reverse_var` that's
    /// initialised to `0` so the matching `*-reverse` static utility
    /// can flip it to `1`.
    Sibling {
        /// `(property, format)` pairs. `{}` in the format is replaced
        /// with the resolved value.
        decls: &'static [(&'static str, &'static str)],
    },
    /// `divide-<color>` — sibling selector + the standard
    /// withAlphaVariable color path on `border-color`. Theme key
    /// flows through `theme.divideColor` which defaults to
    /// `theme.borderColor` (per upstream).
    SiblingColor { opacity_var: &'static str },
    /// `placeholder-<color>` — emits on `<class>::placeholder` with
    /// the standard alpha-variable color path on `color`. Mirrors
    /// upstream's `placeholderColor` plugin.
    PlaceholderColor,
    /// `content-<value>` — emits `--tw-content: <value>; content:
    /// var(--tw-content)`. Theme key `content`. Mirrors upstream's
    /// `content` plugin shape.
    ContentVar,
    /// `shadow-<size>` (boxShadow plugin). Emits
    /// `--tw-shadow: <raw>; --tw-shadow-colored: <colored>;
    /// box-shadow: var(--tw-ring-offset-shadow, 0 0 #0000),
    ///             var(--tw-ring-shadow, 0 0 #0000),
    ///             var(--tw-shadow)`.
    /// The "colored" variant replaces every embedded color literal in
    /// the shadow value with `var(--tw-shadow-color)` so a sibling
    /// `shadow-<color>` utility can override the color without
    /// changing geometry. Mirrors upstream's `boxShadow` plugin
    /// driven by `parseBoxShadowValue` / `formatBoxShadowValue`.
    BoxShadow,
    /// `shadow-<color>` (boxShadowColor plugin). Sets
    /// `--tw-shadow-color` to the resolved color and
    /// `--tw-shadow: var(--tw-shadow-colored)` so that any prior
    /// `shadow-<size>`'s colored variant lights up. No
    /// `withAlphaVariable` indirection — uses `toColorValue` per
    /// upstream.
    BoxShadowColor,
    /// `ring` / `ring-N` (ringWidth plugin). Emits the three
    /// `--tw-ring-*-shadow` cascade vars + the composed `box-shadow`
    /// shorthand. Mirrors upstream's `ringWidth` plugin.
    RingWidth,
    /// `from-<color>` — sets `--tw-gradient-from`, `--tw-gradient-to`
    /// (to the same color with alpha=0 via `transparentTo`), and
    /// `--tw-gradient-stops`. Mirrors upstream's `gradientColorStops`
    /// `from` matcher.
    GradientFrom,
    /// `via-<color>` — sets `--tw-gradient-to` (transparent same
    /// color, with the double-space the upstream output happens to
    /// emit) and the three-stop `--tw-gradient-stops`.
    GradientVia,
    /// `to-<color>` — sets `--tw-gradient-to` to `<color>
    /// var(--tw-gradient-to-position)`.
    GradientTo,
    /// `animate-<name>` — emits `animation: <theme value>` AND ships
    /// the matching `@keyframes <name> { ... }` block at stylesheet
    /// scope. Mirrors upstream's `animation` plugin which calls
    /// `parseAnimationValue(value)` to extract the animation name
    /// and looks up `theme.keyframes[name]`.
    Animation,
    /// `<class><selector_suffix> { <var>: <resolved value> }`. Used by
    /// the per-property opacity utilities that ride on a non-default
    /// selector (`placeholder-opacity-*` lands on `::placeholder`,
    /// `divide-opacity-*` lands on the sibling-pair). Mirrors the
    /// matchUtilities calls in `corePlugins.js` `placeholderOpacity`
    /// and `divideOpacity`.
    SelectorVariable {
        selector_suffix: &'static str,
        var: &'static str,
    },
    /// `border-spacing-*` family — writes one or two `--tw-border-
    /// spacing-{x,y}` vars to the resolved scalar, then re-asserts
    /// the composed `border-spacing: var(--tw-border-spacing-x)
    /// var(--tw-border-spacing-y)` shorthand. Mirrors upstream's
    /// `borderSpacing` plugin which uses a `@defaults` directive +
    /// matchUtilities for the X/Y/both axes.
    BorderSpacing {
        /// Which `--tw-border-spacing-*` cascade vars to write the
        /// resolved scalar to. The full shorthand
        /// `border-spacing: var(--tw-border-spacing-x) var(--tw-border-
        /// spacing-y)` is emitted unconditionally.
        vars: &'static [&'static str],
    },
    /// `line-clamp-<n>` — the `-webkit-line-clamp` value scales with
    /// the theme key, but the rule also writes three constants
    /// (`overflow: hidden`, `display: -webkit-box`,
    /// `-webkit-box-orient: vertical`) per upstream's `lineClamp`
    /// plugin. The static `line-clamp-none` reset lives in
    /// STATIC_UTILITIES.
    LineClamp,
}

/// Cascade-variable defaults that ship inline at every utility-
/// using rule when `experimental.optimizeUniversalDefaults` is on.
/// Mirrors upstream's per-plugin `addDefaults` calls in
/// `corePlugins.js`. The keys are the group name; values are the
/// `(var, default)` pairs the universal `*, ::before, ::after`
/// block would emit when the flag is OFF. Each utility resolver
/// tags its `ResolvedDecls.defaults_groups` with the names that
/// apply.
pub fn defaults_for_group(name: &str) -> &'static [(&'static str, &'static str)] {
    match name {
        "box-shadow" => &[
            ("--tw-ring-offset-shadow", "0 0 #0000"),
            ("--tw-ring-shadow", "0 0 #0000"),
            ("--tw-shadow", "0 0 #0000"),
            ("--tw-shadow-colored", "0 0 #0000"),
        ],
        "ring-width" => &[
            ("--tw-ring-inset", " "),
            ("--tw-ring-offset-width", "0px"),
            ("--tw-ring-offset-color", "#fff"),
            ("--tw-ring-color", "rgb(59 130 246 / 0.5)"),
            ("--tw-ring-offset-shadow", "0 0 #0000"),
            ("--tw-ring-shadow", "0 0 #0000"),
            ("--tw-shadow", "0 0 #0000"),
            ("--tw-shadow-colored", "0 0 #0000"),
        ],
        "transform" => &[
            ("--tw-translate-x", "0"),
            ("--tw-translate-y", "0"),
            ("--tw-rotate", "0"),
            ("--tw-skew-x", "0"),
            ("--tw-skew-y", "0"),
            ("--tw-scale-x", "1"),
            ("--tw-scale-y", "1"),
        ],
        _ => &[],
    }
}

/// Mirrored from `corePlugins.js`'s `margin: createUtilityPlugin('margin', […], { supportsNegativeValues: true })`
/// and `padding: createUtilityPlugin('padding', …)`. Order within each
/// family is preserved so the emitted CSS is byte-aligned with the
/// oracle when the eventual sort key arrives.
pub static VALUE_UTILITIES: &[ValueUtility] = &[
    // ---- margin ----
    // Order matches upstream `margin` corePlugin emission. Tailwind's
    // `createUtilityPlugin` config:
    //   group 1: `m`
    //   group 2: `mx`, `my`                          (alphabetic)
    //   group 3: `ms`, `me`     -> alphabetic         (me, ms)
    //   group 4: `mt`, `mr`, `mb`, `ml` -> alphabetic (mb, ml, mr, mt)
    // matchUtilities emits each group alphabetically by class
    // prefix, but the groups themselves emit in declaration order.
    UV("m", &["margin"], "margin", true, "margin"),
    UV(
        "mx",
        &["margin-left", "margin-right"],
        "margin",
        true,
        "margin",
    ),
    UV(
        "my",
        &["margin-top", "margin-bottom"],
        "margin",
        true,
        "margin",
    ),
    UV("me", &["margin-inline-end"], "margin", true, "margin"),
    UV("ms", &["margin-inline-start"], "margin", true, "margin"),
    UV("mb", &["margin-bottom"], "margin", true, "margin"),
    UV("ml", &["margin-left"], "margin", true, "margin"),
    UV("mr", &["margin-right"], "margin", true, "margin"),
    UV("mt", &["margin-top"], "margin", true, "margin"),
    // ---- padding ----
    UV("p", &["padding"], "padding", false, "padding"),
    UV(
        "px",
        &["padding-left", "padding-right"],
        "padding",
        false,
        "padding",
    ),
    UV(
        "py",
        &["padding-top", "padding-bottom"],
        "padding",
        false,
        "padding",
    ),
    UV("ps", &["padding-inline-start"], "padding", false, "padding"),
    UV("pe", &["padding-inline-end"], "padding", false, "padding"),
    UV("pt", &["padding-top"], "padding", false, "padding"),
    UV("pr", &["padding-right"], "padding", false, "padding"),
    UV("pb", &["padding-bottom"], "padding", false, "padding"),
    UV("pl", &["padding-left"], "padding", false, "padding"),
    // ---- sizing ----
    // Note: `min-w` / `min-h` / `max-w` / `max-h` / `size` go BEFORE
    // `w` / `h` so the longest-prefix-wins rule never accidentally
    // matches the wrong family (e.g. `max-w-md` must hit the `max-w`
    // entry, not be left dangling because `w` was tested first).
    UV("size", &["width", "height"], "size", false, "size"),
    UV("min-w", &["min-width"], "minWidth", false, "minWidth"),
    UV("min-h", &["min-height"], "minHeight", false, "minHeight"),
    UV("max-w", &["max-width"], "maxWidth", false, "maxWidth"),
    UV("max-h", &["max-height"], "maxHeight", false, "maxHeight"),
    UV("w", &["width"], "width", false, "width"),
    UV("h", &["height"], "height", false, "height"),
    // ---- typography ----
    UV(
        "leading",
        &["line-height"],
        "lineHeight",
        false,
        "lineHeight",
    ),
    UV(
        "tracking",
        &["letter-spacing"],
        "letterSpacing",
        true,
        "letterSpacing",
    ),
    UV("indent", &["text-indent"], "textIndent", true, "textIndent"),
    // `font-` is shared between fontWeight and fontFamily. We declare
    // both entries with the same prefix; the lookup walks all matches
    // and uses the first whose theme value resolves. fontWeight comes
    // first because Tailwind registers it before fontFamily — for
    // the disjoint key sets we ship today (thin/normal/bold for weight
    // vs sans/serif/mono for family) there's no conflict, but the
    // ordering is stable for future tweaks.
    UVK(
        "font",
        &["font-weight"],
        "fontWeight",
        false,
        ResolverKind::Simple,
        "fontWeight",
    ),
    UVK(
        "font",
        &[],
        "fontFamily",
        false,
        ResolverKind::FontFamily,
        "fontFamily",
    ),
    // `text-` for fontSize. The kind=FontSize resolver unpacks the
    // tuple in `theme.fontSize.<key>` and emits font-size + optional
    // line-height/letter-spacing/font-weight in one rule. Slash
    // modifier (`text-sm/6`) overrides the line-height.
    UVK(
        "text",
        &[],
        "fontSize",
        false,
        ResolverKind::FontSize,
        "fontSize",
    ),
    // ---- colors ----
    // bg / text / border share a `<prefix>-` namespace with other
    // plugins (text-color shares with text-fontSize, border-color
    // shares with border-{width,style,radius}). The matching
    // walks all entries with the same prefix and takes the first
    // whose theme lookup succeeds — exactly the order Tailwind's
    // plugin pipeline registers them.
    UVK(
        "bg",
        &[],
        "backgroundColor",
        false,
        ResolverKind::Color {
            properties: &["background-color"],
            opacity_var: "--tw-bg-opacity",
            with_alpha_variable: true,
        },
        "backgroundColor",
    ),
    UVK(
        "text",
        &[],
        "textColor",
        false,
        ResolverKind::Color {
            properties: &["color"],
            opacity_var: "--tw-text-opacity",
            with_alpha_variable: true,
        },
        "textColor",
    ),
    // ---- borders ----
    // Border width entries — mirror upstream's `createUtilityPlugin(
    // 'borderWidth', [['border'], ['border-x'], ['border-y'],
    // ['border-s'], ['border-e'], ['border-t'], ['border-r'],
    // ['border-b'], ['border-l']])` declaration order. Then border
    // color entries, mirroring upstream's three sequential
    // `matchUtilities` calls in the `borderColor` corePlugin: bare
    // `border` first, then `border-x`/`border-y`, then the per-side
    // entries.
    UV(
        "border",
        &["border-width"],
        "borderWidth",
        false,
        "borderWidth",
    ),
    UV(
        "border-x",
        &["border-left-width", "border-right-width"],
        "borderWidth",
        false,
        "borderWidth",
    ),
    UV(
        "border-y",
        &["border-top-width", "border-bottom-width"],
        "borderWidth",
        false,
        "borderWidth",
    ),
    UV(
        "border-s",
        &["border-inline-start-width"],
        "borderWidth",
        false,
        "borderWidth",
    ),
    UV(
        "border-e",
        &["border-inline-end-width"],
        "borderWidth",
        false,
        "borderWidth",
    ),
    UV(
        "border-t",
        &["border-top-width"],
        "borderWidth",
        false,
        "borderWidth",
    ),
    UV(
        "border-r",
        &["border-right-width"],
        "borderWidth",
        false,
        "borderWidth",
    ),
    UV(
        "border-b",
        &["border-bottom-width"],
        "borderWidth",
        false,
        "borderWidth",
    ),
    UV(
        "border-l",
        &["border-left-width"],
        "borderWidth",
        false,
        "borderWidth",
    ),
    UVK_FD(
        "border",
        &[],
        "borderColor",
        ResolverKind::Color {
            properties: &["border-color"],
            opacity_var: "--tw-border-opacity",
            with_alpha_variable: true,
        },
        "borderColor",
    ),
    UVK_FD(
        "border-x",
        &[],
        "borderColor",
        ResolverKind::Color {
            properties: &["border-left-color", "border-right-color"],
            opacity_var: "--tw-border-opacity",
            with_alpha_variable: true,
        },
        "borderColor",
    ),
    UVK_FD(
        "border-y",
        &[],
        "borderColor",
        ResolverKind::Color {
            properties: &["border-top-color", "border-bottom-color"],
            opacity_var: "--tw-border-opacity",
            with_alpha_variable: true,
        },
        "borderColor",
    ),
    UVK_FD(
        "border-s",
        &[],
        "borderColor",
        ResolverKind::Color {
            properties: &["border-inline-start-color"],
            opacity_var: "--tw-border-opacity",
            with_alpha_variable: true,
        },
        "borderColor",
    ),
    UVK_FD(
        "border-e",
        &[],
        "borderColor",
        ResolverKind::Color {
            properties: &["border-inline-end-color"],
            opacity_var: "--tw-border-opacity",
            with_alpha_variable: true,
        },
        "borderColor",
    ),
    UVK_FD(
        "border-t",
        &[],
        "borderColor",
        ResolverKind::Color {
            properties: &["border-top-color"],
            opacity_var: "--tw-border-opacity",
            with_alpha_variable: true,
        },
        "borderColor",
    ),
    UVK_FD(
        "border-r",
        &[],
        "borderColor",
        ResolverKind::Color {
            properties: &["border-right-color"],
            opacity_var: "--tw-border-opacity",
            with_alpha_variable: true,
        },
        "borderColor",
    ),
    UVK_FD(
        "border-b",
        &[],
        "borderColor",
        ResolverKind::Color {
            properties: &["border-bottom-color"],
            opacity_var: "--tw-border-opacity",
            with_alpha_variable: true,
        },
        "borderColor",
    ),
    UVK_FD(
        "border-l",
        &[],
        "borderColor",
        ResolverKind::Color {
            properties: &["border-left-color"],
            opacity_var: "--tw-border-opacity",
            with_alpha_variable: true,
        },
        "borderColor",
    ),
    // ---- border-radius ----
    UV(
        "rounded-ss",
        &["border-start-start-radius"],
        "borderRadius",
        false,
        "borderRadius",
    ),
    UV(
        "rounded-se",
        &["border-start-end-radius"],
        "borderRadius",
        false,
        "borderRadius",
    ),
    UV(
        "rounded-ee",
        &["border-end-end-radius"],
        "borderRadius",
        false,
        "borderRadius",
    ),
    UV(
        "rounded-es",
        &["border-end-start-radius"],
        "borderRadius",
        false,
        "borderRadius",
    ),
    UV(
        "rounded-tl",
        &["border-top-left-radius"],
        "borderRadius",
        false,
        "borderRadius",
    ),
    UV(
        "rounded-tr",
        &["border-top-right-radius"],
        "borderRadius",
        false,
        "borderRadius",
    ),
    UV(
        "rounded-br",
        &["border-bottom-right-radius"],
        "borderRadius",
        false,
        "borderRadius",
    ),
    UV(
        "rounded-bl",
        &["border-bottom-left-radius"],
        "borderRadius",
        false,
        "borderRadius",
    ),
    UV(
        "rounded-s",
        &["border-start-start-radius", "border-end-start-radius"],
        "borderRadius",
        false,
        "borderRadius",
    ),
    UV(
        "rounded-e",
        &["border-start-end-radius", "border-end-end-radius"],
        "borderRadius",
        false,
        "borderRadius",
    ),
    UV(
        "rounded-t",
        &["border-top-left-radius", "border-top-right-radius"],
        "borderRadius",
        false,
        "borderRadius",
    ),
    UV(
        "rounded-r",
        &["border-top-right-radius", "border-bottom-right-radius"],
        "borderRadius",
        false,
        "borderRadius",
    ),
    UV(
        "rounded-b",
        &["border-bottom-right-radius", "border-bottom-left-radius"],
        "borderRadius",
        false,
        "borderRadius",
    ),
    UV(
        "rounded-l",
        &["border-top-left-radius", "border-bottom-left-radius"],
        "borderRadius",
        false,
        "borderRadius",
    ),
    UV(
        "rounded",
        &["border-radius"],
        "borderRadius",
        false,
        "borderRadius",
    ),
    // ---- flex / grid layout ----
    // For flexGrow / flexShrink, Tailwind's `createUtilityPlugin`
    // declares the DEPRECATED alias first (`flex-grow`, `flex-shrink`)
    // then the canonical short form (`grow`, `shrink`). Each is its
    // own matchUtilities group so the deprecated form emits before
    // the canonical one. We declare them in matching order so the
    // value-utility within_plugin_order tracks Tailwind output.
    UV("basis", &["flex-basis"], "flexBasis", false, "flexBasis"),
    UV("flex", &["flex"], "flex", false, "flex"),
    UV("flex-grow", &["flex-grow"], "flexGrow", false, "flexGrow"),
    UV("grow", &["flex-grow"], "flexGrow", false, "flexGrow"),
    UV(
        "flex-shrink",
        &["flex-shrink"],
        "flexShrink",
        false,
        "flexShrink",
    ),
    UV(
        "shrink",
        &["flex-shrink"],
        "flexShrink",
        false,
        "flexShrink",
    ),
    UV("order", &["order"], "order", true, "order"),
    UV("gap", &["gap"], "gap", false, "gap"),
    UV("gap-x", &["column-gap"], "gap", false, "gap"),
    UV("gap-y", &["row-gap"], "gap", false, "gap"),
    // grid templates
    UV(
        "grid-cols",
        &["grid-template-columns"],
        "gridTemplateColumns",
        false,
        "gridTemplateColumns",
    ),
    UV(
        "grid-rows",
        &["grid-template-rows"],
        "gridTemplateRows",
        false,
        "gridTemplateRows",
    ),
    // grid placements (col-* / row-*) — support negation per upstream.
    UV(
        "col-start",
        &["grid-column-start"],
        "gridColumnStart",
        true,
        "gridColumnStart",
    ),
    UV(
        "col-end",
        &["grid-column-end"],
        "gridColumnEnd",
        true,
        "gridColumnEnd",
    ),
    UV("col", &["grid-column"], "gridColumn", false, "gridColumn"),
    UV(
        "row-start",
        &["grid-row-start"],
        "gridRowStart",
        true,
        "gridRowStart",
    ),
    UV(
        "row-end",
        &["grid-row-end"],
        "gridRowEnd",
        true,
        "gridRowEnd",
    ),
    UV("row", &["grid-row"], "gridRow", false, "gridRow"),
    // grid auto cols/rows
    UV(
        "auto-cols",
        &["grid-auto-columns"],
        "gridAutoColumns",
        false,
        "gridAutoColumns",
    ),
    UV(
        "auto-rows",
        &["grid-auto-rows"],
        "gridAutoRows",
        false,
        "gridAutoRows",
    ),
    // ---- transforms ----
    // `translate-x` / `translate-y` go BEFORE `translate` so the
    // longest-prefix match works; same for `scale-x` / `scale-y` and
    // `skew-x` / `skew-y`.
    UVK(
        "translate-x",
        &[],
        "translate",
        true,
        ResolverKind::Transform {
            vars: &["--tw-translate-x"],
        },
        "translate",
    ),
    UVK(
        "translate-y",
        &[],
        "translate",
        true,
        ResolverKind::Transform {
            vars: &["--tw-translate-y"],
        },
        "translate",
    ),
    // Order matches upstream `scale` corePlugin emission:
    //   group 1: `scale` (writes both x and y)
    //   group 2: `scale-x`, `scale-y` (alphabetic within group)
    UVK(
        "scale",
        &[],
        "scale",
        true,
        ResolverKind::Transform {
            vars: &["--tw-scale-x", "--tw-scale-y"],
        },
        "scale",
    ),
    UVK(
        "scale-x",
        &[],
        "scale",
        true,
        ResolverKind::Transform {
            vars: &["--tw-scale-x"],
        },
        "scale",
    ),
    UVK(
        "scale-y",
        &[],
        "scale",
        true,
        ResolverKind::Transform {
            vars: &["--tw-scale-y"],
        },
        "scale",
    ),
    UVK(
        "skew-x",
        &[],
        "skew",
        true,
        ResolverKind::Transform {
            vars: &["--tw-skew-x"],
        },
        "skew",
    ),
    UVK(
        "skew-y",
        &[],
        "skew",
        true,
        ResolverKind::Transform {
            vars: &["--tw-skew-y"],
        },
        "skew",
    ),
    UVK(
        "rotate",
        &[],
        "rotate",
        true,
        ResolverKind::Transform {
            vars: &["--tw-rotate"],
        },
        "rotate",
    ),
    // ---- filter family ----
    // Each filter utility writes its `--tw-<name>` var to
    // `<fn>(<value>)` and re-asserts the composed `filter` shorthand.
    // Mirrors `corePlugins.js` `blur`, `brightness`, `contrast`,
    // `grayscale`, `hueRotate`, `invert`, `saturate`, `sepia`,
    // `dropShadow` plugins driven by `cssFilterValue`.
    UVK(
        "blur",
        &[],
        "blur",
        false,
        ResolverKind::Filter {
            var: "--tw-blur",
            wrap_fn: "blur",
            output_properties: &["filter"],
            composed: CSS_FILTER_VALUE,
            allow_empty: true,
        },
        "blur",
    ),
    UVK(
        "brightness",
        &[],
        "brightness",
        false,
        ResolverKind::Filter {
            var: "--tw-brightness",
            wrap_fn: "brightness",
            output_properties: &["filter"],
            composed: CSS_FILTER_VALUE,
            allow_empty: false,
        },
        "brightness",
    ),
    UVK(
        "contrast",
        &[],
        "contrast",
        false,
        ResolverKind::Filter {
            var: "--tw-contrast",
            wrap_fn: "contrast",
            output_properties: &["filter"],
            composed: CSS_FILTER_VALUE,
            allow_empty: false,
        },
        "contrast",
    ),
    UVK(
        "grayscale",
        &[],
        "grayscale",
        false,
        ResolverKind::Filter {
            var: "--tw-grayscale",
            wrap_fn: "grayscale",
            output_properties: &["filter"],
            composed: CSS_FILTER_VALUE,
            allow_empty: false,
        },
        "grayscale",
    ),
    UVK(
        "hue-rotate",
        &[],
        "hueRotate",
        true,
        ResolverKind::Filter {
            var: "--tw-hue-rotate",
            wrap_fn: "hue-rotate",
            output_properties: &["filter"],
            composed: CSS_FILTER_VALUE,
            allow_empty: false,
        },
        "hueRotate",
    ),
    UVK(
        "invert",
        &[],
        "invert",
        false,
        ResolverKind::Filter {
            var: "--tw-invert",
            wrap_fn: "invert",
            output_properties: &["filter"],
            composed: CSS_FILTER_VALUE,
            allow_empty: false,
        },
        "invert",
    ),
    UVK(
        "saturate",
        &[],
        "saturate",
        false,
        ResolverKind::Filter {
            var: "--tw-saturate",
            wrap_fn: "saturate",
            output_properties: &["filter"],
            composed: CSS_FILTER_VALUE,
            allow_empty: false,
        },
        "saturate",
    ),
    UVK(
        "sepia",
        &[],
        "sepia",
        false,
        ResolverKind::Filter {
            var: "--tw-sepia",
            wrap_fn: "sepia",
            output_properties: &["filter"],
            composed: CSS_FILTER_VALUE,
            allow_empty: false,
        },
        "sepia",
    ),
    UVK(
        "drop-shadow",
        &[],
        "dropShadow",
        false,
        ResolverKind::Filter {
            var: "--tw-drop-shadow",
            wrap_fn: "drop-shadow",
            output_properties: &["filter"],
            composed: CSS_FILTER_VALUE,
            allow_empty: false,
        },
        "dropShadow",
    ),
    // ---- backdrop-filter family ----
    UVK(
        "backdrop-blur",
        &[],
        "backdropBlur",
        false,
        ResolverKind::Filter {
            var: "--tw-backdrop-blur",
            wrap_fn: "blur",
            output_properties: &["-webkit-backdrop-filter", "backdrop-filter"],
            composed: CSS_BACKDROP_FILTER_VALUE,
            allow_empty: true,
        },
        "backdropBlur",
    ),
    UVK(
        "backdrop-brightness",
        &[],
        "backdropBrightness",
        false,
        ResolverKind::Filter {
            var: "--tw-backdrop-brightness",
            wrap_fn: "brightness",
            output_properties: &["-webkit-backdrop-filter", "backdrop-filter"],
            composed: CSS_BACKDROP_FILTER_VALUE,
            allow_empty: false,
        },
        "backdropBrightness",
    ),
    UVK(
        "backdrop-contrast",
        &[],
        "backdropContrast",
        false,
        ResolverKind::Filter {
            var: "--tw-backdrop-contrast",
            wrap_fn: "contrast",
            output_properties: &["-webkit-backdrop-filter", "backdrop-filter"],
            composed: CSS_BACKDROP_FILTER_VALUE,
            allow_empty: false,
        },
        "backdropContrast",
    ),
    UVK(
        "backdrop-grayscale",
        &[],
        "backdropGrayscale",
        false,
        ResolverKind::Filter {
            var: "--tw-backdrop-grayscale",
            wrap_fn: "grayscale",
            output_properties: &["-webkit-backdrop-filter", "backdrop-filter"],
            composed: CSS_BACKDROP_FILTER_VALUE,
            allow_empty: false,
        },
        "backdropGrayscale",
    ),
    UVK(
        "backdrop-hue-rotate",
        &[],
        "backdropHueRotate",
        true,
        ResolverKind::Filter {
            var: "--tw-backdrop-hue-rotate",
            wrap_fn: "hue-rotate",
            output_properties: &["-webkit-backdrop-filter", "backdrop-filter"],
            composed: CSS_BACKDROP_FILTER_VALUE,
            allow_empty: false,
        },
        "backdropHueRotate",
    ),
    UVK(
        "backdrop-invert",
        &[],
        "backdropInvert",
        false,
        ResolverKind::Filter {
            var: "--tw-backdrop-invert",
            wrap_fn: "invert",
            output_properties: &["-webkit-backdrop-filter", "backdrop-filter"],
            composed: CSS_BACKDROP_FILTER_VALUE,
            allow_empty: false,
        },
        "backdropInvert",
    ),
    UVK(
        "backdrop-opacity",
        &[],
        "backdropOpacity",
        false,
        ResolverKind::Filter {
            var: "--tw-backdrop-opacity",
            wrap_fn: "opacity",
            output_properties: &["-webkit-backdrop-filter", "backdrop-filter"],
            composed: CSS_BACKDROP_FILTER_VALUE,
            allow_empty: false,
        },
        "backdropOpacity",
    ),
    UVK(
        "backdrop-saturate",
        &[],
        "backdropSaturate",
        false,
        ResolverKind::Filter {
            var: "--tw-backdrop-saturate",
            wrap_fn: "saturate",
            output_properties: &["-webkit-backdrop-filter", "backdrop-filter"],
            composed: CSS_BACKDROP_FILTER_VALUE,
            allow_empty: false,
        },
        "backdropSaturate",
    ),
    UVK(
        "backdrop-sepia",
        &[],
        "backdropSepia",
        false,
        ResolverKind::Filter {
            var: "--tw-backdrop-sepia",
            wrap_fn: "sepia",
            output_properties: &["-webkit-backdrop-filter", "backdrop-filter"],
            composed: CSS_BACKDROP_FILTER_VALUE,
            allow_empty: false,
        },
        "backdropSepia",
    ),
    // ---- opacity ----
    UV("opacity", &["opacity"], "opacity", false, "opacity"),
    // ---- per-property opacity utilities (cascade-var setters) ----
    // `bg-opacity-50` → `--tw-bg-opacity: 0.5`. Mirrors upstream's
    // `createUtilityPlugin('backgroundOpacity', [['bg-opacity',
    // ['--tw-bg-opacity']]])` — each utility sets a single CSS
    // custom property. The companion color plugin (`bg-red-500`,
    // etc.) reads the same var via `var(--tw-bg-opacity, 1)` so
    // these compose by setting the alpha at runtime.
    UV(
        "bg-opacity",
        &["--tw-bg-opacity"],
        "backgroundOpacity",
        false,
        "backgroundOpacity",
    ),
    UV(
        "border-opacity",
        &["--tw-border-opacity"],
        "borderOpacity",
        false,
        "borderOpacity",
    ),
    UV(
        "text-opacity",
        &["--tw-text-opacity"],
        "textOpacity",
        false,
        "textOpacity",
    ),
    UV(
        "ring-opacity",
        &["--tw-ring-opacity"],
        "ringOpacity",
        false,
        "ringOpacity",
    ),
    UVK(
        "placeholder-opacity",
        &[],
        "placeholderOpacity",
        false,
        ResolverKind::SelectorVariable {
            selector_suffix: "::placeholder",
            var: "--tw-placeholder-opacity",
        },
        "placeholderOpacity",
    ),
    UVK(
        "divide-opacity",
        &[],
        "divideOpacity",
        false,
        ResolverKind::SelectorVariable {
            selector_suffix: " > :not([hidden]) ~ :not([hidden])",
            var: "--tw-divide-opacity",
        },
        "divideOpacity",
    ),
    // ---- columns (multi-column-layout) ----
    UV("columns", &["columns"], "columns", false, "columns"),
    // ---- aspect-ratio ----
    UV(
        "aspect",
        &["aspect-ratio"],
        "aspectRatio",
        false,
        "aspectRatio",
    ),
    // ---- object-position (arbitrary form) ----
    // Static keyword variants (`object-top`, `object-center`, etc.)
    // live in STATIC_UTILITIES; this entry handles
    // `object-[<arbitrary>]` and any user-extended theme keys.
    UV(
        "object",
        &["object-position"],
        "objectPosition",
        false,
        "objectPosition",
    ),
    // ---- vertical-align (matchUtilities-only — no theme values) ----
    // Upstream's verticalAlign plugin registers static keyword
    // variants AND a `matchUtilities` for `align-` with no `values`
    // table — meaning only arbitrary values resolve. We replicate by
    // using an empty theme key (`alignVertical` doesn't exist) so
    // bare `align-X` won't accidentally hit the value resolver.
    UV(
        "align",
        &["vertical-align"],
        "verticalAlign",
        false,
        "verticalAlign",
    ),
    // ---- will-change ----
    UV(
        "will-change",
        &["will-change"],
        "willChange",
        false,
        "willChange",
    ),
    // ---- border-spacing (cascade-var family) ----
    // Upstream `borderSpacing` plugin order:
    //   group 1: `border-spacing` (writes both x and y)
    //   group 2: `border-spacing-x`, `border-spacing-y` (alphabetic)
    UVK(
        "border-spacing",
        &[],
        "borderSpacing",
        false,
        ResolverKind::BorderSpacing {
            vars: &["--tw-border-spacing-x", "--tw-border-spacing-y"],
        },
        "borderSpacing",
    ),
    UVK(
        "border-spacing-x",
        &[],
        "borderSpacing",
        false,
        ResolverKind::BorderSpacing {
            vars: &["--tw-border-spacing-x"],
        },
        "borderSpacing",
    ),
    UVK(
        "border-spacing-y",
        &[],
        "borderSpacing",
        false,
        ResolverKind::BorderSpacing {
            vars: &["--tw-border-spacing-y"],
        },
        "borderSpacing",
    ),
    // ---- outline ----
    UV(
        "outline-offset",
        &["outline-offset"],
        "outlineOffset",
        true,
        "outlineOffset",
    ),
    UV(
        "outline",
        &["outline-width"],
        "outlineWidth",
        false,
        "outlineWidth",
    ),
    UVK(
        "outline",
        &[],
        "outlineColor",
        false,
        ResolverKind::Color {
            properties: &["outline-color"],
            opacity_var: "--tw-outline-opacity",
            with_alpha_variable: false,
        },
        "outlineColor",
    ),
    // ---- SVG fill / stroke ----
    // `fill-<color>` and `stroke-<color>` use the raw-value path
    // (no `--tw-*-opacity` indirection) per upstream's `fill` /
    // `stroke` plugins which call `toColorValue` directly.
    UVK(
        "fill",
        &[],
        "fill",
        false,
        ResolverKind::Color {
            properties: &["fill"],
            opacity_var: "--tw-fill-opacity",
            with_alpha_variable: false,
        },
        "fill",
    ),
    // `stroke-<width>` (numeric scale) goes FIRST so `stroke-2`
    // resolves to `stroke-width: 2`. `stroke-<color>` is the
    // fall-through for non-numeric value keys.
    UV(
        "stroke",
        &["stroke-width"],
        "strokeWidth",
        false,
        "strokeWidth",
    ),
    UVK(
        "stroke",
        &[],
        "stroke",
        false,
        ResolverKind::Color {
            properties: &["stroke"],
            opacity_var: "--tw-stroke-opacity",
            with_alpha_variable: false,
        },
        "stroke",
    ),
    // ---- text-decoration ----
    // Two plugins share the `decoration-` prefix in upstream:
    //   - `textDecorationThickness` (lengths, `auto`, `from-font`)
    //   - `textDecorationColor` (color values)
    //
    // The plugin order matters — find_value_utilities walks them in
    // declaration order and picks the first whose resolver succeeds.
    // We declare thickness FIRST so the named numeric scale
    // (`decoration-0`, `decoration-2`) resolves to thickness;
    // `decoration-red-500` falls through to the color resolver
    // because `red-500` isn't a thickness theme key. Arbitrary
    // values (`decoration-[3px]`) bind to thickness too — we don't
    // validate length-vs-color so the user's intent is inferred
    // from the theme's named keys.
    UV(
        "decoration",
        &["text-decoration-thickness"],
        "textDecorationThickness",
        false,
        "textDecorationThickness",
    ),
    UVK(
        "decoration",
        &[],
        "textDecorationColor",
        false,
        ResolverKind::Color {
            properties: &["text-decoration-color"],
            opacity_var: "--tw-text-decoration-color-opacity",
            with_alpha_variable: false,
        },
        "textDecorationColor",
    ),
    // ---- placeholder color ----
    // `placeholder-<color>` emits on `<class>::placeholder` with the
    // standard alpha-variable color path. Sibling-suffix shape;
    // matches upstream's `placeholderColor` plugin.
    UVK(
        "placeholder",
        &[],
        "placeholderColor",
        false,
        ResolverKind::PlaceholderColor,
        "placeholderColor",
    ),
    // ---- content ----
    // `content-<value>` uses the `--tw-content` cascade var so the
    // user can stack `content: var(--tw-content)` rules. Theme key
    // `content` (defaults `none`); arbitrary values pass through.
    // Static `content-none` reset emits the literal `none`.
    UVK(
        "content",
        &[],
        "content",
        false,
        ResolverKind::ContentVar,
        "content",
    ),
    // ---- inset / position offsets ----
    // Order matches upstream's `inset` corePlugin emission. Tailwind
    // splits inset into four `matchUtilities` groups (declared in
    // `createUtilityPlugin('inset', [...])`):
    //   group 1: `inset`
    //   group 2: `inset-x`, `inset-y`     -> alphabetical
    //   group 3: `start`, `end`            -> alphabetical -> end, start
    //   group 4: `top`, `right`, `bottom`, `left` -> alphabetical
    //     -> bottom, left, right, top
    // Used as `within_plugin_order` so `class="inset-0 top-6"`
    // last-wins to `top-6` per the cascade.
    UV("inset", &["inset"], "inset", true, "inset"),
    UV("inset-x", &["left", "right"], "inset", true, "inset"),
    UV("inset-y", &["top", "bottom"], "inset", true, "inset"),
    UV("end", &["inset-inline-end"], "inset", true, "inset"),
    UV("start", &["inset-inline-start"], "inset", true, "inset"),
    UV("bottom", &["bottom"], "inset", true, "inset"),
    UV("left", &["left"], "inset", true, "inset"),
    UV("right", &["right"], "inset", true, "inset"),
    UV("top", &["top"], "inset", true, "inset"),
    // ---- z-index ----
    UV("z", &["z-index"], "zIndex", true, "zIndex"),
    // ---- cursor ----
    UV("cursor", &["cursor"], "cursor", false, "cursor"),
    // ---- transitions ----
    UVK(
        "transition",
        &[],
        "transitionProperty",
        false,
        ResolverKind::TransitionProperty,
        "transitionProperty",
    ),
    // Duration and easing use `filterDefault: true` upstream — the
    // bare `.duration` / `.ease` are NOT emitted even though their
    // theme tables have a `DEFAULT` entry. (DEFAULTs exist so the
    // `transition-property` plugin's "append default duration +
    // timing-function" rule has values to read.)
    UV_FD(
        "duration",
        &["transition-duration"],
        "transitionDuration",
        "transitionDuration",
    ),
    UV_FD(
        "ease",
        &["transition-timing-function"],
        "transitionTimingFunction",
        "transitionTimingFunction",
    ),
    UV(
        "delay",
        &["transition-delay"],
        "transitionDelay",
        false,
        "transitionDelay",
    ),
    // ---- animations ----
    UVK(
        "animate",
        &[],
        "animation",
        false,
        ResolverKind::Animation,
        "animation",
    ),
    // ---- line-clamp ----
    UVK(
        "line-clamp",
        &[],
        "lineClamp",
        false,
        ResolverKind::LineClamp,
        "lineClamp",
    ),
    // ---- text-underline-offset ----
    UV(
        "underline-offset",
        &["text-underline-offset"],
        "textUnderlineOffset",
        false,
        "textUnderlineOffset",
    ),
    // ---- transform-origin (arbitrary form) ----
    // Named keywords (`origin-top-right`, `origin-center` etc.) live
    // in STATIC_UTILITIES — those are the eight predefined values
    // upstream's transformOrigin theme ships. The value-utility entry
    // here catches the arbitrary form `origin-[65%_0%]` (theme
    // resolution falls through because `transformOrigin` isn't
    // registered as a default theme table on the Rust side).
    UV(
        "origin",
        &["transform-origin"],
        "transformOrigin",
        false,
        "transformOrigin",
    ),
    // (`grow`, `flex-grow`, `shrink`, `flex-shrink` are declared near
    // the top of the table in upstream's deprecated-alias-first order
    // — they live under `// ---- flex / grid layout ----` so the
    // within_plugin_order matches Tailwind's `createUtilityPlugin`
    // emission.)
    // ---- scroll-margin / scroll-padding ----
    UV(
        "scroll-mx",
        &["scroll-margin-left", "scroll-margin-right"],
        "scrollMargin",
        true,
        "scrollMargin",
    ),
    UV(
        "scroll-my",
        &["scroll-margin-top", "scroll-margin-bottom"],
        "scrollMargin",
        true,
        "scrollMargin",
    ),
    UV(
        "scroll-mt",
        &["scroll-margin-top"],
        "scrollMargin",
        true,
        "scrollMargin",
    ),
    UV(
        "scroll-mr",
        &["scroll-margin-right"],
        "scrollMargin",
        true,
        "scrollMargin",
    ),
    UV(
        "scroll-mb",
        &["scroll-margin-bottom"],
        "scrollMargin",
        true,
        "scrollMargin",
    ),
    UV(
        "scroll-ml",
        &["scroll-margin-left"],
        "scrollMargin",
        true,
        "scrollMargin",
    ),
    UV(
        "scroll-ms",
        &["scroll-margin-inline-start"],
        "scrollMargin",
        true,
        "scrollMargin",
    ),
    UV(
        "scroll-me",
        &["scroll-margin-inline-end"],
        "scrollMargin",
        true,
        "scrollMargin",
    ),
    UV(
        "scroll-m",
        &["scroll-margin"],
        "scrollMargin",
        true,
        "scrollMargin",
    ),
    UV(
        "scroll-px",
        &["scroll-padding-left", "scroll-padding-right"],
        "scrollPadding",
        false,
        "scrollPadding",
    ),
    UV(
        "scroll-py",
        &["scroll-padding-top", "scroll-padding-bottom"],
        "scrollPadding",
        false,
        "scrollPadding",
    ),
    UV(
        "scroll-pt",
        &["scroll-padding-top"],
        "scrollPadding",
        false,
        "scrollPadding",
    ),
    UV(
        "scroll-pr",
        &["scroll-padding-right"],
        "scrollPadding",
        false,
        "scrollPadding",
    ),
    UV(
        "scroll-pb",
        &["scroll-padding-bottom"],
        "scrollPadding",
        false,
        "scrollPadding",
    ),
    UV(
        "scroll-pl",
        &["scroll-padding-left"],
        "scrollPadding",
        false,
        "scrollPadding",
    ),
    UV(
        "scroll-ps",
        &["scroll-padding-inline-start"],
        "scrollPadding",
        false,
        "scrollPadding",
    ),
    UV(
        "scroll-pe",
        &["scroll-padding-inline-end"],
        "scrollPadding",
        false,
        "scrollPadding",
    ),
    UV(
        "scroll-p",
        &["scroll-padding"],
        "scrollPadding",
        false,
        "scrollPadding",
    ),
    // ---- accent / caret colors ----
    UVK(
        "accent",
        &[],
        "accentColor",
        false,
        ResolverKind::Color {
            properties: &["accent-color"],
            opacity_var: "--tw-accent-opacity",
            with_alpha_variable: false,
        },
        "accentColor",
    ),
    UVK(
        "caret",
        &[],
        "caretColor",
        false,
        ResolverKind::Color {
            properties: &["caret-color"],
            opacity_var: "--tw-caret-opacity",
            with_alpha_variable: false,
        },
        "caretColor",
    ),
    // ---- space-x / space-y (sibling-pair) ----
    UVK(
        "space-x",
        &[],
        "space",
        true,
        ResolverKind::Sibling {
            decls: &[
                ("--tw-space-x-reverse", "0"),
                ("margin-right", "calc({} * var(--tw-space-x-reverse))"),
                (
                    "margin-left",
                    "calc({} * calc(1 - var(--tw-space-x-reverse)))",
                ),
            ],
        },
        "space",
    ),
    UVK(
        "space-y",
        &[],
        "space",
        true,
        ResolverKind::Sibling {
            decls: &[
                ("--tw-space-y-reverse", "0"),
                (
                    "margin-top",
                    "calc({} * calc(1 - var(--tw-space-y-reverse)))",
                ),
                ("margin-bottom", "calc({} * var(--tw-space-y-reverse))"),
            ],
        },
        "space",
    ),
    // ---- divide-x / divide-y (sibling-pair widths) ----
    UVK(
        "divide-x",
        &[],
        "divideWidth",
        false,
        ResolverKind::Sibling {
            decls: &[
                ("--tw-divide-x-reverse", "0"),
                (
                    "border-right-width",
                    "calc({} * var(--tw-divide-x-reverse))",
                ),
                (
                    "border-left-width",
                    "calc({} * calc(1 - var(--tw-divide-x-reverse)))",
                ),
            ],
        },
        "divideWidth",
    ),
    UVK(
        "divide-y",
        &[],
        "divideWidth",
        false,
        ResolverKind::Sibling {
            decls: &[
                ("--tw-divide-y-reverse", "0"),
                (
                    "border-top-width",
                    "calc({} * calc(1 - var(--tw-divide-y-reverse)))",
                ),
                (
                    "border-bottom-width",
                    "calc({} * var(--tw-divide-y-reverse))",
                ),
            ],
        },
        "divideWidth",
    ),
    // ---- shadow + ring ----
    // `shadow-` is shared between boxShadow (size) and boxShadowColor.
    UVK(
        "shadow",
        &[],
        "boxShadow",
        false,
        ResolverKind::BoxShadow,
        "boxShadow",
    ),
    UVK(
        "shadow",
        &[],
        "boxShadowColor",
        false,
        ResolverKind::BoxShadowColor,
        "boxShadowColor",
    ),
    // `ring-offset-` goes BEFORE `ring-` so longest-prefix wins for
    // `ring-offset-2` (offset width) and `ring-offset-blue-500`
    // (offset color).
    UV(
        "ring-offset",
        &["--tw-ring-offset-width"],
        "ringOffsetWidth",
        false,
        "ringOffsetWidth",
    ),
    UVK(
        "ring-offset",
        &[],
        "ringOffsetColor",
        false,
        ResolverKind::Color {
            properties: &["--tw-ring-offset-color"],
            opacity_var: "--tw-ring-offset-opacity",
            with_alpha_variable: false,
        },
        "ringOffsetColor",
    ),
    // `ring-` (without offset prefix): width OR color.
    UVK(
        "ring",
        &[],
        "ringWidth",
        false,
        ResolverKind::RingWidth,
        "ringWidth",
    ),
    UVK_FD(
        "ring",
        &[],
        "ringColor",
        ResolverKind::Color {
            properties: &["--tw-ring-color"],
            opacity_var: "--tw-ring-opacity",
            with_alpha_variable: true,
        },
        "ringColor",
    ),
    // ---- gradients (from / via / to + their positions) ----
    UVK(
        "from",
        &[],
        "gradientColorStops",
        false,
        ResolverKind::GradientFrom,
        "gradientColorStops",
    ),
    UV(
        "from",
        &["--tw-gradient-from-position"],
        "gradientColorStopPositions",
        false,
        "gradientColorStopPositions",
    ),
    UVK(
        "via",
        &[],
        "gradientColorStops",
        false,
        ResolverKind::GradientVia,
        "gradientColorStops",
    ),
    UV(
        "via",
        &["--tw-gradient-via-position"],
        "gradientColorStopPositions",
        false,
        "gradientColorStopPositions",
    ),
    UVK(
        "to",
        &[],
        "gradientColorStops",
        false,
        ResolverKind::GradientTo,
        "gradientColorStops",
    ),
    UV(
        "to",
        &["--tw-gradient-to-position"],
        "gradientColorStopPositions",
        false,
        "gradientColorStopPositions",
    ),
    // ---- divide-<color> (sibling-pair color) ----
    // Tailwind's divideColor plugin also strips DEFAULT from values
    // (mirrors the borderColor pattern), so bare `divide` never gets a
    // color rule — only divide-<color> candidates.
    UVK_FD(
        "divide",
        &[],
        "divideColor",
        ResolverKind::SiblingColor {
            opacity_var: "--tw-divide-opacity",
        },
        "divideColor",
    ),
    // ---- list-style ----
    UV(
        "list-image",
        &["list-style-image"],
        "listStyleImage",
        false,
        "listStyleImage",
    ),
    UV(
        "list",
        &["list-style-type"],
        "listStyleType",
        false,
        "listStyleType",
    ),
    // ---- background image / position / size ----
    // Share the `bg-` prefix with backgroundColor. Multi-match walks
    // all entries in declaration order; the resolvers reject shapes
    // that aren't theirs (color resolver refuses image/position
    // values, image resolver refuses non-image arbitrary values, ...)
    // so the first one whose shape check passes wins.
    UV(
        "bg",
        &["background-image"],
        "backgroundImage",
        false,
        "backgroundImage",
    ),
    UV(
        "bg",
        &["background-position"],
        "backgroundPosition",
        false,
        "backgroundPosition",
    ),
    UV(
        "bg",
        &["background-size"],
        "backgroundSize",
        false,
        "backgroundSize",
    ),
];

/// The `transform` shorthand value Tailwind 3.4.19 emits for every
/// transform utility (verbatim from `corePlugins.js`'s
/// `cssTransformValue`). Each transform plugin updates its own
/// `--tw-*` var and re-asserts this whole shorthand so the
/// composition stays consistent across siblings.
pub const CSS_TRANSFORM_VALUE: &str = "translate(var(--tw-translate-x), var(--tw-translate-y)) \
     rotate(var(--tw-rotate)) \
     skewX(var(--tw-skew-x)) \
     skewY(var(--tw-skew-y)) \
     scaleX(var(--tw-scale-x)) \
     scaleY(var(--tw-scale-y))";

/// The `filter` shorthand. Mirrors `cssFilterValue` in upstream
/// `corePlugins.js`.
pub const CSS_FILTER_VALUE: &str = "var(--tw-blur) var(--tw-brightness) var(--tw-contrast) \
     var(--tw-grayscale) var(--tw-hue-rotate) var(--tw-invert) \
     var(--tw-saturate) var(--tw-sepia) var(--tw-drop-shadow)";

/// The `backdrop-filter` shorthand. Mirrors `cssBackdropFilterValue`
/// in upstream `corePlugins.js`.
pub const CSS_BACKDROP_FILTER_VALUE: &str =
    "var(--tw-backdrop-blur) var(--tw-backdrop-brightness) \
     var(--tw-backdrop-contrast) var(--tw-backdrop-grayscale) \
     var(--tw-backdrop-hue-rotate) var(--tw-backdrop-invert) \
     var(--tw-backdrop-opacity) var(--tw-backdrop-saturate) \
     var(--tw-backdrop-sepia)";

#[allow(non_snake_case)]
const fn UV(
    class_prefix: &'static str,
    properties: &'static [&'static str],
    theme_key: &'static str,
    supports_negative: bool,
    plugin: &'static str,
) -> ValueUtility {
    UVK(
        class_prefix,
        properties,
        theme_key,
        supports_negative,
        ResolverKind::Simple,
        plugin,
    )
}

#[allow(non_snake_case)]
const fn UVK(
    class_prefix: &'static str,
    properties: &'static [&'static str],
    theme_key: &'static str,
    supports_negative: bool,
    kind: ResolverKind,
    plugin: &'static str,
) -> ValueUtility {
    ValueUtility {
        class_prefix,
        properties,
        theme_key,
        supports_negative,
        kind,
        plugin,
        filter_default: false,
    }
}

/// Same as `UV` but with `filter_default: true` — bare-prefix candidate
/// (no `-<value>` suffix) is rejected even if theme has `DEFAULT`.
/// Used for the `transitionDuration` / `transitionTimingFunction`
/// plugins which set `{ filterDefault: true }` in upstream's
/// `corePlugins.js`.
#[allow(non_snake_case)]
const fn UV_FD(
    class_prefix: &'static str,
    properties: &'static [&'static str],
    theme_key: &'static str,
    plugin: &'static str,
) -> ValueUtility {
    ValueUtility {
        class_prefix,
        properties,
        theme_key,
        supports_negative: false,
        kind: ResolverKind::Simple,
        plugin,
        filter_default: true,
    }
}

/// Same as `UVK` but with `filter_default: true`. Used for color
/// utilities that strip the `DEFAULT` key from their values
/// (e.g. borderColor, divideColor) so the bare prefix class
/// (e.g. `border`, `divide`) never gets a color declaration.
#[allow(non_snake_case)]
const fn UVK_FD(
    class_prefix: &'static str,
    properties: &'static [&'static str],
    theme_key: &'static str,
    kind: ResolverKind,
    plugin: &'static str,
) -> ValueUtility {
    ValueUtility {
        class_prefix,
        properties,
        theme_key,
        supports_negative: false,
        kind,
        plugin,
        filter_default: true,
    }
}

/// Attempt to resolve `parsed.root` as a value-bearing utility. Returns
/// every plugin that claims the prefix, in declaration order, paired
/// with the *value key* (the slice after `<prefix>-`, or `"DEFAULT"`
/// for the bare-prefix case `m` / `p`).
///
/// Multiple matches are intentional: `font-` is shared between
/// `fontWeight` and `fontFamily`, and `text-` is shared between
/// `fontSize` and (eventually) `textColor`. The compiler walks the
/// returned candidates and uses the first one whose theme lookup
/// succeeds — exactly how Tailwind's plugin pipeline behaves.
pub fn find_value_utilities(root: &str) -> Vec<(&'static ValueUtility, &str)> {
    // Walk `root` from end to start, splitting on each `-` that could
    // be the boundary between a prefix and a value-key. At each step
    // probe the prefix index. The first hit is the longest matching
    // prefix; collect every utility that registered against it (some
    // prefixes are shared — `font-` is fontWeight + fontFamily).
    //
    // The fast path: `bg-red-500` first probes `bg-red`, miss; then
    // `bg`, hit — two index lookups, no string allocations. Replaces
    // the original linear scan over ~150 entries that did a `format!`
    // per probe.
    //
    // We also probe the bare root (`flex` -> prefix `flex`, key
    // `"DEFAULT"`) in case the candidate is a no-modifier utility.
    let bytes = root.as_bytes();
    let prefix_index = prefix_index();

    if let Some(uts) = prefix_index.get(root) {
        // Bare prefix match: `flex`, `m`, etc. Honor `filter_default`
        // so plugins like `ease` / `duration` (which set it in
        // upstream) don't expose the DEFAULT key as a bare-prefix
        // utility.
        let filtered: Vec<(&'static ValueUtility, &str)> = uts
            .iter()
            .filter(|u| !u.filter_default)
            .map(|u| (*u, "DEFAULT"))
            .collect();
        if !filtered.is_empty() {
            return filtered;
        }
    }

    // Walk `-` positions from the right (longest-prefix-first) without
    // collecting them into a Vec. `bg-red-500` has `-` at indices 2
    // and 6; we probe `bg-red` first, then `bg`. Most class names
    // have ≤3 hyphens so the inner loop runs ≤3 times — no need for
    // a sorted heap or vec of probes.
    let mut i = bytes.len();
    while i > 0 {
        i -= 1;
        if bytes[i] == b'-' {
            let prefix = &root[..i];
            if let Some(uts) = prefix_index.get(prefix) {
                let value_key = &root[i + 1..];
                return uts.iter().map(|u| (*u, value_key)).collect();
            }
        }
    }
    Vec::new()
}

/// Returns the index in `VALUE_UTILITIES` of the FIRST entry with
/// the given class prefix. Tailwind v3 emits a plugin's utilities in
/// declaration order (e.g. `inset` corePlugin emits `inset, inset-x,
/// inset-y, start, end, top, right, bottom, left` in that exact
/// sequence). Mirroring that order requires within-plugin sort keys;
/// this index is the one we use as the secondary sort axis for
/// value-utility candidates. None for unknown prefixes.
pub fn value_table_index(class_prefix: &str) -> Option<u32> {
    // Group-level override first: prefixes from the same
    // `createUtilityPlugin` group (which become a single
    // `matchUtilities` call in upstream) share an index so they
    // tie-break alphabetically on `input_index`. Upstream emits e.g.
    // `border-b-4` before `border-t-4` (b < t) within the
    // `[border-s, border-e, border-t, border-r, border-b, border-l]`
    // group, even though the createUtilityPlugin declaration order
    // is `t, r, b, l`. Mirrors
    // `vendor/tailwindcss-v3/src/util/createUtilityPlugin.js`'s
    // single-`matchUtilities`-per-group behaviour.
    if let Some(g) = group_index_map().get(class_prefix) {
        return Some(*g);
    }
    value_table_index_map().get(class_prefix).copied()
}

/// Manually-maintained map from `class_prefix` to a group index that
/// mirrors upstream's `createUtilityPlugin([…])` grouping. Entries in
/// the same group share an index so candidates from the same
/// `matchUtilities` call tie on `within_plugin_order` and fall through
/// to alphabetic `input_index`. The base values are spaced (10, 20,
/// 30…) so each grouped family has a distinct slot while still
/// admitting more groups between existing ones if needed.
fn group_index_map() -> &'static rustc_hash::FxHashMap<&'static str, u32> {
    static MAP: std::sync::OnceLock<rustc_hash::FxHashMap<&'static str, u32>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| {
        let mut m: rustc_hash::FxHashMap<&'static str, u32> = rustc_hash::FxHashMap::default();
        // padding (`createUtilityPlugin('padding', [
        //   ['p'],
        //   [['px'], ['py']],
        //   [['ps'], ['pe'], ['pt'], ['pr'], ['pb'], ['pl']]
        // ])`)
        m.insert("p", 10);
        m.insert("px", 20);
        m.insert("py", 20);
        m.insert("ps", 30);
        m.insert("pe", 30);
        m.insert("pt", 30);
        m.insert("pr", 30);
        m.insert("pb", 30);
        m.insert("pl", 30);
        // margin (same shape)
        m.insert("m", 10);
        m.insert("mx", 20);
        m.insert("my", 20);
        m.insert("ms", 30);
        m.insert("me", 30);
        m.insert("mt", 30);
        m.insert("mr", 30);
        m.insert("mb", 30);
        m.insert("ml", 30);
        // borderWidth (`createUtilityPlugin('borderWidth', [
        //   ['border'],
        //   [['border-x'], ['border-y']],
        //   [['border-s'], ['border-e'], ['border-t'], ['border-r'],
        //    ['border-b'], ['border-l']]
        // ])`) — and borderColor uses the SAME grouping pattern with
        // three sequential `matchUtilities` calls.
        m.insert("border", 10);
        m.insert("border-x", 20);
        m.insert("border-y", 20);
        m.insert("border-s", 30);
        m.insert("border-e", 30);
        m.insert("border-t", 30);
        m.insert("border-r", 30);
        m.insert("border-b", 30);
        m.insert("border-l", 30);
        // inset (`createUtilityPlugin('inset', [
        //   ['inset'],
        //   [['inset-x'], ['inset-y']],
        //   [['start'], ['end'], ['top'], ['right'], ['bottom'], ['left']]
        // ])`)
        m.insert("inset", 10);
        m.insert("inset-x", 20);
        m.insert("inset-y", 20);
        m.insert("start", 30);
        m.insert("end", 30);
        m.insert("top", 30);
        m.insert("right", 30);
        m.insert("bottom", 30);
        m.insert("left", 30);
        // scrollPadding (same shape as padding)
        m.insert("scroll-p", 10);
        m.insert("scroll-px", 20);
        m.insert("scroll-py", 20);
        m.insert("scroll-ps", 30);
        m.insert("scroll-pe", 30);
        m.insert("scroll-pt", 30);
        m.insert("scroll-pr", 30);
        m.insert("scroll-pb", 30);
        m.insert("scroll-pl", 30);
        // scrollMargin (same shape as margin)
        m.insert("scroll-m", 10);
        m.insert("scroll-mx", 20);
        m.insert("scroll-my", 20);
        m.insert("scroll-ms", 30);
        m.insert("scroll-me", 30);
        m.insert("scroll-mt", 30);
        m.insert("scroll-mr", 30);
        m.insert("scroll-mb", 30);
        m.insert("scroll-ml", 30);
        // borderRadius (`createUtilityPlugin('borderRadius', [
        //   ['rounded'],
        //   [['rounded-s'], ['rounded-e'], ['rounded-t'], ['rounded-r'],
        //    ['rounded-b'], ['rounded-l']],
        //   [['rounded-ss'], ['rounded-se'], ['rounded-ee'], ['rounded-es'],
        //    ['rounded-tl'], ['rounded-tr'], ['rounded-br'], ['rounded-bl']]
        // ])`)
        m.insert("rounded", 10);
        m.insert("rounded-s", 20);
        m.insert("rounded-e", 20);
        m.insert("rounded-t", 20);
        m.insert("rounded-r", 20);
        m.insert("rounded-b", 20);
        m.insert("rounded-l", 20);
        m.insert("rounded-ss", 30);
        m.insert("rounded-se", 30);
        m.insert("rounded-ee", 30);
        m.insert("rounded-es", 30);
        m.insert("rounded-tl", 30);
        m.insert("rounded-tr", 30);
        m.insert("rounded-br", 30);
        m.insert("rounded-bl", 30);
        // gap (`createUtilityPlugin('gap', [
        //   ['gap'], [['gap-x'], ['gap-y']]
        // ])`)
        m.insert("gap", 10);
        m.insert("gap-x", 20);
        m.insert("gap-y", 20);
        // space (matchUtilities({ 'space-x': fn, 'space-y': fn }))
        m.insert("space-x", 10);
        m.insert("space-y", 10);
        // divideWidth (single matchUtilities for divide-x + divide-y)
        m.insert("divide-x", 10);
        m.insert("divide-y", 10);
        // translate (`createUtilityPlugin('translate', [[['translate-x',…],['translate-y',…]]])` —
        // single matchUtilities call wraps both axes, so they share a wpo.
        // corePlugins.js: `translate: createUtilityPlugin('translate', [...one variation with 2 utils...])`.
        m.insert("translate-x", 10);
        m.insert("translate-y", 10);
        // skew (same shape as translate — single grouped matchUtilities).
        m.insert("skew-x", 10);
        m.insert("skew-y", 10);
        // scale (`createUtilityPlugin('scale', [['scale',…], [['scale-x',…],['scale-y',…]]])` —
        // standalone `scale` lives in its own variation (group 10), `scale-x` and
        // `scale-y` share a second variation (group 20).
        m.insert("scale", 10);
        m.insert("scale-x", 20);
        m.insert("scale-y", 20);
        m
    })
}

fn value_table_index_map() -> &'static rustc_hash::FxHashMap<&'static str, u32> {
    static IDX: std::sync::OnceLock<rustc_hash::FxHashMap<&'static str, u32>> =
        std::sync::OnceLock::new();
    IDX.get_or_init(|| {
        let mut m: rustc_hash::FxHashMap<&'static str, u32> = rustc_hash::FxHashMap::default();
        for (i, util) in VALUE_UTILITIES.iter().enumerate() {
            // First registration of each class_prefix wins — later
            // duplicates (rare, but exist for aliases) inherit the
            // earlier index.
            m.entry(util.class_prefix).or_insert(i as u32);
        }
        m
    })
}

/// Pre-built index from `class_prefix` to every utility that registered
/// against it. Built once on first access and cached for the life of the
/// process — `VALUE_UTILITIES` is a `&'static` slice and the prefixes
/// are `&'static str`, so the keys and values are all stable references.
///
/// Uses FxHashMap (rustc's hash) instead of the default SipHash. SipHash
/// has DoS-resistance properties we don't need at compile time and it's
/// 2-3x slower per lookup; for the hot path (every candidate hits this
/// map ≥1 time) the swap is straight win.
fn prefix_index() -> &'static rustc_hash::FxHashMap<&'static str, Vec<&'static ValueUtility>> {
    static PREFIX_INDEX: std::sync::OnceLock<
        rustc_hash::FxHashMap<&'static str, Vec<&'static ValueUtility>>,
    > = std::sync::OnceLock::new();
    PREFIX_INDEX.get_or_init(|| {
        let mut m: rustc_hash::FxHashMap<&'static str, Vec<&'static ValueUtility>> =
            rustc_hash::FxHashMap::default();
        for util in VALUE_UTILITIES {
            m.entry(util.class_prefix).or_default().push(util);
        }
        m
    })
}

/// Convenience for callers that only want the first match — useful when
/// the prefix is unambiguous (margin, padding, sizing).
#[cfg(test)]
pub fn find_value_utility(root: &str) -> Option<(&'static ValueUtility, &str)> {
    find_value_utilities(root).into_iter().next()
}

/// Resolve a value-bearing utility into one or more `(property, value)`
/// declarations.
///
/// `selector_suffix` is appended to the candidate's class selector
/// after variants run — used by sibling-pair plugins (space-x,
/// divide-x) that emit on `<class> > :not([hidden]) ~ :not([hidden])`
/// rather than on `<class>` directly.
///
/// `extra_keyframes` are top-level `@keyframes <name> { ... }` rules
/// the candidate brings with it (e.g. `animate-spin` ships
/// `@keyframes spin`). Each entry is `(name, body)` where `body` is
/// the inside of the at-rule. The compiler emits these once per name
/// at stylesheet scope, deduplicating across candidates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedDecls {
    pub decls: Vec<(String, String)>,
    pub selector_suffix: Option<&'static str>,
    pub extra_keyframes: Vec<(&'static str, &'static str)>,
    /// Keyframes sourced from the user's config (`theme.keyframes.<name>`)
    /// rather than from a static stub. Owned strings because the
    /// values come from the config JSON; we can't borrow them as
    /// `&'static`. Emitted alongside `extra_keyframes` at stylesheet
    /// scope.
    pub extra_keyframes_owned: Vec<(String, String)>,
    /// Cascade-var defaults groups this utility participates in
    /// (e.g. `["box-shadow"]`, `["transform"]`). Used under
    /// `experimental.optimizeUniversalDefaults` to inline the
    /// group's default declarations BEFORE the resolved decls,
    /// matching upstream's `@defaults <id>` marker mechanism. Empty
    /// when the utility doesn't depend on any cascade-var defaults.
    pub defaults_groups: &'static [&'static str],
}

impl From<Vec<(String, String)>> for ResolvedDecls {
    fn from(decls: Vec<(String, String)>) -> Self {
        Self {
            decls,
            selector_suffix: None,
            extra_keyframes: Vec::new(),
            extra_keyframes_owned: Vec::new(),
            defaults_groups: &[],
        }
    }
}

impl IntoIterator for ResolvedDecls {
    type Item = (String, String);
    type IntoIter = std::vec::IntoIter<(String, String)>;
    fn into_iter(self) -> Self::IntoIter {
        self.decls.into_iter()
    }
}

/// Format a `var(--tw-*-opacity)` call. By default (matching our
/// pinned Tailwind v3.4.19 target) emits `var(--tw-X-opacity, 1)`
/// with the fallback v3.4 added. When `__tailwindVersion: "3.3"` is
/// set on the config — by `compile()` from
/// `features.compat.tailwind_version` — emits `var(--tw-X-opacity)`
/// without the fallback, matching Tailwind v3.3.x byte-for-byte.
/// Functionally equivalent at runtime because the corresponding
/// `--tw-X-opacity: 1` declaration is always emitted alongside.
fn format_opacity_var(opacity_var: &str, config: Option<&Value>) -> String {
    if is_v33_compat(config) {
        format!("var({opacity_var})")
    } else {
        format!("var({opacity_var}, 1)")
    }
}

/// True when the user selected `tailwindVersion: '3.3'` compat mode.
/// All the v3.3-byte-equivalence behaviors gate on this single check.
pub(crate) fn is_v33_compat(config: Option<&Value>) -> bool {
    config
        .and_then(|c| c.get("__tailwindVersion"))
        .and_then(|v| v.as_str())
        == Some("3.3")
}

pub fn resolve_value(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
) -> Option<ResolvedDecls> {
    let decls_only = match util.kind {
        ResolverKind::Simple => resolve_simple(util, value_key, parsed, config),
        ResolverKind::FontSize => resolve_font_size(util, value_key, parsed, config),
        ResolverKind::FontFamily => resolve_font_family(util, value_key, parsed, config),
        ResolverKind::Color {
            properties,
            opacity_var,
            with_alpha_variable,
        } => resolve_color(
            util,
            value_key,
            parsed,
            config,
            properties,
            opacity_var,
            with_alpha_variable,
        ),
        ResolverKind::Transform { vars } => {
            resolve_transform(util, value_key, parsed, config, vars)
        }
        ResolverKind::Filter {
            var,
            wrap_fn,
            output_properties,
            composed,
            allow_empty,
        } => resolve_filter(
            util,
            value_key,
            parsed,
            config,
            var,
            wrap_fn,
            output_properties,
            composed,
            allow_empty,
        ),
        ResolverKind::TransitionProperty => {
            resolve_transition_property(util, value_key, parsed, config)
        }
        ResolverKind::Sibling { decls } => {
            return resolve_sibling(util, value_key, parsed, config, decls);
        }
        ResolverKind::SiblingColor { opacity_var } => {
            return resolve_sibling_color(util, value_key, parsed, config, opacity_var);
        }
        ResolverKind::PlaceholderColor => {
            return resolve_placeholder_color(util, value_key, parsed, config);
        }
        ResolverKind::SelectorVariable {
            selector_suffix,
            var,
        } => {
            return resolve_selector_variable(
                util,
                value_key,
                parsed,
                config,
                selector_suffix,
                var,
            );
        }
        ResolverKind::BorderSpacing { vars } => {
            resolve_border_spacing(util, value_key, parsed, config, vars)
        }
        ResolverKind::ContentVar => resolve_content_var(util, value_key, parsed, config),
        ResolverKind::BoxShadow => resolve_box_shadow(util, value_key, parsed, config),
        ResolverKind::BoxShadowColor => resolve_box_shadow_color(util, value_key, parsed, config),
        ResolverKind::RingWidth => resolve_ring_width(util, value_key, parsed, config),
        ResolverKind::GradientFrom => {
            resolve_gradient_color(util, value_key, parsed, config, GradientStop::From)
        }
        ResolverKind::GradientVia => {
            resolve_gradient_color(util, value_key, parsed, config, GradientStop::Via)
        }
        ResolverKind::GradientTo => {
            resolve_gradient_color(util, value_key, parsed, config, GradientStop::To)
        }
        ResolverKind::Animation => {
            return resolve_animation(util, value_key, parsed, config);
        }
        ResolverKind::LineClamp => resolve_line_clamp(util, value_key, parsed, config),
    };
    decls_only.map(|decls| {
        let mut r = ResolvedDecls::from(decls);
        r.defaults_groups = defaults_groups_for(util.theme_key);
        r
    })
}

/// Map a utility's theme-key to its cascade-var defaults groups.
/// Used by `experimental.optimizeUniversalDefaults` to inline the
/// group's default declarations BEFORE the utility's resolved
/// decls. Mirrors the `addDefaults('<group>', { ... })` calls in
/// upstream `corePlugins.js`.
fn defaults_groups_for(theme_key: &str) -> &'static [&'static str] {
    match theme_key {
        "boxShadow" | "boxShadowColor" => &["box-shadow"],
        "ringWidth" | "ringColor" | "ringOpacity" | "ringOffsetWidth" | "ringOffsetColor" => {
            &["ring-width"]
        }
        "translate" | "rotate" | "skew" | "scale" | "transformOrigin" => &["transform"],
        _ => &[],
    }
}

/// `line-clamp-<n>`. Resolves the theme key (1..=6 in the default
/// table) and emits the four-decl shape upstream's `lineClamp` plugin
/// produces. Modifier/negative are not supported (Tailwind rejects
/// them).
fn resolve_line_clamp(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
) -> Option<Vec<(String, String)>> {
    if parsed.modifier.is_some() || parsed.negative {
        return None;
    }
    let value = resolve_scalar(util, value_key, config)?;
    Some(vec![
        ("overflow".to_string(), "hidden".to_string()),
        ("display".to_string(), "-webkit-box".to_string()),
        ("-webkit-box-orient".to_string(), "vertical".to_string()),
        ("-webkit-line-clamp".to_string(), value),
    ])
}

/// `animate-<name>` plugin. Mirrors upstream's `animation` plugin in
/// `corePlugins.js`. Resolves `theme.animation.<name>` to the
/// `animation` shorthand value (e.g. `'spin 1s linear infinite'`),
/// extracts the animation NAME from the first whitespace-separated
/// token, and ships the matching `@keyframes <name>` block at
/// stylesheet scope.
fn resolve_animation(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
) -> Option<ResolvedDecls> {
    if parsed.modifier.is_some() || parsed.negative {
        return None;
    }
    let value = resolve_scalar(util, value_key, config)?;
    let mut extra: Vec<(&'static str, &'static str)> = Vec::new();
    let mut owned_extra: Vec<(String, String)> = Vec::new();
    let cfg_prefix: &str = config
        .and_then(|c| c.get("prefix"))
        .and_then(Value::as_str)
        .unwrap_or("");
    // Mirror upstream's `prefixName = (name) => escapeClassName(prefix + name)`
    // from the `animation` corePlugin: keyframes get the user prefix
    // applied AND the resulting name is selector-escaped (so dotted
    // animation names like `zoom-.5` survive as `zoom-\.5`).
    // Keyframe names rarely contain commas; the upstream
    // `escapeClassName` path runs `escapeCommas` so we use `Numeric`
    // for parity.
    let _ = config; // borrow check: config used elsewhere, kept for future
    let prefix_and_escape = |name: &str| -> String {
        let combined = format!("{cfg_prefix}{name}");
        galeforce_css::escape_class_name_with(&combined, galeforce_css::CommaEscapeStyle::Numeric)
    };
    let mut final_value = value.clone();
    if value != "none" {
        // Tailwind's `animation` shorthand may list multiple
        // animations separated by commas — `bounce 2s linear,
        // pulse 3s ease-in`. We need a `@keyframes` block for each
        // unique name, sourced from either the built-in stubs or
        // the user's `theme.keyframes` config.
        let mut rebuilt_parts: Vec<String> = Vec::new();
        for raw in split_top_level_commas(&value) {
            let name = first_token(raw);
            if name.is_empty() {
                rebuilt_parts.push(raw.to_string());
                continue;
            }
            let mut keyframe_name: Option<String> = None;
            if let Some((static_name, body)) = lookup_builtin_keyframes(name) {
                if cfg_prefix.is_empty() {
                    extra.push((static_name, body));
                } else {
                    let prefixed = prefix_and_escape(name);
                    owned_extra.push((prefixed.clone(), body.to_string()));
                    keyframe_name = Some(prefixed);
                }
            } else if let Some(cfg) = config {
                if let Some(kf) = cfg
                    .get("theme")
                    .and_then(|t| t.get("keyframes"))
                    .and_then(|k| k.get(name))
                {
                    if let Some(body) = serialize_keyframes_body(kf) {
                        let prefixed = prefix_and_escape(name);
                        owned_extra.push((prefixed.clone(), body));
                        keyframe_name = Some(prefixed);
                    }
                }
            }
            if let Some(prefixed) = keyframe_name {
                let new_part = raw.replacen(name, &prefixed, 1);
                rebuilt_parts.push(new_part);
            } else {
                rebuilt_parts.push(raw.to_string());
            }
        }
        final_value = rebuilt_parts.join(", ");
    }
    Some(ResolvedDecls {
        decls: vec![("animation".to_string(), final_value)],
        selector_suffix: None,
        extra_keyframes: extra,
        extra_keyframes_owned: owned_extra,
        defaults_groups: &[],
    })
}

/// Serialize a JSON keyframes object into a CSS body string. The
/// shape is `{ <selector>: { <property>: <value>, ... }, ... }` —
/// each top-level key is a selector (`to`, `from`, `50%`, `0%, 100%`)
/// and the value is a nested map of property -> value. Returns the
/// body text *between* the outer braces of `@keyframes <name> { ... }`.
fn serialize_keyframes_body(node: &Value) -> Option<String> {
    let map = node.as_object()?;
    let mut out = String::new();
    for (selector, decls) in map {
        let decls_obj = decls.as_object()?;
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(selector);
        out.push_str(" { ");
        for (prop, val) in decls_obj {
            let val_str = match val {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => continue,
            };
            // Convert camelCase property names (`animationTimingFunction`)
            // into kebab-case (`animation-timing-function`) since the
            // resolved JS theme keeps them camelCase. CSS property names
            // are kebab-case.
            out.push_str(&camel_to_kebab(prop));
            out.push_str(": ");
            out.push_str(&val_str);
            out.push_str("; ");
        }
        out.push('}');
    }
    Some(out)
}

fn camel_to_kebab(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('-');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn first_token(s: &str) -> &str {
    let trimmed = s.trim_start();
    match trimmed.find(char::is_whitespace) {
        Some(i) => &trimmed[..i],
        None => trimmed,
    }
}

/// Built-in `@keyframes` blocks that ship with the four default
/// animations (`spin`, `ping`, `pulse`, `bounce`). Each body is the
/// CSS *between* the outer braces of `@keyframes <name> { ... }`.
/// Returns `(static_name, body)` so the caller can store both in a
/// `&'static str`-keyed structure without lifetime gymnastics.
/// Vendored verbatim from `vendor/tailwindcss-v3/stubs/config.full.js`'s
/// `keyframes` block.
fn lookup_builtin_keyframes(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "spin" => Some(("spin", "to { transform: rotate(360deg) }")),
        "ping" => Some((
            "ping",
            "75%, 100% { transform: scale(2); opacity: 0 }",
        )),
        "pulse" => Some(("pulse", "50% { opacity: .5 }")),
        "bounce" => Some((
            "bounce",
            // No spaces between the cubic-bezier arguments — matches
            // upstream's stub authoring (`cubic-bezier(0.8,0,1,1)`).
            "0%, 100% { transform: translateY(-25%); animation-timing-function: cubic-bezier(0.8,0,1,1) } \
             50% { transform: none; animation-timing-function: cubic-bezier(0,0,0.2,1) }",
        )),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GradientStop {
    From,
    Via,
    To,
}

/// Resolve `from-<color>` / `via-<color>` / `to-<color>`. Mirrors
/// upstream `gradientColorStops` plugin's per-stop matchers. The
/// `from` and `via` paths emit the auxiliary `--tw-gradient-to` (set
/// to a transparent version of the same color) so subsequent
/// `to-<color>` candidates can override only when explicitly provided
/// — this is what upstream calls `transparentTo` /
/// `withAlphaValue(value, 0, 'rgb(255 255 255 / 0)')`.
fn resolve_gradient_color(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
    stop: GradientStop,
) -> Option<Vec<(String, String)>> {
    if parsed.negative {
        return None;
    }
    // Resolve the raw color string — same paths as resolve_color but
    // we end up writing custom-property values, not CSS color decls.
    let raw = if let Some(arb) = arbitrary_inner(value_key) {
        normalize_arbitrary_with_config(arb, config)
    } else {
        let v = lookup_theme_value(util, value_key, config)?;
        value_to_string(&v)?
    };

    // Apply opacity modifier as inline alpha when the value is hex.
    let alpha = parsed
        .modifier
        .as_ref()
        .and_then(|m| resolve_opacity_modifier(m, config));
    let color_value = if is_color_keyword(&raw) {
        if parsed.modifier.is_some() {
            return None;
        }
        raw.clone()
    } else if let Some((r, g, b)) = parse_hex(&raw) {
        if let Some(a) = alpha.clone() {
            format!("rgb({r} {g} {b} / {a})")
        } else if parsed.modifier.is_some() {
            return None;
        } else {
            raw.clone()
        }
    } else if parsed.modifier.is_some() {
        return None;
    } else {
        raw.clone()
    };

    // The "transparent same color" used in --tw-gradient-to fallback.
    // Mirrors upstream's `transparentTo(value)` =
    // `withAlphaValue(value, 0, 'rgb(255 255 255 / 0)')`. Tailwind's
    // color parser treats `transparent` as black with alpha=0, so
    // it round-trips as `rgb(0 0 0 / 0)`. Other keywords (`currentColor`,
    // `inherit`, …) aren't parseable colors so the fallback applies.
    let transparent = if let Some((r, g, b)) = parse_hex(&raw) {
        format!("rgb({r} {g} {b} / 0)")
    } else if raw == "transparent" {
        "rgb(0 0 0 / 0)".to_string()
    } else {
        "rgb(255 255 255 / 0)".to_string()
    };

    match stop {
        GradientStop::From => Some(vec![
            (
                "--tw-gradient-from".into(),
                format!("{color_value} var(--tw-gradient-from-position)"),
            ),
            (
                "--tw-gradient-to".into(),
                format!("{transparent} var(--tw-gradient-to-position)"),
            ),
            (
                "--tw-gradient-stops".into(),
                "var(--tw-gradient-from), var(--tw-gradient-to)".into(),
            ),
        ]),
        GradientStop::Via => Some(vec![
            // Note the double space between the transparent-to and
            // the var — present in upstream's emitted output (looks
            // like a typo in `corePlugins.js` but we mirror exactly).
            (
                "--tw-gradient-to".into(),
                format!("{transparent}  var(--tw-gradient-to-position)"),
            ),
            (
                "--tw-gradient-stops".into(),
                format!(
                    "var(--tw-gradient-from), {color_value} var(--tw-gradient-via-position), var(--tw-gradient-to)"
                ),
            ),
        ]),
        GradientStop::To => Some(vec![(
            "--tw-gradient-to".into(),
            format!("{color_value} var(--tw-gradient-to-position)"),
        )]),
    }
}

/// `shadow-<size>` plugin body. The "colored" variant is the input
/// shadow with every embedded color literal replaced by
/// `var(--tw-shadow-color)`. Mirrors upstream's
/// `parseBoxShadowValue` + `formatBoxShadowValue` round-trip.
fn resolve_box_shadow(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
) -> Option<Vec<(String, String)>> {
    if parsed.modifier.is_some() || parsed.negative {
        return None;
    }
    // Type discrimination: arbitrary `shadow-[#hex]` and rgb() forms
    // are colors, not sizes. Fall through so the boxShadowColor
    // sibling plugin claims them. Mirrors upstream's
    // `type: ['shadow']` filter on the `boxShadow` matcher.
    if let Some(arb) = arbitrary_inner(value_key) {
        let probe = normalize_arbitrary_with_config(arb, config);
        if probe.starts_with('#') || starts_with_color_function(&probe) {
            return None;
        }
        if let Some((hint, _)) = data_type_hint(&probe) {
            // Color-hinted arbitrary values fall through to the
            // boxShadowColor sibling plugin.
            if hint == "color" {
                return None;
            }
        }
        // boxShadow's matchUtilities has `type: ['shadow']` only —
        // unhinted `var(...)` falls through to boxShadowColor (which
        // has `['color', 'any']`). Without this rejection
        // `shadow-[var(...)]` would emit the box-shadow shorthand.
        if probe.trim_start().starts_with("var(") {
            return None;
        }
    }
    let raw = if let Some(arb) = arbitrary_inner(value_key) {
        let probe = normalize_arbitrary_with_config(arb, config);
        // Strip a `shadow:` data-type hint — upstream advertises
        // `type: ['shadow']` on the boxShadow matcher, which means
        // the user can disambiguate with `shadow:var(--x)`. The
        // hint isn't part of the emitted value.
        if let Some((hint, body)) = data_type_hint(&probe) {
            if hint == "shadow" {
                body.to_string()
            } else {
                probe
            }
        } else {
            probe
        }
    } else {
        let v = lookup_theme_value(util, value_key, config)?;
        // theme.boxShadow values are sometimes arrays (multiple
        // shadows) but Tailwind's stub uses comma-joined strings;
        // both forms are acceptable for the resolver.
        match v {
            Value::String(s) => s.clone(),
            Value::Array(items) => items
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect::<Vec<_>>()
                .join(", "),
            _ => return None,
        }
    };
    let (raw_value, colored) = if raw == "none" {
        ("0 0 #0000".to_string(), "0 0 #0000".to_string())
    } else {
        (raw.clone(), substitute_shadow_colors(&raw))
    };
    Some(vec![
        ("--tw-shadow".to_string(), raw_value),
        ("--tw-shadow-colored".to_string(), colored),
        (
            "box-shadow".to_string(),
            "var(--tw-ring-offset-shadow, 0 0 #0000), var(--tw-ring-shadow, 0 0 #0000), var(--tw-shadow)"
                .to_string(),
        ),
    ])
}

/// Replace every embedded color literal in a box-shadow value with
/// `var(--tw-shadow-color)`. Lightweight port of upstream's
/// `parseBoxShadowValue` + `formatBoxShadowValue`: split on
/// top-level commas, then within each segment swap any rgb()/rgba()/
/// hsl()/hsla() function call, hex literal, or named color keyword
/// for the var. Length tokens (numbers + unit) and the `inset`
/// keyword pass through.
fn substitute_shadow_colors(value: &str) -> String {
    let segments = split_top_level_commas(value);
    segments
        .iter()
        .map(|seg| substitute_shadow_segment(seg.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn split_top_level_commas(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b',' if depth == 0 => {
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

/// Within a single shadow segment, replace or append `var(--tw-shadow-color)`.
///
/// - If the segment contains a color literal (hex, rgb(), named color), replace
///   it with `var(--tw-shadow-color)`.
/// - If the segment has NO color literal (e.g. `14px 17px 40px 4px` or
///   `inset 0px 18px 22px`), append `var(--tw-shadow-color)` at the end.
///   This mirrors upstream's `parseBoxShadowValue` which always sets
///   `shadow.color = 'var(--tw-shadow-color)'`, adding the field when absent.
/// - If the segment is an unwrapped `var(--name)` (no fallback), it is treated
///   as an opaque/invalid shadow and returned unchanged — mirrors upstream's
///   `!shadow.valid` guard that skips color injection for bare CSS variables.
///
/// Fallback unwrapping: `var(--name, <shadow>)` recurses into the fallback
/// so `var(--a, 0 35px 60px -15px rgba(0,0,0))` →
/// `0 35px 60px -15px var(--tw-shadow-color)`.
fn substitute_shadow_segment(seg: &str) -> String {
    if let Some(fallback) = unwrap_var_fallback(seg) {
        return substitute_shadow_segment(fallback.trim());
    }
    // Bare `var(--name)` with no fallback: treat as opaque/invalid shadow,
    // matching upstream's `!shadow.valid` guard. Return unchanged.
    let trimmed = seg.trim();
    if trimmed.starts_with("var(") && unwrap_var_fallback(trimmed).is_none() {
        return trimmed.to_string();
    }
    let tokens = split_segment_tokens(seg);
    let mut out_parts: Vec<String> = Vec::with_capacity(tokens.len() + 1);
    let mut color_replaced = false;
    for tok in tokens.iter() {
        if !color_replaced && is_shadow_color_token(tok) {
            out_parts.push("var(--tw-shadow-color)".to_string());
            color_replaced = true;
        } else {
            out_parts.push(tok.clone());
        }
    }
    // If no hex/named/rgb color was found, look for a `var(...)` call.
    // Tailwind treats a `var()` in a shadow segment as the color slot
    // when there's no explicit color elsewhere — common shape:
    // `inset 0 0 0 1px var(--my-color)`. Replace IN PLACE so the
    // colored form reads `inset 0 0 0 1px var(--tw-shadow-color)`.
    if !color_replaced {
        if let Some(idx) = out_parts
            .iter()
            .rposition(|t| t.starts_with("var(") && t.ends_with(')'))
        {
            out_parts[idx] = "var(--tw-shadow-color)".to_string();
            color_replaced = true;
        }
    }
    // Upstream always sets shadow.color — if neither a color literal
    // nor a var() was found, append.
    if !color_replaced {
        out_parts.push("var(--tw-shadow-color)".to_string());
    }
    out_parts.join(" ")
}

/// If `seg` is exactly `var(--name, <fallback>)` (no leading or
/// trailing tokens around the var call), return `<fallback>`.
/// Otherwise `None`. Used by the box-shadow colored-form pass to
/// recurse into a var() fallback that wraps a real shadow value.
fn unwrap_var_fallback(seg: &str) -> Option<&str> {
    let s = seg.trim();
    let rest = s.strip_prefix("var(")?.strip_suffix(')')?;
    // Find the first top-level comma — splits the var()'s name from
    // its fallback.
    let bytes = rest.as_bytes();
    let mut depth = 0i32;
    let mut comma: Option<usize> = None;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'(' | b'[' => {
                depth += 1;
                i += 1;
            }
            b')' | b']' => {
                depth -= 1;
                i += 1;
            }
            b',' if depth == 0 => {
                comma = Some(i);
                break;
            }
            _ => i += 1,
        }
    }
    let comma = comma?;
    Some(&rest[comma + 1..])
}

fn split_segment_tokens(seg: &str) -> Vec<String> {
    let bytes = seg.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b' ' | b'\t' if depth == 0 => {
                if i > start {
                    out.push(seg[start..i].to_string());
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < bytes.len() {
        out.push(seg[start..].to_string());
    }
    out.into_iter().filter(|t| !t.is_empty()).collect()
}

fn is_shadow_color_token(token: &str) -> bool {
    if token.starts_with('#') {
        return true;
    }
    // rgb()/rgba()/hsl()/hsla()/hwb()/oklch()/etc. function calls.
    if let Some(open) = token.find('(') {
        let ident: String = token[..open].chars().flat_map(char::to_lowercase).collect();
        if matches!(
            ident.as_str(),
            "rgb" | "rgba" | "hsl" | "hsla" | "hwb" | "lab" | "lch" | "oklab" | "oklch" | "color"
        ) {
            return true;
        }
    }
    // Numeric / length tokens are NOT colors.
    let first_byte = token.as_bytes().first().copied();
    if matches!(first_byte, Some(b'-') | Some(b'+'))
        || matches!(first_byte, Some(b) if b.is_ascii_digit())
        || token.starts_with('.')
    {
        return false;
    }
    // Keywords that aren't colors.
    if matches!(token, "inset" | "inherit" | "initial" | "revert" | "unset") {
        return false;
    }
    // Otherwise treat as a named color (red, blue, currentColor, …).
    token.chars().all(|c| c.is_ascii_alphabetic())
}

/// `shadow-<color>` plugin body. Emits `--tw-shadow-color: <hex|rgb>`
/// and `--tw-shadow: var(--tw-shadow-colored)`. Mirrors upstream's
/// `boxShadowColor: ({matchUtilities, theme}) => matchUtilities(
///   { shadow: (value) => ({ '--tw-shadow-color': toColorValue(value),
///                           '--tw-shadow': 'var(--tw-shadow-colored)' }) })`.
fn resolve_box_shadow_color(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
) -> Option<Vec<(String, String)>> {
    if parsed.negative {
        return None;
    }
    let raw = if let Some(arb) = arbitrary_inner(value_key) {
        normalize_arbitrary_with_config(arb, config)
    } else {
        let v = lookup_theme_value(util, value_key, config)?;
        value_to_string(&v)?
    };
    // Apply the modifier as inline alpha when the value is a hex.
    let alpha = parsed
        .modifier
        .as_ref()
        .and_then(|m| resolve_opacity_modifier(m, config));
    let resolved_color = if let Some((r, g, b)) = parse_hex(&raw) {
        if let Some(a) = alpha {
            format!("rgb({r} {g} {b} / {a})")
        } else if parsed.modifier.is_some() {
            return None;
        } else {
            raw.clone()
        }
    } else if parsed.modifier.is_some() {
        return None;
    } else {
        raw.clone()
    };
    Some(vec![
        ("--tw-shadow-color".to_string(), resolved_color),
        (
            "--tw-shadow".to_string(),
            "var(--tw-shadow-colored)".to_string(),
        ),
    ])
}

/// `ring` / `ring-N` plugin body. Emits the cascade-bridging custom
/// properties and the composed `box-shadow` shorthand per upstream's
/// `ringWidth` plugin.
fn resolve_ring_width(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
) -> Option<Vec<(String, String)>> {
    if parsed.modifier.is_some() || parsed.negative {
        return None;
    }
    // Type discrimination — `ring-[#hex]` is a color, not a width.
    // Mirrors upstream's `type: 'length'` filter on the ringWidth
    // matchUtilities, which dispatches `color:`-hinted values to
    // the sibling ringColor plugin and accepts only length-shaped
    // arbitrary values.
    let value = if let Some(arb) = arbitrary_inner(value_key) {
        let probe = normalize_arbitrary_with_config(arb, config);
        if probe.starts_with('#') || starts_with_color_function(&probe) {
            return None;
        }
        if is_color_value(&probe) {
            return None;
        }
        if let Some((hint, body)) = data_type_hint(&probe) {
            if matches!(hint, "color" | "image" | "url") {
                return None;
            }
            // Strip `length:` / `percentage:` etc. — the body is the
            // emitted width.
            body.to_string()
        } else if has_top_level_colon(&probe) {
            return None;
        } else if probe.trim_start().starts_with("var(") {
            // ringWidth's matchUtilities has `type: 'length'` only.
            // Unhinted `var(...)` falls through to ringColor (with
            // `['color', 'any']`).
            return None;
        } else {
            probe
        }
    } else {
        resolve_scalar(util, value_key, config)?
    };
    Some(vec![
        (
            "--tw-ring-offset-shadow".to_string(),
            "var(--tw-ring-inset) 0 0 0 var(--tw-ring-offset-width) var(--tw-ring-offset-color)"
                .to_string(),
        ),
        (
            "--tw-ring-shadow".to_string(),
            format!(
                "var(--tw-ring-inset) 0 0 0 calc({value} + var(--tw-ring-offset-width)) var(--tw-ring-color)"
            ),
        ),
        (
            "box-shadow".to_string(),
            "var(--tw-ring-offset-shadow), var(--tw-ring-shadow), var(--tw-shadow, 0 0 #0000)"
                .to_string(),
        ),
    ])
}

/// `space-x-*` / `space-y-*` / `divide-x-*` / `divide-y-*` body
/// generator. Substitutes the resolved value into each entry's
/// format string and tags the result with the
/// ` > :not([hidden]) ~ :not([hidden])` selector suffix.
fn resolve_sibling(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
    decls_template: &[(&'static str, &'static str)],
) -> Option<ResolvedDecls> {
    if parsed.modifier.is_some() {
        return None;
    }
    let resolved = resolve_scalar(util, value_key, config)?;
    let value = if parsed.negative {
        if !util.supports_negative || !is_negatable(&resolved) {
            return None;
        }
        negate(&resolved)
    } else {
        resolved
    };
    // Tailwind's space plugin special-cases `0` -> `0px` for the
    // calc subtraction (so `calc(0px * ...)` rather than `calc(0 * ...)`).
    let value = if value == "0" {
        "0px".to_string()
    } else {
        value
    };
    let decls: Vec<(String, String)> = decls_template
        .iter()
        .map(|(prop, fmt)| ((*prop).to_string(), fmt.replace("{}", &value)))
        .collect();
    Some(ResolvedDecls {
        decls,
        selector_suffix: Some(" > :not([hidden]) ~ :not([hidden])"),
        extra_keyframes: Vec::new(),
        extra_keyframes_owned: Vec::new(),
        defaults_groups: &[],
    })
}

/// `divide-<color>` — runs the standard color resolver against
/// `theme.divideColor` (which defaults to `theme.borderColor` per
/// upstream), then tags the sibling-pair selector suffix.
fn resolve_sibling_color(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
    opacity_var: &'static str,
) -> Option<ResolvedDecls> {
    let inner_decls = resolve_color(
        util,
        value_key,
        parsed,
        config,
        &["border-color"],
        opacity_var,
        true,
    )?;
    Some(ResolvedDecls {
        decls: inner_decls,
        selector_suffix: Some(" > :not([hidden]) ~ :not([hidden])"),
        extra_keyframes: Vec::new(),
        extra_keyframes_owned: Vec::new(),
        defaults_groups: &[],
    })
}

/// `placeholder-opacity-<n>` / `divide-opacity-<n>` and friends:
/// resolves a scalar from theme and emits one decl `<var>: <value>`
/// on `<class><selector_suffix>`. Mirrors the matchUtilities calls
/// in upstream's `placeholderOpacity` and `divideOpacity` plugins.
/// Negation/modifier are not supported (Tailwind rejects them).
fn resolve_selector_variable(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
    selector_suffix: &'static str,
    var: &'static str,
) -> Option<ResolvedDecls> {
    if parsed.modifier.is_some() || parsed.negative {
        return None;
    }
    let resolved = resolve_scalar(util, value_key, config)?;
    Some(ResolvedDecls {
        decls: vec![(var.to_string(), resolved)],
        selector_suffix: Some(selector_suffix),
        extra_keyframes: Vec::new(),
        extra_keyframes_owned: Vec::new(),
        defaults_groups: &[],
    })
}

/// `placeholder-<color>` — runs the standard color resolver against
/// `theme.placeholderColor` (defaults to `theme.colors`), then tags
/// the rule's selector with `::placeholder`. Mirrors upstream's
/// `placeholderColor` plugin.
fn resolve_placeholder_color(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
) -> Option<ResolvedDecls> {
    let inner_decls = resolve_color(
        util,
        value_key,
        parsed,
        config,
        &["color"],
        "--tw-placeholder-opacity",
        true,
    )?;
    Some(ResolvedDecls {
        decls: inner_decls,
        selector_suffix: Some("::placeholder"),
        extra_keyframes: Vec::new(),
        extra_keyframes_owned: Vec::new(),
        defaults_groups: &[],
    })
}

/// `content-<value>` — emits `--tw-content: <value>; content:
/// var(--tw-content)`. Resolves through `theme.content` (defaults
/// `none`) or accepts arbitrary values verbatim. Tailwind drops the
/// degenerate `content-[""]` form (the arbitrary inner is just two
/// quote chars with nothing between them) — match that.
fn resolve_content_var(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
) -> Option<Vec<(String, String)>> {
    if parsed.modifier.is_some() || parsed.negative {
        return None;
    }
    let value = if let Some(arb) = arbitrary_inner(value_key) {
        // Double-quoted string arbitrary values (`content-[""]`,
        // `content-["hello"]`) are accepted — Tailwind v3.3 and v3.4
        // both emit the `--tw-content: "<str>"` decl for them. The
        // string passes through verbatim including the surrounding
        // quotes.
        normalize_arbitrary_with_config(arb, config)
    } else {
        let v = lookup_theme_value(util, value_key, config)?;
        value_to_string(&v)?
    };
    Some(vec![
        ("--tw-content".to_string(), value),
        ("content".to_string(), "var(--tw-content)".to_string()),
    ])
}

/// Resolve a `transition-<key>` candidate. Mirrors upstream's
/// `transitionProperty` plugin: emits `transition-property: <value>`
/// alone for `none`, or `transition-property` + default
/// `transition-timing-function` + `transition-duration` otherwise.
fn resolve_transition_property(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
) -> Option<Vec<(String, String)>> {
    if parsed.modifier.is_some() || parsed.negative {
        return None;
    }
    let value = resolve_scalar(util, value_key, config)?;
    let mut out: Vec<(String, String)> = vec![("transition-property".into(), value.clone())];
    if value != "none" {
        // Defaults from `theme.transitionTimingFunction.DEFAULT` and
        // `theme.transitionDuration.DEFAULT`. Resolved through the
        // same fallback as the rest of the value plugins.
        if let Some(timing) = lookup_default(config, "transitionTimingFunction") {
            out.push(("transition-timing-function".into(), timing));
        }
        if let Some(duration) = lookup_default(config, "transitionDuration") {
            out.push(("transition-duration".into(), duration));
        }
    }
    Some(out)
}

fn lookup_default(config: Option<&Value>, theme_key: &str) -> Option<String> {
    let path = format!("{theme_key}.DEFAULT");
    if let Some(s) = config
        .and_then(|c| c.get("theme"))
        .and_then(|t| lookup_theme(t, &path))
        .and_then(value_to_string)
    {
        return Some(s);
    }
    let dt = default_theme_ref();
    lookup_theme(dt, &path).and_then(value_to_string)
}

#[allow(clippy::too_many_arguments)]
fn resolve_filter(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
    var: &str,
    wrap_fn: &str,
    output_properties: &[&str],
    composed: &str,
    allow_empty: bool,
) -> Option<Vec<(String, String)>> {
    if parsed.modifier.is_some() {
        return None;
    }
    // Arbitrary `[…]` short-circuits to a scalar.
    let var_value = if let Some(arb) = arbitrary_inner(value_key) {
        let value = normalize_arbitrary_with_config(arb, config);
        let value = if parsed.negative {
            if !util.supports_negative || !is_negatable(&value) {
                return None;
            }
            negate(&value)
        } else {
            value
        };
        if value.trim().is_empty() && allow_empty {
            " ".to_string()
        } else {
            format!("{wrap_fn}({value})")
        }
    } else {
        // Theme lookup — may be a string OR an array (drop-shadow).
        // Per `corePlugins.js`'s dropShadow plugin:
        //   '--tw-drop-shadow': Array.isArray(value)
        //     ? value.map((v) => `drop-shadow(${v})`).join(' ')
        //     : `drop-shadow(${value})`,
        let theme_value = lookup_theme_value(util, value_key, config)?;
        match theme_value {
            Value::Array(items) => {
                let strs: Vec<String> = items
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| format!("{wrap_fn}({s})")))
                    .collect();
                if strs.is_empty() {
                    return None;
                }
                strs.join(" ")
            }
            Value::String(s) => {
                let value = if parsed.negative {
                    if !util.supports_negative || !is_negatable(&s) {
                        return None;
                    }
                    negate(&s)
                } else {
                    s.clone()
                };
                if value.trim().is_empty() && allow_empty {
                    " ".to_string()
                } else {
                    format!("{wrap_fn}({value})")
                }
            }
            Value::Number(n) => format!("{wrap_fn}({n})"),
            _ => return None,
        }
    };
    let mut out: Vec<(String, String)> = vec![(var.to_string(), var_value)];
    let v33 = is_v33_compat(config);
    for prop in output_properties {
        // Tailwind v3.3 emits only the unprefixed `backdrop-filter`;
        // v3.4 added `-webkit-backdrop-filter` alongside. Drop the
        // vendor-prefixed property in v3.3 compat mode so output
        // matches byte-for-byte.
        if v33 && prop.starts_with("-webkit-") {
            continue;
        }
        out.push(((*prop).to_string(), composed.to_string()));
    }
    Some(out)
}

/// Resolve a transform-shorthand utility (`translate-x-4`, `rotate-45`,
/// `scale-95`, `skew-x-12`). Each emitted rule sets the named
/// `--tw-*` vars to the resolved value and re-asserts the full
/// `transform: <CSS_TRANSFORM_VALUE>` so the composition with sibling
/// transform utilities stays consistent. Mirrors
/// `vendor/tailwindcss-v3/src/corePlugins.js` for the `translate`,
/// `rotate`, `skew`, `scale` plugins driven by `cssTransformValue`.
/// `border-spacing-N` / `border-spacing-x-N` / `border-spacing-y-N`.
/// Writes each var in `vars` to the resolved scalar then re-asserts
/// the composed shorthand
/// `border-spacing: var(--tw-border-spacing-x) var(--tw-border-spacing-y)`.
/// Mirrors upstream's `borderSpacing` plugin.
fn resolve_border_spacing(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
    vars: &[&'static str],
) -> Option<Vec<(String, String)>> {
    if parsed.modifier.is_some() || parsed.negative {
        return None;
    }
    let resolved = resolve_scalar(util, value_key, config)?;
    let mut out: Vec<(String, String)> = vars
        .iter()
        .map(|v| ((*v).to_string(), resolved.clone()))
        .collect();
    out.push((
        "border-spacing".to_string(),
        "var(--tw-border-spacing-x) var(--tw-border-spacing-y)".to_string(),
    ));
    Some(out)
}

fn resolve_transform(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
    vars: &[&'static str],
) -> Option<Vec<(String, String)>> {
    if parsed.modifier.is_some() {
        return None;
    }
    let resolved = resolve_scalar(util, value_key, config)?;
    // Negative handling: same gate as `Simple`.
    let value = if parsed.negative {
        if !util.supports_negative || !is_negatable(&resolved) {
            return None;
        }
        negate(&resolved)
    } else {
        resolved
    };
    let mut out: Vec<(String, String)> = vars
        .iter()
        .map(|name| ((*name).to_string(), value.clone()))
        .collect();
    out.push(("transform".to_string(), CSS_TRANSFORM_VALUE.to_string()));
    Some(out)
}

/// Color resolver. Per `vendor/tailwindcss-v3/src/util/withAlphaVariable.js`:
///
///   - keyword color (transparent, currentColor, inherit, etc.) ->
///     emit `<property>: <keyword>`. No opacity wrapping. Modifier is
///     ignored (Tailwind drops the rule rather than warns).
///   - hex color, no modifier -> two decls:
///       `<opacity_var>: 1`
///       `<property>: rgb(R G B / var(<opacity_var>, 1))`
///   - hex color + opacity modifier -> one decl:
///       `<property>: rgb(R G B / <opacity>)`
///     where `<opacity>` resolves through `theme.opacity.<key>` for
///     named modifiers (`/50` -> `0.5`) or the literal value for
///     arbitrary `[.31]`.
///   - rgb(…) / hsl(…) arbitrary value -> use as-is, no parsing.
#[allow(clippy::too_many_arguments)]
fn resolve_color(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
    properties: &[&str],
    opacity_var: &str,
    with_alpha_variable: bool,
) -> Option<Vec<(String, String)>> {
    if parsed.negative {
        return None;
    }
    // Gate the alpha-variable indirection on the related
    // `*-opacity` core plugin being enabled. If the user disabled
    // (or didn't include) `backgroundOpacity`, `bg-red-500`
    // should emit the raw color — `background-color: <value>` —
    // not `background-color: rgb(... / var(--tw-bg-opacity, 1))`.
    // Mirrors upstream's `withAlphaVariable` short-circuit.
    let opacity_plugin = opacity_plugin_for(opacity_var);
    let with_alpha_variable = with_alpha_variable
        && opacity_plugin
            .map(|p| core_plugin_enabled(config, p))
            .unwrap_or(true);

    // Slash-in-key first-match: when the candidate has a modifier
    // (`bg-red-500/50`) and the user defined a color with that
    // exact slashed key (`theme.colors['red-500/50']`), match the
    // full key as the color value and DROP the opacity modifier.
    // Mirrors upstream's `colors with slashes are matched first`
    // behaviour.
    let slashed_match: Option<String> = parsed.modifier.as_ref().and_then(|m| {
        let modifier_str = match m {
            galeforce_parser::Modifier::Named(s) => *s,
            galeforce_parser::Modifier::Arbitrary(s) => *s,
        };
        let full_key = format!("{value_key}/{modifier_str}");
        lookup_theme_value(util, &full_key, config).and_then(|v| value_to_string(&v))
    });
    if let Some(_raw_match) = &slashed_match {
        // Fall through to the rest of the function with a synthetic
        // parsed candidate (modifier cleared) and `raw` set to the
        // matched color. The early-return logic that treats hex /
        // rgb / function colors with the alpha-variable wrap stays
        // identical to the no-modifier path.
    }
    // Build a synthetic parsed-without-modifier for the slashed-
    // match path so the existing modifier handling below treats
    // it as if the user wrote `bg-red-500/50` with no `/50`.
    let parsed_for_resolve;
    let parsed_ref: &ParsedCandidate = if slashed_match.is_some() {
        let mut p = parsed.clone();
        p.modifier = None;
        parsed_for_resolve = p;
        &parsed_for_resolve
    } else {
        parsed
    };
    let parsed = parsed_ref;
    let raw = if let Some(matched_raw) = slashed_match {
        matched_raw
    } else if let Some(arb) = arbitrary_inner(value_key) {
        let probe = normalize_arbitrary_with_config(arb, config);
        // Data-type hint dispatch: `border-[length:var(--v)]` should
        // never resolve as a color. Reject anything not hinted as
        // `color`. Strip the `color:` hint when it's present so the
        // body becomes the raw color value.
        if let Some((hint, body)) = data_type_hint(&probe) {
            if hint != "color" {
                return None;
            }
            body.to_string()
        } else {
            // Non-color shapes — image/url/gradient functions,
            // position-keyword expressions, and length-pair
            // (`200px 100px`) tuples — fall through so the sibling
            // resolver (backgroundImage / backgroundPosition /
            // backgroundSize) can pick them up. Mirrors upstream's
            // `inferDataType`: `bg-[value]` only routes to
            // backgroundColor when the value's shape doesn't match a
            // more specific `bg-` plugin.
            if util.theme_key == "backgroundColor"
                && (looks_like_css_image(&probe)
                    || looks_like_css_position(&probe)
                    || looks_like_length_pair(&probe)
                    || looks_like_bg_size_list(&probe))
            {
                return None;
            }
            probe
        }
    } else {
        let v = lookup_theme_value(util, value_key, config)?;
        value_to_string(&v)?
    };

    // Keywords pass through verbatim.
    if is_color_keyword(&raw) {
        if parsed.modifier.is_some() {
            // Tailwind silently drops `bg-transparent/50` etc. (alpha
            // on a keyword has no defined meaning).
            return None;
        }
        return Some(
            properties
                .iter()
                .map(|p| ((*p).to_string(), raw.clone()))
                .collect(),
        );
    }

    // Plugins that opt out of `withAlphaVariable` (outline-color in
    // v3.4.19) just emit the raw color via `toColorValue`. No
    // `--tw-*-opacity` indirection. Modifier still inlines an alpha
    // when the value is a parsable hex.
    if !with_alpha_variable {
        let modifier_alpha = parsed
            .modifier
            .as_ref()
            .and_then(|m| resolve_opacity_modifier(m, config));
        if let Some((r, g, b)) = parse_hex(&raw) {
            if let Some(alpha_str) = modifier_alpha {
                return Some(
                    properties
                        .iter()
                        .map(|p| ((*p).to_string(), format!("rgb({r} {g} {b} / {alpha_str})")))
                        .collect(),
                );
            }
            if parsed.modifier.is_some() {
                return None;
            }
            return Some(
                properties
                    .iter()
                    .map(|p| ((*p).to_string(), raw.clone()))
                    .collect(),
            );
        }
        if parsed.modifier.is_some() {
            return None;
        }
        return Some(
            properties
                .iter()
                .map(|p| ((*p).to_string(), raw.clone()))
                .collect(),
        );
    }

    // Try to parse as a hex color.
    let alpha = parsed
        .modifier
        .as_ref()
        .and_then(|m| resolve_opacity_modifier(m, config));

    // CSS named colors get the same wrap as hex. `text-[black]` →
    // `--tw-text-opacity: 1; color: rgb(0 0 0 / var(...))`. Mirrors
    // upstream's `parseColor` which converts named colors to RGB
    // before running through `withAlphaVariable.js`.
    if let Some((r, g, b)) = named_color_to_rgb(&raw) {
        if let Some(alpha_str) = alpha {
            return Some(
                properties
                    .iter()
                    .map(|p| ((*p).to_string(), format!("rgb({r} {g} {b} / {alpha_str})")))
                    .collect(),
            );
        }
        if parsed.modifier.is_some() {
            return None;
        }
        let opacity_fn = format_opacity_var(opacity_var, config);
        let mut out: Vec<(String, String)> = vec![(opacity_var.to_string(), "1".to_string())];
        for prop in properties {
            out.push((
                (*prop).to_string(),
                format!("rgb({r} {g} {b} / {opacity_fn})"),
            ));
        }
        return Some(out);
    }

    if let Some((r, g, b)) = parse_hex(&raw) {
        if let Some(alpha_str) = alpha {
            // Inline alpha; no opacity variable.
            return Some(
                properties
                    .iter()
                    .map(|p| ((*p).to_string(), format!("rgb({r} {g} {b} / {alpha_str})")))
                    .collect(),
            );
        }
        if parsed.modifier.is_some() {
            // Modifier present but couldn't resolve to an opacity (e.g.
            // unknown theme key). Tailwind drops the rule.
            return None;
        }
        // 8-char (`#RRGGBBAA`) and 4-char (`#RGBA`) hexes carry their
        // alpha in the literal. The oracle skips the `--tw-*-opacity`
        // wrapping in that case and emits the hex verbatim — see
        // `withAlphaVariable.js` `hasAlpha` short-circuit. Without
        // this, `bg-[#0b14374d]` would emit a wrapped `rgb(11 20 55 /
        // var(--tw-bg-opacity, 1))` that ignores the explicit alpha.
        if hex_has_embedded_alpha(&raw) {
            return Some(
                properties
                    .iter()
                    .map(|p| ((*p).to_string(), raw.clone()))
                    .collect(),
            );
        }
        // No modifier, no embedded alpha: opacity variable shared
        // across properties + alpha-channel rgb on each property.
        let opacity_fn = format_opacity_var(opacity_var, config);
        let mut out: Vec<(String, String)> = vec![(opacity_var.to_string(), "1".to_string())];
        for prop in properties {
            out.push((
                (*prop).to_string(),
                format!("rgb({r} {g} {b} / {opacity_fn})"),
            ));
        }
        return Some(out);
    }

    // Function-call colors (rgb / hsl / etc.) get the same treatment
    // as hex when there's no alpha component. Per Tailwind's
    // `withAlphaVariable.js`, the rule is: if the existing function
    // call already has an alpha (a `/` separator inside the parens),
    // leave it alone; otherwise inject ` / var(--tw-*-opacity, 1)`
    // before the closing paren.
    if let Some((fn_open, fn_close)) = find_color_function(&raw) {
        // If the function call isn't the *whole* value (anything
        // non-whitespace before or after the parens), the user wrote
        // a multi-token expression like `rgb(...) black`. Tailwind's
        // `parseColor` rejects that so it emits verbatim with no
        // alpha wrap. We mirror by falling through to the
        // pass-through tail.
        let prefix_clean = raw[..fn_open]
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == '-' || c == '_');
        let suffix_clean = raw[fn_close + 1..].trim().is_empty();
        let body_inner = &raw[fn_open + 1..fn_close];
        // Mirror upstream's `parseColor`: a function body is a
        // wrappable color iff it has 3 numeric components OR has a
        // 1–2-part shape where the first part is `var(...)` (the
        // `rgba(var(--rgb), 0.5)` indirection). For other bodies
        // — most commonly `hsl(var(--x))` — `withAlphaVariable`
        // returns the value verbatim with no alpha indirection.
        let body_parses = function_body_parses_as_color(body_inner);
        if !suffix_clean || !prefix_clean || !body_parses {
            if parsed.modifier.is_some() {
                return None;
            }
            if has_top_level_colon(&raw) {
                return None;
            }
            return Some(
                properties
                    .iter()
                    .map(|p| ((*p).to_string(), raw.clone()))
                    .collect(),
            );
        }
        let body = body_inner;
        // Two ways the function call already specifies alpha:
        //   1. Modern slash form — `rgb(R G B / A)`. `body` contains `/`.
        //   2. Legacy comma form — `rgba(R, G, B, A)` / `hsla(H, S, L, A)`.
        //      Detect by function name (ends in `a`). The 4-arg comma form
        //      ALWAYS has alpha; the 3-arg form would actually be invalid
        //      so this is safe.
        // Without this, `text-[rgba(255,255,255,0.15)]` was wrapped with
        // `var(--tw-text-opacity, 1)` which mangles the explicit alpha.
        let fn_name = &raw[..fn_open];
        let legacy_alpha = matches!(fn_name, "rgba" | "hsla")
            || fn_name
                .rsplit(|c: char| !c.is_ascii_alphabetic())
                .next()
                .map(|n| matches!(n, "rgba" | "hsla"))
                .unwrap_or(false);
        let already_has_alpha = body.contains('/') || legacy_alpha;
        // Convert legacy comma syntax `rgb(R, G, B)` / `hsl(H, S, L)`
        // to modern space syntax `rgb(R G B)` so the alpha wrap
        // produces clean output. Mirrors `parseColor` + re-emit in
        // upstream's `withAlphaVariable.js`. Only kicks in for the
        // non-alpha variants (rgb/hsl/hwb/lab/lch/oklab/oklch); the
        // -a (rgba/hsla) form keeps its comma form because the
        // alpha already specifies separator semantics.
        let body_normalized: String =
            if !already_has_alpha && fn_name_supports_modern_syntax(fn_name) {
                comma_to_space_top_level(body)
            } else {
                body.to_string()
            };
        if let Some(alpha_str) = alpha {
            // Modifier wins. Replace any existing alpha with the
            // resolved one; if none was present, append.
            let new_body = if already_has_alpha {
                replace_alpha(body, &alpha_str)
            } else {
                format!("{} / {alpha_str}", body_normalized.trim())
            };
            let composed = format!("{}({new_body}{}", &raw[..fn_open], &raw[fn_close..]);
            return Some(
                properties
                    .iter()
                    .map(|p| ((*p).to_string(), composed.clone()))
                    .collect(),
            );
        }
        if parsed.modifier.is_some() {
            return None;
        }
        if already_has_alpha {
            // Custom colors with opacityValue functions that resolve to numeric
            // opacity (e.g., `rgb(10 20 30 / 1)`) should generate opacity variables
            // so they can be modified via opacity modifiers.
            //
            // This applies only to:
            // - Non-arbitrary theme colors (not `bg-[...]`)
            // - When there's no explicit opacity modifier (those already inline alpha)
            // - When the opacity plugin is enabled
            let is_arbitrary = arbitrary_inner(value_key).is_some();
            let has_no_modifier = parsed.modifier.is_none();

            if !is_arbitrary && has_no_modifier && with_alpha_variable {
                if let Some(alpha_value) = body.split('/').last().map(|s| s.trim()) {
                    let is_numeric_or_var =
                        alpha_value.chars().all(|c| c.is_ascii_digit() || c == '.')
                            || (alpha_value.starts_with("var(") && alpha_value.ends_with(")"));

                    if is_numeric_or_var {
                        let color_part = body.split('/').next().unwrap_or(body).trim();
                        let new_value = format!(
                            "{}({} / var({}, 1){}",
                            &raw[..fn_open],
                            color_part,
                            opacity_var,
                            &raw[fn_close..],
                        );
                        let mut out: Vec<(String, String)> =
                            vec![(opacity_var.to_string(), "1".to_string())];
                        for prop in properties {
                            out.push(((*prop).to_string(), new_value.clone()));
                        }
                        return Some(out);
                    }
                }
            }

            return Some(
                properties
                    .iter()
                    .map(|p| ((*p).to_string(), raw.clone()))
                    .collect(),
            );
        }
        let opacity_fn = format_opacity_var(opacity_var, config);
        let new_value = format!(
            "{}({} / {}{}",
            &raw[..fn_open],
            body_normalized.trim(),
            opacity_fn,
            &raw[fn_close..],
        );
        let mut out: Vec<(String, String)> = vec![(opacity_var.to_string(), "1".to_string())];
        for prop in properties {
            out.push(((*prop).to_string(), new_value.clone()));
        }
        return Some(out);
    }

    // Non-hex, non-function value. Pass through verbatim. Modifier
    // can't apply.
    if parsed.modifier.is_some() {
        return None;
    }
    // Reject values with a top-level `:` — these are non-color
    // shapes that snuck through the hint detector
    // (`text-[angle:var(--x)]`). Tailwind drops them rather than
    // emit invalid CSS.
    if has_top_level_colon(&raw) {
        return None;
    }
    Some(
        properties
            .iter()
            .map(|p| ((*p).to_string(), raw.clone()))
            .collect(),
    )
}

/// Locate the first color-function call in `s`, returning the
/// `(open_paren_index, close_paren_index)` for the outermost matching
/// parens. Recognized identifiers: `rgb`, `rgba`, `hsl`, `hsla`,
/// `hwb`, `lab`, `lch`, `oklab`, `oklch`, `color`. Returns `None` if
/// no recognized function is found.
fn find_color_function(s: &str) -> Option<(usize, usize)> {
    const FNS: &[&str] = &[
        "rgb", "rgba", "hsl", "hsla", "hwb", "lab", "lch", "oklab", "oklch", "color",
    ];
    for fn_name in FNS {
        let needle = format!("{fn_name}(");
        if let Some(pos) = s.find(&needle) {
            // Make sure this is actually a token boundary (not part of
            // a longer identifier).
            let before_ok = pos == 0 || {
                let prev = s.as_bytes()[pos - 1];
                !prev.is_ascii_alphanumeric() && prev != b'-' && prev != b'_'
            };
            if !before_ok {
                continue;
            }
            let open = pos + fn_name.len();
            let close = find_matching_close_paren_at(s, open)?;
            return Some((open, close));
        }
    }
    None
}

/// Same as `looks_like_css_image` but accepts comma-separated lists:
/// `image(),var(--x)`, `linear-gradient(...),conic-gradient(...)`.
/// Each top-level comma-separated part must be either a CSS image
/// shape OR `var(...)` (which Tailwind treats as image-typed in
/// arbitrary value contexts when sat in a list with another image).
fn is_arbitrary_image_shape(value: &str) -> bool {
    let parts: Vec<&str> = split_top_level_comma(value).collect();
    if parts.is_empty() {
        return false;
    }
    let mut saw_image = false;
    for part in parts {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            return false;
        }
        if looks_like_css_image(trimmed) {
            saw_image = true;
        } else if trimmed.starts_with("var(") || trimmed.starts_with("--") {
            // `var(...)` parts are accepted in image lists alongside
            // a recognised image; alone they're insufficient.
            continue;
        } else {
            return false;
        }
    }
    saw_image
}

/// Split `value` on top-level commas (skipping nested parens etc.).
/// Returns owned slice references walking each part.
fn split_top_level_comma(value: &str) -> impl Iterator<Item = &str> {
    let mut starts: Vec<usize> = vec![0];
    let mut depth = 0i32;
    let bytes = value.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                starts.push(i + 1);
            }
            _ => {}
        }
    }
    let len = value.len();
    let count = starts.len();
    (0..count).map(move |i| {
        let start = starts[i];
        let end = if i + 1 < count {
            starts[i + 1] - 1
        } else {
            len
        };
        &value[start..end]
    })
}

/// True when `value` looks like a CSS image expression — any
/// `*-gradient(...)`, `image(...)`, `image-set(...)`, `url(...)`,
/// `cross-fade(...)`, `element(...)`, or `paint(...)` call. Used by
/// the bg-* dispatch so `bg-[linear-gradient(...)]` resolves through
/// `background-image` rather than `background-color`. Mirrors
/// upstream's `dataTypes.image()` which accepts these shapes.
fn looks_like_css_image(value: &str) -> bool {
    let trimmed = value.trim_start();
    let ident: String = trimmed
        .chars()
        .take_while(|c| c.is_ascii_alphabetic() || *c == '-')
        .flat_map(char::to_lowercase)
        .collect();
    let after = trimmed[ident.len()..].trim_start();
    if !after.starts_with('(') {
        return false;
    }
    if matches!(
        ident.as_str(),
        "image" | "image-set" | "url" | "cross-fade" | "element" | "paint"
    ) {
        return true;
    }
    ident.ends_with("-gradient")
}

/// True when `value` is two whitespace-separated CSS length /
/// percentage tokens — `200px 100px`, `50% 4rem`. Used as a tiebreak
/// in the bg-* dispatch so length pairs route to background-position
/// rather than the color fallback. Mirrors upstream's bgSize/bgPosition
/// `length`/`percentage` typed acceptance.
fn looks_like_length_pair(value: &str) -> bool {
    let tokens: Vec<&str> = value.split_whitespace().collect();
    tokens.len() == 2 && tokens.iter().all(|t| is_length_or_percent(t))
}

/// True for a comma-separated <bg-size>+ list: `auto auto,cover,
/// contain,10px,10px 10%`. Each entry is a keyword
/// (`auto`/`cover`/`contain`) or a length / percent (single or
/// paired). Mirrors CSS's <bg-size>+ grammar for the multi-bg
/// form. Used so `bg-[…]` with this shape routes to
/// background-size rather than falling through to background-color.
fn looks_like_bg_size_list(value: &str) -> bool {
    if !value.contains(',') {
        return false;
    }
    value.split(',').all(|part| {
        let p = part.trim();
        if p.is_empty() {
            return false;
        }
        let toks: Vec<&str> = p.split_whitespace().collect();
        if toks.is_empty() || toks.len() > 2 {
            return false;
        }
        toks.iter().all(|t| {
            let lc = t.to_ascii_lowercase();
            matches!(lc.as_str(), "auto" | "cover" | "contain") || is_length_or_percent(t)
        })
    })
}

/// True when `value` looks like a `<position>` shape per CSS
/// background-position grammar. Recognises the four keyword positions
/// and short keyword-plus-length combinations like `center top 1rem`.
/// Used so `bg-[center_top_1rem]` routes to background-position
/// rather than background-color.
fn looks_like_css_position(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if tokens.is_empty() || tokens.len() > 4 {
        return false;
    }
    let mut saw_keyword = false;
    for token in &tokens {
        let lc: String = token.chars().flat_map(char::to_lowercase).collect();
        if matches!(lc.as_str(), "center" | "top" | "right" | "bottom" | "left") {
            saw_keyword = true;
        } else if !is_length_or_percent(token) {
            return false;
        }
    }
    saw_keyword
}

fn is_length_or_percent(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return false;
    }
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    if matches!(bytes[i], b'+' | b'-') {
        i += 1;
    }
    let mut saw_digit = false;
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
        if bytes[i].is_ascii_digit() {
            saw_digit = true;
        }
        i += 1;
    }
    if !saw_digit {
        return false;
    }
    let suffix = &trimmed[i..];
    suffix.is_empty()
        || suffix == "%"
        || matches!(
            suffix,
            "px" | "em"
                | "rem"
                | "ex"
                | "ch"
                | "vw"
                | "vh"
                | "vmin"
                | "vmax"
                | "cm"
                | "mm"
                | "in"
                | "pt"
                | "pc"
                | "fr"
                | "deg"
                | "rad"
                | "grad"
                | "turn"
                | "s"
                | "ms"
                | "Q"
                | "lh"
                | "rlh"
                | "vi"
                | "vb"
                | "svh"
                | "svw"
                | "lvh"
                | "lvw"
                | "dvh"
                | "dvw"
                | "cqw"
                | "cqh"
                | "cqi"
                | "cqb"
                | "cqmin"
                | "cqmax"
        )
}

/// Mirrors `parseColor()` from `vendor/tailwindcss-v3/src/util/color.js`
/// for the function-body case: returns `true` iff the body would
/// resolve to a parseable color triplet, in which case
/// `withAlphaVariable` injects the alpha indirection. Returns
/// `false` for shapes Tailwind drops as unparseable —
/// `hsl(var(--x))`, `rgb(0)`, etc. — which then emit verbatim.
fn function_body_parses_as_color(body: &str) -> bool {
    let parts: Vec<String> = split_color_parts(body)
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect();
    // Strip an explicit alpha component. `parseColor` separates the
    // alpha via `,` or `/` after the colour parts; for our wrap-or-
    // not decision the alpha doesn't matter.
    let color_parts: Vec<&String> = parts.iter().take(3).collect();
    let n = color_parts.len();
    if n == 0 {
        return false;
    }
    // 3+ parts → parseable.
    if n >= 3 {
        return true;
    }
    // The non-loose `withAlphaVariable` path (which feeds the
    // `--tw-*-opacity` indirection) only accepts the
    // `rgba(var(--rgb), 0.5)` shorthand: exactly 2 components, the
    // first being `var(...)`. Single-var bodies like
    // `rgb(var(--bg-color))` parse as length=1 → null upstream and
    // get emitted verbatim with NO alpha wrap. Mirror that here.
    n == 2 && color_parts[0].starts_with("var(")
}

/// Split a color-function body into the channel parts. Tailwind's
/// `parseColor` accepts both whitespace and comma separators,
/// plus the `/` alpha separator. We split on top-level whitespace
/// and commas, treating `/` as an alpha boundary.
fn split_color_parts(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let mut parts: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut depth = 0i32;
    for &b in bytes {
        match b {
            b'(' | b'[' | b'{' => {
                depth += 1;
                buf.push(b as char);
            }
            b')' | b']' | b'}' => {
                depth -= 1;
                buf.push(b as char);
            }
            b',' | b' ' | b'\t' | b'\n' if depth == 0 => {
                if !buf.is_empty() {
                    parts.push(std::mem::take(&mut buf));
                }
            }
            b'/' if depth == 0 => {
                // Alpha boundary — done with color components.
                if !buf.is_empty() {
                    parts.push(std::mem::take(&mut buf));
                }
                break;
            }
            _ => buf.push(b as char),
        }
    }
    if !buf.is_empty() {
        parts.push(buf);
    }
    parts
}

/// `rgb`, `hsl`, `hwb`, `lab`, `lch`, `oklab`, `oklch`, `color` accept
/// modern space-separated syntax (`rgb(R G B / A)`); `rgba`/`hsla`
/// only accept comma-separated. Used to decide whether to normalise
/// a parsed body's commas to spaces during alpha-variable wrapping.
fn fn_name_supports_modern_syntax(name: &str) -> bool {
    let last_ident = name
        .rsplit(|c: char| !c.is_ascii_alphabetic())
        .next()
        .unwrap_or("");
    matches!(
        last_ident,
        "rgb" | "hsl" | "hwb" | "lab" | "lch" | "oklab" | "oklch" | "color"
    )
}

/// Find the matching `)` for the `(` at `open` (depth-aware). Mirrors
/// the helper in `directives.rs` but operating on `&str` and returning
/// the close-paren index.
fn find_matching_close_paren_at(s: &str, open: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.get(open) != Some(&b'(') {
        return None;
    }
    let mut depth = 1i32;
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
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

/// Replace the substring after a `/` (alpha component) inside a color
/// function body with `new_alpha`. The body is the content between the
/// `(` and `)` (exclusive). Whitespace around the `/` is preserved.
fn replace_alpha(body: &str, new_alpha: &str) -> String {
    if let Some(slash_idx) = body.find('/') {
        // Keep everything up through the `/` (inclusive of trailing
        // whitespace before the alpha). Replace the rest with the new
        // alpha.
        let prefix = &body[..slash_idx + 1];
        format!("{prefix} {new_alpha}")
    } else {
        // Caller should not reach this path; defensive.
        format!("{body} / {new_alpha}")
    }
}

fn is_color_keyword(s: &str) -> bool {
    matches!(
        s,
        "transparent"
            | "currentColor"
            | "inherit"
            | "initial"
            | "revert"
            | "revert-layer"
            | "unset"
    )
}

/// Resolve the opacity component of a color modifier
/// (`bg-red-500/50`, `text-blue-700/[.31]`). Named modifiers consult
/// `theme.opacity.<key>`; arbitrary modifiers use the inner value
/// verbatim (with the `_`-to-space normalize so `/[0.5_]` works).
fn resolve_opacity_modifier(
    modifier: &galeforce_parser::Modifier,
    config: Option<&Value>,
) -> Option<String> {
    match modifier {
        galeforce_parser::Modifier::Arbitrary(s) => Some(normalize_arbitrary(s)),
        galeforce_parser::Modifier::Named(name) => config
            .and_then(|c| c.get("theme"))
            .and_then(|t| lookup_theme(t, &format!("opacity.{name}")))
            .and_then(value_to_string)
            .or_else(|| {
                let dt = default_theme_ref();
                lookup_theme(dt, &format!("opacity.{name}")).and_then(value_to_string)
            }),
    }
}

fn resolve_simple(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
) -> Option<Vec<(String, String)>> {
    // Type discrimination for arbitrary values: a hex literal or
    // a color function call should never resolve through a Simple
    // plugin (which is for length / size / number values). Mirrors
    // what Tailwind's `dataTypes` type list rejects on plugins like
    // `borderWidth: { type: ['line-width', 'length'] }` — passing a
    // color falls through so the multi-match lookup can try a
    // sibling color plugin next.
    if let Some(arb) = arbitrary_inner(value_key) {
        let probe = normalize_arbitrary_with_config(arb, config);
        if probe.starts_with('#') || starts_with_color_function(&probe) {
            // Color-shaped arbitrary values belong to a Color resolver,
            // not Simple — except for backgroundImage which doesn't
            // exist as a shape.
            if util.theme_key != "backgroundImage" {
                return None;
            }
            return None;
        }
        // Length-y plugins (border-width, outline-width, decoration-
        // thickness, divide-width, stroke-width, fontSize) refuse
        // color-shaped arbitrary values so the matching color
        // resolver can pick the candidate up. `border-[red_black]`
        // → border-color (multi-color), not border-width. Per
        // `corePlugins.js` these plugins also lack the `'any'` type
        // — unhinted `var(...)` values fall through to the sibling
        // color resolver which has `['color', 'any']`.
        if is_length_y_theme_key(util.theme_key) {
            if is_color_value(&probe) {
                return None;
            }
            if probe.trim_start().starts_with("var(") {
                return None;
            }
        }
        // strokeWidth additionally refuses `url(...)` so `stroke-
        // [url(#g)]` falls through to the SVG `stroke` color/image
        // resolver.
        if util.theme_key == "strokeWidth" && probe.trim_start().starts_with("url(") {
            return None;
        }
        // Data-type hint dispatch (`border-[length:var(--v)]`,
        // `font-[number:lighter]`, etc.). Mirrors Tailwind 3's
        // `inferDataType` step: when the user wrote an explicit
        // type prefix, it's binding — wrong-type plugins drop the
        // candidate, right-type plugins consume the body verbatim.
        if let Some((hint, body)) = data_type_hint(&probe) {
            // Per-theme-key hint affinity. The bg-* family splits
            // arbitrary candidates across image/position/size based
            // on the hint, so each entry only accepts its own.
            let accept = match (util.theme_key, hint) {
                ("backgroundImage", "image" | "url") => true,
                ("backgroundImage", _) => false,
                ("backgroundPosition", "position") => true,
                ("backgroundPosition", _) => false,
                ("backgroundSize", "length" | "percentage" | "size") => true,
                ("backgroundSize", _) => false,
                ("fontWeight", "number") => true,
                ("fontWeight", _) => false,
                (_, "color" | "image" | "url" | "family-name") => false,
                _ => true,
            };
            if !accept {
                return None;
            }
            return Some(
                util.properties
                    .iter()
                    .map(|p| ((*p).to_string(), body.to_string()))
                    .collect(),
            );
        }
        // No recognized hint, but a top-level `:` means the user
        // wrote a *bogus* hint (`text-[angle:var(--x)]`) — Tailwind
        // refuses these. Reject so we don't produce invalid CSS.
        if has_top_level_colon(&probe) {
            return None;
        }
        // No hint: Tailwind's plugin-specific dataType inference. We
        // only handle the cases the conformance corpus exercises.
        if util.theme_key == "fontWeight" {
            let first = probe.chars().next();
            let is_number = first
                .map(|c| c.is_ascii_digit() || c == '+' || c == '-' || c == '.')
                .unwrap_or(false);
            // Per upstream `corePlugins.js`, fontWeight has type
            // `['lookup', 'number', 'any']`. The `'any'` fallback
            // means `font-[var(--x)]` resolves through fontWeight
            // (registered before fontFamily). Accept `var(...)` and
            // bare CSS-identifier shapes via that fallback; reject
            // everything else so fontFamily can claim e.g. `font-
            // [Helvetica]`-style values.
            let is_var = probe.trim_start().starts_with("var(");
            if !is_number && !is_var {
                return None;
            }
        }
        // bg-* shape inference: the three resolvers share the `bg-`
        // prefix and only one should accept any given candidate.
        // Match upstream's `inferDataType` for these specific shapes.
        match util.theme_key {
            "backgroundImage" => {
                if !is_arbitrary_image_shape(&probe) {
                    return None;
                }
            }
            "backgroundPosition" => {
                if !looks_like_css_position(&probe) && !looks_like_length_pair(&probe) {
                    return None;
                }
            }
            "backgroundSize" => {
                // Hint-driven by default — a bare `bg-[200px]` is
                // ambiguous (length-pair routes to position) so
                // upstream refuses without the `length:` hint. The
                // exception: a multi-bg-size comma-list (`bg-
                // [auto_auto,cover,10px_10%]`) is unambiguously a
                // size value and routes here.
                if !looks_like_bg_size_list(&probe) {
                    return None;
                }
            }
            _ => {}
        }
    }
    let resolved = resolve_scalar(util, value_key, config)?;

    // Negative handling: only applies if the family allows it AND the
    // resolved value is "numeric-looking" (Tailwind doesn't negate
    // `auto`, for instance — it just drops the candidate). The simplest
    // and exact rule mirrors corePlugins: `supportsNegativeValues` is a
    // lookup hint, and Tailwind's `formatNegativeValue` checks
    // `value.startsWith('-')` / numeric prefix.
    let resolved = if parsed.negative {
        if !util.supports_negative || !is_negatable(&resolved) {
            return None;
        }
        negate(&resolved)
    } else {
        resolved
    };

    // Modifier (`/<value>`) on simple value-bearing utilities is
    // reserved for opacity shorthands and other niche features we
    // haven't shipped. Fail the resolve if a modifier was attached.
    if parsed.modifier.is_some() {
        return None;
    }

    // `gridTemplateColumns` / `gridTemplateRows` / `objectPosition`
    // get a backwards-compat transform that converts top-level
    // commas to spaces (BEFORE the `_` → space step, users wrote
    // `grid-cols-[200px,300px]` with commas). Mirrors upstream's
    // `transformThemeValue.js`.
    let resolved = if matches!(
        util.theme_key,
        "gridTemplateColumns" | "gridTemplateRows" | "objectPosition"
    ) {
        comma_to_space_top_level(&resolved)
    } else {
        resolved
    };

    Some(
        util.properties
            .iter()
            .map(|p| ((*p).to_string(), resolved.clone()))
            .collect(),
    )
}

/// Replace top-level commas (those NOT inside parens/brackets/braces
/// or string literals) with single spaces, then collapse runs of
/// whitespace to one space and trim. Mirrors PostCSS's
/// `list.comma(value).join(' ')` used by upstream's
/// `transformThemeValue` for grid-template-columns/rows and
/// object-position.
fn comma_to_space_top_level(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut buf = String::with_capacity(s.len());
    let mut depth = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' || b == b'\'' {
            let quote = b;
            buf.push(b as char);
            i += 1;
            while i < bytes.len() && bytes[i] != quote {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    buf.push(bytes[i] as char);
                    buf.push(bytes[i + 1] as char);
                    i += 2;
                } else {
                    buf.push(bytes[i] as char);
                    i += 1;
                }
            }
            if i < bytes.len() {
                buf.push(bytes[i] as char);
                i += 1;
            }
            continue;
        }
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            _ => {}
        }
        if depth == 0 && b == b',' {
            buf.push(' ');
        } else {
            buf.push(b as char);
        }
        i += 1;
    }
    let mut out = String::with_capacity(buf.len());
    let mut prev_space = false;
    for c in buf.chars() {
        if c == ' ' || c == '\t' {
            if prev_space {
                continue;
            }
            prev_space = true;
            out.push(' ');
        } else {
            prev_space = false;
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// Resolve `text-<size>[/<line-height>]` per `corePlugins.js`'s
/// `fontSize` plugin. Theme value can be:
///
///   - `string`            -> just `font-size`.
///   - `[size, lineHeight]` -> `font-size` + `line-height`.
///   - `[size, { lineHeight, letterSpacing, fontWeight }]`
///     -> `font-size` + each option.
///
/// Slash modifier overrides `line-height` regardless of theme shape.
/// Validate the Named portion of a `text-xs/<modifier>` form so the
/// `line-height` decl we emit is well-formed. Accepts the chars
/// upstream's `lineHeight` theme keys and `coerceValue`'s `length`/
/// `number`/`percentage` types use: alphanumeric, `-`, `_`, `.`, `%`.
/// Anything else (parens, semis, slashes, quotes…) is junk extracted
/// from comments/regex-literals and should cause the candidate to be
/// rejected outright.
fn is_valid_line_height_modifier(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '%'))
}

fn resolve_font_size(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
) -> Option<Vec<(String, String)>> {
    if parsed.negative {
        return None;
    }

    // Arbitrary `text-[14px]` -> `font-size: 14px` (no extras). The
    // upstream `fontSize` plugin advertises
    // `type: ['absolute-size', 'relative-size', 'length', 'percentage']`,
    // so values that obviously aren't a font-size shape (hex colors,
    // rgb()/hsl() functions) skip this plugin so the matcher falls
    // through to `textColor` for `text-[#ef4444]`-style candidates.
    if let Some(arb) = arbitrary_inner(value_key) {
        let normalized = normalize_arbitrary_with_config(arb, config);
        // Hint dispatch — `text-[length:var(--v)]` strips the hint and
        // emits `font-size: var(--v)`, `text-[color:var(--c)]` falls
        // through to textColor, and unknown hints (`angle:`, etc.)
        // drop the candidate. Mirrors upstream's fontSize type list:
        // `['absolute-size', 'relative-size', 'length', 'percentage']`.
        let body = if let Some((hint, hint_body)) = data_type_hint(&normalized) {
            if !matches!(
                hint,
                "length" | "percentage" | "absolute-size" | "relative-size"
            ) {
                return None;
            }
            hint_body.to_string()
        } else {
            if !looks_like_font_size_value(&normalized) {
                return None;
            }
            // Color-shaped values fall through to textColor.
            if is_color_value(&normalized) {
                return None;
            }
            // fontSize has type `['absolute-size', 'relative-size',
            // 'length', 'percentage']` — no `'any'` fallback. So
            // `var(...)`-only values fall through to textColor
            // (which has the `'any'` type) per upstream's plugin
            // ordering.
            if normalized.trim_start().starts_with("var(") {
                return None;
            }
            normalized
        };
        let mut out = vec![("font-size".into(), body)];
        if let Some(modifier) = &parsed.modifier {
            out.push(("line-height".into(), modifier_value(modifier)));
        }
        return Some(out);
    }

    let theme_value = lookup_theme_value(util, value_key, config)?;
    let mut out: Vec<(String, String)> = Vec::new();
    match theme_value {
        Value::String(s) => out.push(("font-size".into(), s.clone())),
        Value::Array(arr) => {
            // First element is the font-size string.
            let size = arr.first().and_then(Value::as_str)?;
            out.push(("font-size".into(), size.to_string()));
            // Second element: either a string (legacy line-height) or
            // an object of options.
            match arr.get(1) {
                Some(Value::String(lh)) => {
                    out.push(("line-height".into(), lh.clone()));
                }
                Some(Value::Object(opts)) => {
                    if let Some(lh) = opts.get("lineHeight").and_then(Value::as_str) {
                        out.push(("line-height".into(), lh.to_string()));
                    }
                    if let Some(ls) = opts.get("letterSpacing").and_then(Value::as_str) {
                        out.push(("letter-spacing".into(), ls.to_string()));
                    }
                    if let Some(fw) = opts.get("fontWeight").and_then(Value::as_str) {
                        out.push(("font-weight".into(), fw.to_string()));
                    }
                }
                _ => {}
            }
        }
        _ => return None,
    }

    // Slash modifier `text-sm/6` overrides line-height. The modifier
    // value first checks `theme.lineHeight.<modifier>`; falling back
    // to the literal value (so `text-sm/[1.4]` and `text-sm/4` both
    // work). We don't have access to the theme.lineHeight ourselves
    // here without a full lookup — but `theme.lineHeight.6 = '1.5rem'`
    // for the default theme; for arbitrary `[…]` modifiers we use the
    // literal.
    if let Some(modifier) = &parsed.modifier {
        let lh = match modifier {
            galeforce_parser::Modifier::Arbitrary(s) => normalize_arbitrary(s),
            galeforce_parser::Modifier::Named(name) => {
                // Reject modifiers that contain characters CSS values
                // would never accept (parens, semis, slashes, etc.).
                // Mirrors upstream's behaviour where junk like
                // `text-xs/);` (regex-literal noise from test files)
                // fails value validation and the whole candidate is
                // discarded — without this we'd emit
                // `line-height: );` which the browser ignores anyway
                // but pollutes the bundle.
                if !is_valid_line_height_modifier(name) {
                    return None;
                }
                // theme.lineHeight.<name> -> use that; otherwise the literal.
                let from_theme = config
                    .and_then(|c| c.get("theme"))
                    .and_then(|t| lookup_theme(t, &format!("lineHeight.{name}")))
                    .and_then(value_to_string)
                    .or_else(|| {
                        let dt = default_theme_ref();
                        lookup_theme(dt, &format!("lineHeight.{name}")).and_then(value_to_string)
                    });
                from_theme.unwrap_or_else(|| (*name).to_string())
            }
        };
        // Replace any existing line-height entry; otherwise append.
        if let Some(existing) = out.iter_mut().find(|(p, _)| p == "line-height") {
            existing.1 = lh;
        } else {
            out.push(("line-height".into(), lh));
        }
    }

    Some(out)
}

fn resolve_font_family(
    util: &ValueUtility,
    value_key: &str,
    parsed: &ParsedCandidate,
    config: Option<&Value>,
) -> Option<Vec<(String, String)>> {
    if parsed.negative || parsed.modifier.is_some() {
        return None;
    }
    if let Some(arb) = arbitrary_inner(value_key) {
        let probe = normalize_arbitrary_with_config(arb, config);
        // Hint-aware dispatch. `family-name:` and the unhinted shape
        // belong to fontFamily; `number:` (and numeric shapes) belong
        // to fontWeight — drop those so the Simple resolver picks the
        // candidate up.
        if let Some((hint, body)) = data_type_hint(&probe) {
            if hint != "family-name" && hint != "generic-name" {
                return None;
            }
            return Some(vec![("font-family".into(), body.to_string())]);
        }
        let starts_numeric = probe
            .chars()
            .next()
            .map(|c| c.is_ascii_digit() || c == '+' || c == '-' || c == '.')
            .unwrap_or(false);
        if starts_numeric {
            return None;
        }
        return Some(vec![("font-family".into(), probe)]);
    }
    let value = lookup_theme_value(util, value_key, config)?;
    let mut out: Vec<(String, String)> = Vec::new();
    match value {
        Value::String(s) => out.push(("font-family".into(), s.clone())),
        Value::Array(arr) => {
            // `[families, options?]` — Tailwind unpacks when arr[1] is a
            // plain object. Otherwise the entire array is the family list.
            let (families_val, options) =
                if arr.len() == 2 && matches!(arr.get(1), Some(Value::Object(_))) {
                    (
                        arr.first().cloned().unwrap_or(Value::Null),
                        arr.get(1).and_then(|v| v.as_object()),
                    )
                } else {
                    (Value::Array(arr.clone()), None)
                };
            let family_string = match families_val {
                Value::String(s) => s,
                Value::Array(items) => items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
                    .join(", "),
                _ => return None,
            };
            out.push(("font-family".into(), family_string));
            if let Some(opts) = options {
                if let Some(ffs) = opts.get("fontFeatureSettings").and_then(Value::as_str) {
                    out.push(("font-feature-settings".into(), ffs.to_string()));
                }
                if let Some(fvs) = opts.get("fontVariationSettings").and_then(Value::as_str) {
                    out.push(("font-variation-settings".into(), fvs.to_string()));
                }
            }
        }
        _ => return None,
    }
    Some(out)
}

fn lookup_theme_value(
    util: &ValueUtility,
    value_key: &str,
    config: Option<&Value>,
) -> Option<Value> {
    // Hot path: direct flat lookup against the user's theme.
    if let Some(v) = config
        .and_then(|c| c.get("theme"))
        .and_then(|t| t.get(util.theme_key))
        .and_then(|n| n.get(value_key))
    {
        return Some(v.clone());
    }
    // Slow path: a user-supplied nested theme (`theme.colors = {
    // green: { light: 'green' }, … }`) needs a hyphen-split walk
    // because `bg-green-light` looks up a flat `green-light` key.
    // Mirrors Tailwind's `flattenColorPalette` resolution applied at
    // theme-read time. We try the value-key as a `-`-separated path
    // against the user's theme section, then against the canonical
    // color table fallbacks (`backgroundColor` → `colors`,
    // `textColor` → `colors`, etc.).
    if let Some(v) = nested_lookup(config, util.theme_key, value_key) {
        return Some(v);
    }
    if let Some(fallback_key) = color_inherits_from(util.theme_key) {
        if let Some(v) = nested_lookup(config, fallback_key, value_key) {
            return Some(v);
        }
    }
    // Only fall back to the default theme if the user hasn't
    // explicitly overridden this theme key. When the user supplies
    // `theme.minHeight: { primary, secondary }` (or removes the
    // default config via `presets: []`), we must NOT serve the
    // defaults — otherwise candidates like `min-h-0` keep working
    // when upstream Tailwind would reject them. Mirrors upstream's
    // `resolveConfig` behaviour: explicit theme overrides replace,
    // not extend.
    let user_section = config
        .and_then(|c| c.get("theme"))
        .and_then(|t| t.get(util.theme_key));
    if let Some(s) = user_section {
        // Section explicitly present — only look up within it; no
        // default fallback. (We already tried the flat + nested
        // forms above.)
        let _ = s;
        return None;
    }
    let dt = default_theme_ref();
    dt.get(util.theme_key)
        .and_then(|n| n.get(value_key))
        .cloned()
}

/// Tailwind's color-family theme keys default-inherit from
/// `theme.colors` per `stubs/config.full.js`. When a flat lookup
/// against the user's `<themeKey>` fails we fall back to
/// `theme.colors`. Returns the fallback theme path or `None` when
/// the key isn't a color family.
fn color_inherits_from(theme_key: &str) -> Option<&'static str> {
    match theme_key {
        "backgroundColor"
        | "textColor"
        | "borderColor"
        | "divideColor"
        | "outlineColor"
        | "ringColor"
        | "ringOffsetColor"
        | "placeholderColor"
        | "caretColor"
        | "accentColor"
        | "fill"
        | "stroke"
        | "textDecorationColor"
        | "boxShadowColor"
        | "gradientColorStops" => Some("colors"),
        _ => None,
    }
}

/// Walk `config.theme.<theme_key>` as a `-`-separated path. So
/// `value_key = "green-light"` against `theme.colors = { green: {
/// light: '...' } }` returns `'...'`. Mirrors how
/// `flattenColorPalette` resolves nested color objects, applied
/// lazily at lookup time.
fn nested_lookup(config: Option<&Value>, theme_key: &str, value_key: &str) -> Option<Value> {
    let section = config
        .and_then(|c| c.get("theme"))
        .and_then(|t| t.get(theme_key))?;
    walk_nested(section, value_key)
}

/// Recursively walk a nested theme object using `value_key` as a
/// dash-joined path. Mirrors Tailwind's `flattenColorPalette` which
/// flattens arbitrary-depth color objects into `red-500` /
/// `severity-log-critical` keys. The walk tries every dash split
/// position from longest-suffix to shortest, descending into the
/// matching branch on each match and recursing with the remainder.
///
/// `colors.severity.log.critical` matches `severity-log-critical`:
///   - try head=`severity-log-critical` → no branch
///   - try head=`severity-log`,    tail=`critical`     → no branch
///   - try head=`severity`,        tail=`log-critical` → found `severity`,
///         recurse into the `log` branch with `critical` as key
fn walk_nested(branch: &Value, key: &str) -> Option<Value> {
    // Direct exact-match shortcut. Tailwind's flattened table has
    // `red-500` as a literal key; if the user wrote `colors.red.500`
    // we'd hit this on the recursion via `red` -> `500`.
    if let Some(v) = branch.get(key) {
        return Some(v.clone());
    }
    let bytes = key.as_bytes();
    let mut split_at: Vec<usize> = Vec::new();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'-' {
            split_at.push(i);
        }
    }
    // Try every split position. Walk longest-head-first since the
    // user's intent is usually the deepest match (Tailwind's
    // `flattenColorPalette` is greedy in that direction too).
    for idx in split_at.iter().rev() {
        let head = &key[..*idx];
        let tail = &key[idx + 1..];
        if let Some(child) = branch.get(head) {
            if let Some(v) = walk_nested(child, tail) {
                return Some(v);
            }
        }
    }
    None
}

fn resolve_scalar(util: &ValueUtility, value_key: &str, config: Option<&Value>) -> Option<String> {
    // Try the theme map FIRST so a user can register a literal
    // `[10px]` key (or any other bracket-shaped string) as a
    // theme value. Only fall through to the arbitrary-value
    // path when the lookup misses. Mirrors upstream's order in
    // `setupContextUtils.js#getMatchingTypes` where theme keys
    // win against the bare-arbitrary parser.
    if let Some(v) = lookup_theme_value(util, value_key, config) {
        if let Some(s) = value_to_string(&v) {
            return Some(s);
        }
    }
    if let Some(arb) = arbitrary_inner(value_key) {
        return Some(normalize_arbitrary_with_config(arb, config));
    }
    None
}

/// Quick discriminator: does this normalized arbitrary value plausibly
/// belong to the fontSize plugin? Mirrors Tailwind's `dataTypes`
/// type-list approach without porting the full type system: hex colors,
/// rgb/hsl/color/oklch/oklab/lab/lch function calls, and the explicit
/// `color(...)` form are clear non-fits and pass through to the next
/// plugin (typically textColor).
fn looks_like_font_size_value(value: &str) -> bool {
    let trimmed = value.trim_start();
    if trimmed.starts_with('#') || starts_with_color_function(trimmed) {
        return false;
    }
    // A bare `<ident>:<rest>` shape that wasn't recognized as a known
    // data-type hint isn't a valid font-size value either — emitting
    // `font-size: angle:var(--x)` produces unparseable CSS. `:` inside
    // a function call (e.g. `min(...)`) is fine; only top-level colons
    // disqualify.
    !has_top_level_colon(trimmed)
}

fn has_top_level_colon(value: &str) -> bool {
    let mut depth = 0i32;
    for b in value.bytes() {
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b':' if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

/// Whether `value` begins with a known color function name followed
/// by `(` — `rgb(...)`, `rgba(...)`, `hsl(...)`, `hsla(...)`, `hwb(...)`,
/// `lab(...)`, `lch(...)`, `oklab(...)`, `oklch(...)`, `color(...)`.
/// Used as a "this isn't a length/size" check by the Simple and
/// FontSize resolvers so they fall through to the matching color
/// plugin.
fn starts_with_color_function(value: &str) -> bool {
    let trimmed = value.trim_start();
    let ident: String = trimmed
        .chars()
        .take_while(|c| c.is_ascii_alphabetic() || *c == '-')
        .flat_map(char::to_lowercase)
        .collect();
    let after_ident = trimmed[ident.len()..].trim_start();
    if !after_ident.starts_with('(') {
        return false;
    }
    matches!(
        ident.as_str(),
        "rgb" | "rgba" | "hsl" | "hsla" | "hwb" | "lab" | "lch" | "oklab" | "oklch" | "color"
    )
}

fn modifier_value(modifier: &galeforce_parser::Modifier) -> String {
    match modifier {
        galeforce_parser::Modifier::Named(s) => (*s).to_string(),
        galeforce_parser::Modifier::Arbitrary(s) => normalize_arbitrary(s),
    }
}

/// `[<inner>]` → `Some(<inner>)`; otherwise `None`.
/// CSS Color Module Level 3+4 named colors. Used to detect when an
/// arbitrary value like `[black]` is a color and length-y resolvers
/// (`borderWidth`, `outlineWidth`, `textDecorationThickness`, …)
/// should fall through to the matching color resolver rather than
/// emitting `border-width: black`. The list is alphabetised but
/// stored in a sorted slice — `binary_search` keeps the lookup O(log n).
static CSS_NAMED_COLORS: &[&str] = &[
    "aliceblue",
    "antiquewhite",
    "aqua",
    "aquamarine",
    "azure",
    "beige",
    "bisque",
    "black",
    "blanchedalmond",
    "blue",
    "blueviolet",
    "brown",
    "burlywood",
    "cadetblue",
    "chartreuse",
    "chocolate",
    "coral",
    "cornflowerblue",
    "cornsilk",
    "crimson",
    "cyan",
    "darkblue",
    "darkcyan",
    "darkgoldenrod",
    "darkgray",
    "darkgreen",
    "darkgrey",
    "darkkhaki",
    "darkmagenta",
    "darkolivegreen",
    "darkorange",
    "darkorchid",
    "darkred",
    "darksalmon",
    "darkseagreen",
    "darkslateblue",
    "darkslategray",
    "darkslategrey",
    "darkturquoise",
    "darkviolet",
    "deeppink",
    "deepskyblue",
    "dimgray",
    "dimgrey",
    "dodgerblue",
    "firebrick",
    "floralwhite",
    "forestgreen",
    "fuchsia",
    "gainsboro",
    "ghostwhite",
    "gold",
    "goldenrod",
    "gray",
    "green",
    "greenyellow",
    "grey",
    "honeydew",
    "hotpink",
    "indianred",
    "indigo",
    "ivory",
    "khaki",
    "lavender",
    "lavenderblush",
    "lawngreen",
    "lemonchiffon",
    "lightblue",
    "lightcoral",
    "lightcyan",
    "lightgoldenrodyellow",
    "lightgray",
    "lightgreen",
    "lightgrey",
    "lightpink",
    "lightsalmon",
    "lightseagreen",
    "lightskyblue",
    "lightslategray",
    "lightslategrey",
    "lightsteelblue",
    "lightyellow",
    "lime",
    "limegreen",
    "linen",
    "magenta",
    "maroon",
    "mediumaquamarine",
    "mediumblue",
    "mediumorchid",
    "mediumpurple",
    "mediumseagreen",
    "mediumslateblue",
    "mediumspringgreen",
    "mediumturquoise",
    "mediumvioletred",
    "midnightblue",
    "mintcream",
    "mistyrose",
    "moccasin",
    "navajowhite",
    "navy",
    "oldlace",
    "olive",
    "olivedrab",
    "orange",
    "orangered",
    "orchid",
    "palegoldenrod",
    "palegreen",
    "paleturquoise",
    "palevioletred",
    "papayawhip",
    "peachpuff",
    "peru",
    "pink",
    "plum",
    "powderblue",
    "purple",
    "rebeccapurple",
    "red",
    "rosybrown",
    "royalblue",
    "saddlebrown",
    "salmon",
    "sandybrown",
    "seagreen",
    "seashell",
    "sienna",
    "silver",
    "skyblue",
    "slateblue",
    "slategray",
    "slategrey",
    "snow",
    "springgreen",
    "steelblue",
    "tan",
    "teal",
    "thistle",
    "tomato",
    "turquoise",
    "violet",
    "wheat",
    "white",
    "whitesmoke",
    "yellow",
    "yellowgreen",
];

fn is_css_named_color(s: &str) -> bool {
    // ASCII case-insensitive match. The list is sorted lowercase, so
    // we lowercase-equal each candidate. The list is small enough
    // that a linear scan vs binary_search doesn't move the needle in
    // practice.
    if !s.bytes().all(|b| b.is_ascii_alphabetic()) {
        return false;
    }
    let lc: String = s.chars().flat_map(char::to_lowercase).collect();
    CSS_NAMED_COLORS.binary_search(&lc.as_str()).is_ok()
}

/// Map a CSS named color to its `(r, g, b)` triple. Used by
/// `resolve_color` to run named-color arbitrary values
/// (`text-[black]`) through the same alpha-variable wrap as hex
/// (`text-[#000]`). Mirrors upstream's `parseColor` for the named-
/// color short-circuit.
fn named_color_to_rgb(name: &str) -> Option<(u8, u8, u8)> {
    if !name.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    let lc: String = name.chars().flat_map(char::to_lowercase).collect();
    match lc.as_str() {
        "aliceblue" => Some((240, 248, 255)),
        "antiquewhite" => Some((250, 235, 215)),
        "aqua" | "cyan" => Some((0, 255, 255)),
        "aquamarine" => Some((127, 255, 212)),
        "azure" => Some((240, 255, 255)),
        "beige" => Some((245, 245, 220)),
        "bisque" => Some((255, 228, 196)),
        "black" => Some((0, 0, 0)),
        "blanchedalmond" => Some((255, 235, 205)),
        "blue" => Some((0, 0, 255)),
        "blueviolet" => Some((138, 43, 226)),
        "brown" => Some((165, 42, 42)),
        "burlywood" => Some((222, 184, 135)),
        "cadetblue" => Some((95, 158, 160)),
        "chartreuse" => Some((127, 255, 0)),
        "chocolate" => Some((210, 105, 30)),
        "coral" => Some((255, 127, 80)),
        "cornflowerblue" => Some((100, 149, 237)),
        "cornsilk" => Some((255, 248, 220)),
        "crimson" => Some((220, 20, 60)),
        "darkblue" => Some((0, 0, 139)),
        "darkcyan" => Some((0, 139, 139)),
        "darkgoldenrod" => Some((184, 134, 11)),
        "darkgray" | "darkgrey" => Some((169, 169, 169)),
        "darkgreen" => Some((0, 100, 0)),
        "darkkhaki" => Some((189, 183, 107)),
        "darkmagenta" => Some((139, 0, 139)),
        "darkolivegreen" => Some((85, 107, 47)),
        "darkorange" => Some((255, 140, 0)),
        "darkorchid" => Some((153, 50, 204)),
        "darkred" => Some((139, 0, 0)),
        "darksalmon" => Some((233, 150, 122)),
        "darkseagreen" => Some((143, 188, 143)),
        "darkslateblue" => Some((72, 61, 139)),
        "darkslategray" | "darkslategrey" => Some((47, 79, 79)),
        "darkturquoise" => Some((0, 206, 209)),
        "darkviolet" => Some((148, 0, 211)),
        "deeppink" => Some((255, 20, 147)),
        "deepskyblue" => Some((0, 191, 255)),
        "dimgray" | "dimgrey" => Some((105, 105, 105)),
        "dodgerblue" => Some((30, 144, 255)),
        "firebrick" => Some((178, 34, 34)),
        "floralwhite" => Some((255, 250, 240)),
        "forestgreen" => Some((34, 139, 34)),
        "fuchsia" | "magenta" => Some((255, 0, 255)),
        "gainsboro" => Some((220, 220, 220)),
        "ghostwhite" => Some((248, 248, 255)),
        "gold" => Some((255, 215, 0)),
        "goldenrod" => Some((218, 165, 32)),
        "gray" | "grey" => Some((128, 128, 128)),
        "green" => Some((0, 128, 0)),
        "greenyellow" => Some((173, 255, 47)),
        "honeydew" => Some((240, 255, 240)),
        "hotpink" => Some((255, 105, 180)),
        "indianred" => Some((205, 92, 92)),
        "indigo" => Some((75, 0, 130)),
        "ivory" => Some((255, 255, 240)),
        "khaki" => Some((240, 230, 140)),
        "lavender" => Some((230, 230, 250)),
        "lavenderblush" => Some((255, 240, 245)),
        "lawngreen" => Some((124, 252, 0)),
        "lemonchiffon" => Some((255, 250, 205)),
        "lightblue" => Some((173, 216, 230)),
        "lightcoral" => Some((240, 128, 128)),
        "lightcyan" => Some((224, 255, 255)),
        "lightgoldenrodyellow" => Some((250, 250, 210)),
        "lightgray" | "lightgrey" => Some((211, 211, 211)),
        "lightgreen" => Some((144, 238, 144)),
        "lightpink" => Some((255, 182, 193)),
        "lightsalmon" => Some((255, 160, 122)),
        "lightseagreen" => Some((32, 178, 170)),
        "lightskyblue" => Some((135, 206, 250)),
        "lightslategray" | "lightslategrey" => Some((119, 136, 153)),
        "lightsteelblue" => Some((176, 196, 222)),
        "lightyellow" => Some((255, 255, 224)),
        "lime" => Some((0, 255, 0)),
        "limegreen" => Some((50, 205, 50)),
        "linen" => Some((250, 240, 230)),
        "maroon" => Some((128, 0, 0)),
        "mediumaquamarine" => Some((102, 205, 170)),
        "mediumblue" => Some((0, 0, 205)),
        "mediumorchid" => Some((186, 85, 211)),
        "mediumpurple" => Some((147, 112, 219)),
        "mediumseagreen" => Some((60, 179, 113)),
        "mediumslateblue" => Some((123, 104, 238)),
        "mediumspringgreen" => Some((0, 250, 154)),
        "mediumturquoise" => Some((72, 209, 204)),
        "mediumvioletred" => Some((199, 21, 133)),
        "midnightblue" => Some((25, 25, 112)),
        "mintcream" => Some((245, 255, 250)),
        "mistyrose" => Some((255, 228, 225)),
        "moccasin" => Some((255, 228, 181)),
        "navajowhite" => Some((255, 222, 173)),
        "navy" => Some((0, 0, 128)),
        "oldlace" => Some((253, 245, 230)),
        "olive" => Some((128, 128, 0)),
        "olivedrab" => Some((107, 142, 35)),
        "orange" => Some((255, 165, 0)),
        "orangered" => Some((255, 69, 0)),
        "orchid" => Some((218, 112, 214)),
        "palegoldenrod" => Some((238, 232, 170)),
        "palegreen" => Some((152, 251, 152)),
        "paleturquoise" => Some((175, 238, 238)),
        "palevioletred" => Some((219, 112, 147)),
        "papayawhip" => Some((255, 239, 213)),
        "peachpuff" => Some((255, 218, 185)),
        "peru" => Some((205, 133, 63)),
        "pink" => Some((255, 192, 203)),
        "plum" => Some((221, 160, 221)),
        "powderblue" => Some((176, 224, 230)),
        "purple" => Some((128, 0, 128)),
        "rebeccapurple" => Some((102, 51, 153)),
        "red" => Some((255, 0, 0)),
        "rosybrown" => Some((188, 143, 143)),
        "royalblue" => Some((65, 105, 225)),
        "saddlebrown" => Some((139, 69, 19)),
        "salmon" => Some((250, 128, 114)),
        "sandybrown" => Some((244, 164, 96)),
        "seagreen" => Some((46, 139, 87)),
        "seashell" => Some((255, 245, 238)),
        "sienna" => Some((160, 82, 45)),
        "silver" => Some((192, 192, 192)),
        "skyblue" => Some((135, 206, 235)),
        "slateblue" => Some((106, 90, 205)),
        "slategray" | "slategrey" => Some((112, 128, 144)),
        "snow" => Some((255, 250, 250)),
        "springgreen" => Some((0, 255, 127)),
        "steelblue" => Some((70, 130, 180)),
        "tan" => Some((210, 180, 140)),
        "teal" => Some((0, 128, 128)),
        "thistle" => Some((216, 191, 216)),
        "tomato" => Some((255, 99, 71)),
        "turquoise" => Some((64, 224, 208)),
        "violet" => Some((238, 130, 238)),
        "wheat" => Some((245, 222, 179)),
        "white" => Some((255, 255, 255)),
        "whitesmoke" => Some((245, 245, 245)),
        "yellow" => Some((255, 255, 0)),
        "yellowgreen" => Some((154, 205, 50)),
        _ => None,
    }
}

/// Theme keys that carry length-y values (widths, thicknesses, …)
/// — the matching plugins refuse color-shaped AND `var(...)`-shape
/// arbitrary values so the shared-prefix color resolver (with the
/// `'any'` type fallback) gets a turn. List drawn from the
/// `type: ['length', ...]` declarations in upstream's `corePlugins.js`.
fn is_length_y_theme_key(key: &str) -> bool {
    matches!(
        key,
        "borderWidth"
            | "outlineWidth"
            | "textDecorationThickness"
            | "divideWidth"
            | "strokeWidth"
            | "fontSize"
            | "ringOffsetWidth"
    )
}

/// Returns true when `value` is a color shape — hex literal,
/// `rgb()`/`hsl()`/etc. function call, or a CSS named color. Used by
/// length-y Simple resolvers (`borderWidth`, `outlineWidth`, …) to
/// drop the candidate so the matching color resolver picks it up.
/// For multi-token values (whitespace-separated, e.g. `red black`),
/// every token must be a color.
fn is_color_value(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with('#') {
        return true;
    }
    if starts_with_color_function(trimmed) {
        return true;
    }
    if is_css_named_color(trimmed) {
        return true;
    }
    // Multi-token at top level (e.g. `red black` from `border-[red_black]`).
    // We only treat the value as a color if every space-separated
    // token is itself a color shape.
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if tokens.len() > 1
        && tokens
            .iter()
            .all(|t| t.starts_with('#') || starts_with_color_function(t) || is_css_named_color(t))
    {
        return true;
    }
    false
}

/// Strip a Tailwind data-type hint prefix (`length:`, `color:`,
/// `family-name:`, etc.) from the front of an arbitrary value.
/// Returns `(hint, body)` if found. Mirrors the prefix list in
/// `vendor/tailwindcss-v3/src/util/dataTypes.js` — these hints let
/// the user disambiguate when multiple plugins share a class prefix
/// (`font-[number:lighter]` is fontWeight, not fontFamily).
fn data_type_hint(s: &str) -> Option<(&'static str, &str)> {
    const HINTS: &[&str] = &[
        "color",
        "length",
        "line-width",
        "absolute-size",
        "relative-size",
        "percentage",
        "number",
        "url",
        "image",
        "position",
        "shadow",
        "lookup",
        "generic-name",
        "family-name",
        "any",
        "string",
        "list",
        "size",
    ];
    for hint in HINTS {
        if let Some(rest) = s.strip_prefix(hint) {
            if let Some(body) = rest.strip_prefix(':') {
                return Some((hint, body));
            }
        }
    }
    None
}

fn has_unquoted_braces(s: &str) -> bool {
    let bytes = s.as_bytes();
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

pub fn is_arbitrary_value_key(s: &str) -> bool {
    s.starts_with('[') && s.ends_with(']')
}

fn arbitrary_inner(s: &str) -> Option<&str> {
    let inner = s.strip_prefix('[')?.strip_suffix(']')?;
    // Reject obviously malformed arbitrary values whose brackets,
    // parens, or braces don't balance — emitting them would yield
    // invalid CSS like `width: )(`. Mirrors upstream's
    // `validateFormalSyntax` step which drops these candidates.
    if !brackets_balanced(inner) {
        return None;
    }
    // Real CSS values never contain `{` or `}` outside string
    // literals — those open and close declaration blocks. The
    // only thing that does is template-literal residue like
    // `${foo}` swept up by permissive content extractors. Same
    // rationale as `parse_arbitrary_property`'s brace check;
    // mirrors upstream's `isParsableCssValue` rejection.
    if has_unquoted_braces(inner) {
        return None;
    }
    Some(inner)
}

/// Returns true iff every `(` matches a later `)`, `[` matches `]`, and
/// `{` matches `}`. Strings (`"…"` / `'…'`) are skipped so a quoted `(`
/// doesn't influence the balance count. Used to drop arbitrary values
/// whose embedded delimiters would produce unparseable CSS downstream.
fn brackets_balanced(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut stack: Vec<u8> = Vec::with_capacity(8);
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
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
            b'(' | b'[' | b'{' => stack.push(bytes[i]),
            b')' => {
                if stack.pop() != Some(b'(') {
                    return false;
                }
            }
            b']' => {
                if stack.pop() != Some(b'[') {
                    return false;
                }
            }
            b'}' => {
                if stack.pop() != Some(b'{') {
                    return false;
                }
            }
            _ => {}
        }
        i += 1;
    }
    stack.is_empty()
}

/// Map the opacity custom-property name (`--tw-bg-opacity`) to
/// its owning core plugin (`backgroundOpacity`). Used to decide
/// whether to emit the alpha-variable wrapping or skip it
/// (mirrors upstream's `withAlphaVariable` short-circuit when
/// the related opacity plugin is disabled).
fn opacity_plugin_for(opacity_var: &str) -> Option<&'static str> {
    match opacity_var {
        "--tw-bg-opacity" => Some("backgroundOpacity"),
        "--tw-text-opacity" => Some("textOpacity"),
        "--tw-border-opacity" => Some("borderOpacity"),
        "--tw-divide-opacity" => Some("divideOpacity"),
        "--tw-placeholder-opacity" => Some("placeholderOpacity"),
        "--tw-ring-opacity" => Some("ringOpacity"),
        _ => None,
    }
}

fn core_plugin_enabled(config: Option<&Value>, plugin_name: &str) -> bool {
    let Some(cp) = config.and_then(|c| c.get("corePlugins")) else {
        return true;
    };
    if let Some(arr) = cp.as_array() {
        return arr.iter().any(|v| v.as_str() == Some(plugin_name));
    }
    if let Some(obj) = cp.as_object() {
        if let Some(flag) = obj.get(plugin_name).and_then(Value::as_bool) {
            return flag;
        }
        return true;
    }
    if let Some(flag) = cp.as_bool() {
        return flag;
    }
    true
}

/// Convenience wrapper for callers that don't carry config — keeps
/// existing call sites working without a noisy diff. Without config
/// we can't resolve embedded `theme()` calls, so they pass through.
fn normalize_arbitrary(s: &str) -> String {
    normalize_arbitrary_with_config(s, None)
}

/// Tailwind's arbitrary-value `_`-to-space normalize, then math-operator
/// spacing, ported from `vendor/tailwindcss-v3/src/util/dataTypes.js`'s
/// `normalize` and `math-operators.ts`'s `addWhitespaceAroundMathOperators`.
/// When `config` is provided, also resolves embedded `theme(...)` calls
/// in advance — this matches upstream's `evaluateTailwindFunctions`
/// pass which substitutes theme values inside arbitrary CSS values
/// (`w-[theme(spacing.1)]`, `w-[calc(100% - theme('spacing.1'))]`).
fn normalize_arbitrary_with_config(s: &str, config: Option<&Value>) -> String {
    // Custom-property reference shorthand: any value starting with
    // `--` gets wrapped in `var(...)`. Mirrors `normalize()` in
    // upstream's `dataTypes.js`. The wrap applies even when a
    // fallback follows (`--color, #000` → `var(--color, #000)`).
    // The `AUTO_VAR_INJECTION_EXCEPTIONS` list (anchor-name,
    // scroll-timeline-name, etc.) opts out for known dashed-ident
    // properties; that gating belongs at the call site since the
    // property name lives there, not here. For now apply the wrap
    // unconditionally — every utility we map to a property in this
    // crate is outside the exception list.
    if s.starts_with("--") {
        return format!("var({s})");
    }
    // Type-hinted forms (`length:--size-var`, `color:--c`) keep the
    // hint prefix verbatim and wrap the body when it starts with
    // `--`. Without this, `bg-[length:--size-var]` would emit
    // `--size-var` literal instead of `var(--size-var)`.
    if let Some((hint, body)) = data_type_hint(s) {
        if body.starts_with("--") {
            return format!("{hint}:var({body})");
        }
    }
    let underscores_replaced = replace_underscores(s);
    let theme_resolved = if config.is_some() && underscores_replaced.contains("theme(") {
        resolve_theme_calls(&underscores_replaced, config)
    } else {
        underscores_replaced
    };
    // v3.4 inserts a space after commas inside math functions
    // (`min(420px, 50vh)`); v3.3 leaves them tight (`min(420px,50vh)`).
    // Gated by the single `__tailwindVersion` compat key.
    let space_after_comma = !is_v33_compat(config);
    add_whitespace_around_math_operators(&theme_resolved, space_after_comma)
}

/// Walk `input` and replace every top-level `theme(<path>[, <default>])`
/// call with its resolved theme value. Mirrors upstream's
/// `evaluateTailwindFunctions.js` `theme` resolver: looks up the
/// dot-path in `config.theme`, falls back to the supplied default,
/// and stringifies. Used inside arbitrary values so
/// `w-[calc(100%-theme('spacing.1'))]` is fully resolved before the
/// rule emits.
fn resolve_theme_calls(input: &str, config: Option<&Value>) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 6 <= bytes.len() && &bytes[i..i + 6] == b"theme(" {
            // Find balanced close paren.
            let mut depth = 1i32;
            let mut j = i + 6;
            while j < bytes.len() {
                match bytes[j] {
                    b'\\' if j + 1 < bytes.len() => j += 2,
                    b'"' | b'\'' => {
                        let quote = bytes[j];
                        j += 1;
                        while j < bytes.len() && bytes[j] != quote {
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
                        depth += 1;
                        j += 1;
                    }
                    b')' => {
                        depth -= 1;
                        j += 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => j += 1,
                }
            }
            if depth == 0 {
                let arg = &input[i + 6..j - 1];
                let resolved = resolve_theme_arg(arg, config);
                out.push_str(&resolved);
                i = j;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Resolve a single `theme()` argument string (e.g. `'spacing.1'` or
/// `spacing[0.5]` or `colors.red.500, currentColor`). Returns the
/// resolved value, or — if resolution fails — the supplied default
/// (or the literal `theme(<arg>)` form for diagnostics, but in this
/// context we just pass through the original).
fn resolve_theme_arg(arg: &str, config: Option<&Value>) -> String {
    // Split at the first top-level comma to peel off the default.
    let (path_arg, default_arg) = split_theme_first_arg(arg);
    let mut path = path_arg.trim().to_string();
    // Strip surrounding quotes (single or double) so theme('spacing.1')
    // and theme(spacing.1) resolve identically. Mirrors upstream's
    // `path.replace(/^['"]+|['"]+$/g, '')`.
    while path.starts_with('"') || path.starts_with('\'') {
        path.remove(0);
    }
    while path.ends_with('"') || path.ends_with('\'') {
        path.pop();
    }
    // Tokenize into segments. Tailwind's `toPath` returns an array
    // because a key like `'0.5'` is significant — its dot is part of
    // the SEGMENT, not a separator. Bracketed parts (`[0.5]`) become
    // a single segment `'0.5'`; unbracketed parts split on `.`.
    let segments = path_to_segments(&path);
    let resolved = config
        .and_then(|c| c.get("theme"))
        .and_then(|theme| lookup_segments(theme, &segments));
    match resolved {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Array(arr)) => {
            // Tailwind joins arrays as comma-separated lists for
            // `theme()` resolution (e.g. fontFamily.sans).
            arr.iter()
                .filter_map(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .or_else(|| v.as_f64().map(|f| f.to_string()))
                })
                .collect::<Vec<_>>()
                .join(", ")
        }
        _ => match default_arg {
            Some(d) => d.trim().to_string(),
            None => format!("theme({arg})"),
        },
    }
}

/// `spacing[0.5]` → `["spacing", "0.5"]`. Mirrors upstream's `toPath`:
/// bracketed segments are kept whole (so a key like `'0.5'` resolves
/// against an object that literally has `'0.5'` as its key), while
/// unbracketed parts split on `.`. Used by `theme()` resolution
/// inside arbitrary values.
fn path_to_segments(path: &str) -> Vec<String> {
    let bytes = path.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'.' => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
                i += 1;
            }
            b'[' => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
                i += 1;
                while i < bytes.len() && bytes[i] != b']' {
                    current.push(bytes[i] as char);
                    i += 1;
                }
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
                if i < bytes.len() {
                    i += 1;
                }
            }
            _ => {
                current.push(bytes[i] as char);
                i += 1;
            }
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn lookup_segments<'a>(root: &'a Value, segments: &[String]) -> Option<&'a Value> {
    let mut node = root;
    for seg in segments {
        node = node.get(seg.as_str())?;
    }
    Some(node)
}

/// Split at the first top-level comma — separates the theme path
/// from the optional default value. Identical to `directives::
/// split_first_arg` but kept local to avoid a cross-module dep.
fn split_theme_first_arg(arg: &str) -> (&str, Option<&str>) {
    let bytes = arg.as_bytes();
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
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
            b',' if depth_paren == 0 && depth_bracket == 0 => {
                return (&arg[..i], Some(&arg[i + 1..]));
            }
            _ => {}
        }
        i += 1;
    }
    (arg, None)
}

fn replace_underscores(s: &str) -> String {
    // Mirrors upstream's `normalize()` in `dataTypes.js`: outside of
    // `url(...)` we replace `_` with ` ` (except `\_` → `_`); inside
    // `url(...)` the substring is preserved verbatim so URLs like
    // `url('brown_potato.jpg')` keep their underscores.
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        // Detect `url(` — preserve the entire balanced-paren slice.
        if bytes[i] == b'u' && i + 4 <= bytes.len() && &bytes[i..i + 4] == b"url(" {
            let start = i;
            // Walk forward to the matching `)`. URLs in Tailwind are
            // unlikely to contain nested parens, but we track depth
            // anyway so cases like `url(data:image/svg+xml;base64,…)`
            // with stray `(` survive.
            let mut depth = 1i32;
            let mut j = i + 4;
            while j < bytes.len() {
                match bytes[j] {
                    b'\\' if j + 1 < bytes.len() => j += 2,
                    b'(' => {
                        depth += 1;
                        j += 1;
                    }
                    b')' => {
                        depth -= 1;
                        j += 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => j += 1,
                }
            }
            // Push the verbatim url(...) chunk.
            out.push_str(&s[start..j]);
            i = j;
            continue;
        }
        if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'_' {
            out.push('_');
            i += 2;
            continue;
        }
        if bytes[i] == b'_' {
            out.push(' ');
            i += 1;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

const MATH_FUNCTIONS: &[&str] = &[
    "calc", "min", "max", "clamp", "mod", "rem", "sin", "cos", "tan", "asin", "acos", "atan",
    "atan2", "pow", "sqrt", "hypot", "log", "exp", "round",
];

/// Direct port of `addWhitespaceAroundMathOperators` from
/// `vendor/tailwindcss-v3/src/util/math-operators.ts`. Mirroring the
/// algorithm byte-for-byte is the only way to get parity with the
/// oracle on `calc(50%-1rem)` -> `calc(50% - 1rem)`.
fn add_whitespace_around_math_operators(input: &str, space_after_comma: bool) -> String {
    if !MATH_FUNCTIONS.iter().any(|fn_name| input.contains(fn_name)) {
        return input.to_string();
    }

    let bytes = input.as_bytes();
    let mut result: Vec<u8> = Vec::with_capacity(input.len());
    // `formattable` is a stack: `formattable[0]` is the innermost paren
    // group's "are we inside a known math function?" flag. The JS source
    // uses an array with `unshift`/`shift`; we use Vec and treat index 0
    // as the front for fidelity.
    let mut formattable: Vec<bool> = Vec::new();
    let mut value_pos: Option<usize> = None;
    let mut last_value_pos: Option<usize> = None;

    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i];

        // Track digit / unit positions so we know "the previous position
        // was a value" for operator-spacing decisions. Digits OR (a unit
        // char that follows a digit) extend the value-position; anything
        // else snapshots it into `last_value_pos` and clears.
        let extends_value = ch.is_ascii_digit()
            || (value_pos.is_some() && (ch == b'%' || ch.is_ascii_alphabetic()));
        if extends_value {
            value_pos = Some(i);
        } else {
            last_value_pos = value_pos;
            value_pos = None;
        }

        match ch {
            b'(' => {
                result.push(ch);
                // Walk back over alphanumeric chars to find the function name.
                let mut start = i;
                let mut j = i;
                while j > 0 {
                    let inner = bytes[j - 1];
                    if inner.is_ascii_digit() || (inner.is_ascii_lowercase()) {
                        start = j - 1;
                        j -= 1;
                    } else {
                        break;
                    }
                }
                let fn_name = &input[start..i];
                if MATH_FUNCTIONS.contains(&fn_name) {
                    formattable.insert(0, true);
                } else if matches!(formattable.first(), Some(true)) && fn_name.is_empty() {
                    // Nested parens inside a math function: keep formatting
                    // until the inner paren closes.
                    formattable.insert(0, true);
                } else {
                    formattable.insert(0, false);
                }
                i += 1;
                continue;
            }
            b')' => {
                result.push(ch);
                if !formattable.is_empty() {
                    formattable.remove(0);
                }
                i += 1;
                continue;
            }
            b',' if matches!(formattable.first(), Some(true)) => {
                if space_after_comma {
                    result.extend_from_slice(b", ");
                } else {
                    result.push(b',');
                }
                i += 1;
                continue;
            }
            b' ' if matches!(formattable.first(), Some(true)) && result.last() == Some(&b' ') => {
                // Skip consecutive whitespace inside a math function.
                i += 1;
                continue;
            }
            b'+' | b'*' | b'/' | b'-' if matches!(formattable.first(), Some(true)) => {
                let trimmed = trim_end_spaces(&result);
                let prev = trimmed.last().copied();
                let prev_prev = trimmed.len().checked_sub(2).map(|idx| trimmed[idx]);
                let next = bytes.get(i + 1).copied();

                // Scientific notation guard: `3.4e-2` shouldn't become `3.4 e -2`.
                if matches!(prev, Some(b'e') | Some(b'E'))
                    && matches!(prev_prev, Some(c) if c.is_ascii_digit())
                {
                    result.push(ch);
                    i += 1;
                    continue;
                }

                // Already preceded by an operator: don't add spaces.
                if matches!(prev, Some(b'+') | Some(b'-') | Some(b'*') | Some(b'/')) {
                    result.push(ch);
                    i += 1;
                    continue;
                }

                // Beginning of an argument: don't add spaces.
                if matches!(prev, Some(b'(') | Some(b',')) {
                    result.push(ch);
                    i += 1;
                    continue;
                }

                // If the previous char in the *input* (not the trimmed
                // result) is a space, only add a space after the operator.
                if i > 0 && bytes[i - 1] == b' ' {
                    result.push(ch);
                    result.push(b' ');
                    i += 1;
                    continue;
                }

                let prev_is_digit = matches!(prev, Some(c) if c.is_ascii_digit());
                let next_is_digit = matches!(next, Some(c) if c.is_ascii_digit());
                let prev_is_close_paren = matches!(prev, Some(b')'));
                let next_is_open_paren = matches!(next, Some(b'('));
                let next_is_operator =
                    matches!(next, Some(b'+') | Some(b'-') | Some(b'*') | Some(b'/'));
                let prev_was_value_unit = last_value_pos == Some(i.saturating_sub(1));

                if prev_is_digit
                    || next_is_digit
                    || prev_is_close_paren
                    || next_is_open_paren
                    || next_is_operator
                    || prev_was_value_unit
                {
                    result.push(b' ');
                    result.push(ch);
                    result.push(b' ');
                } else {
                    result.push(ch);
                }
                i += 1;
                continue;
            }
            _ => {}
        }

        result.push(ch);
        i += 1;
    }

    String::from_utf8(result).expect("only ASCII bytes pushed")
}

fn trim_end_spaces(buf: &[u8]) -> &[u8] {
    let mut end = buf.len();
    while end > 0 && buf[end - 1] == b' ' {
        end -= 1;
    }
    &buf[..end]
}

/// Whether `negate_value` would return a non-`undefined` result. Mirrors
/// the discriminant in Tailwind's `negateValue.js`: numeric strings,
/// `var()` / `calc()` / `min()` / `max()` / `clamp()` calls.
fn is_negatable(value: &str) -> bool {
    if value == "0" {
        return true;
    }
    if matches_numeric_token(value) {
        return true;
    }
    const NUMERIC_FNS: &[&str] = &["var", "calc", "min", "max", "clamp"];
    NUMERIC_FNS
        .iter()
        .any(|fn_name| value.contains(&format!("{fn_name}(")))
}

/// Mirror of `negateValue.js` from `vendor/tailwindcss-v3/src/util/negateValue.js`.
/// Returns the negated form. Caller should `is_negatable`-check first.
fn negate(value: &str) -> String {
    if value == "0" {
        return "0".to_string();
    }
    if matches_numeric_token(value) {
        // Flip leading sign (or insert `-`).
        if let Some(rest) = value.strip_prefix('-') {
            return rest.to_string();
        }
        if let Some(rest) = value.strip_prefix('+') {
            return format!("-{rest}");
        }
        return format!("-{value}");
    }
    // Function-call form: wrap in `calc(<value> * -1)`.
    format!("calc({value} * -1)")
}

/// Mirrors `/^[+-]?(\d+|\d*\.\d+)(e[+-]?\d+)?(%|\w+)?$/` from negateValue.js.
fn matches_numeric_token(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    // (\d+ | \d*\.\d+)
    let int_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let had_int = i > int_start;
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == frac_start {
            return false; // `<digits>.` with no fractional digits doesn't match
        }
    } else if !had_int {
        return false;
    }
    // (e[+-]?\d+)? — only consume `e` as exponent if it's actually
    // followed by an optional sign and at least one digit. Otherwise
    // leave the `e` for the `\w+` unit match (e.g. `em`, `ex`).
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        let mut probe = i + 1;
        if probe < bytes.len() && (bytes[probe] == b'+' || bytes[probe] == b'-') {
            probe += 1;
        }
        let exp_start = probe;
        while probe < bytes.len() && bytes[probe].is_ascii_digit() {
            probe += 1;
        }
        if probe > exp_start {
            // Exponent matched.
            i = probe;
        }
    }
    // (%|\w+)?
    if i < bytes.len() {
        if bytes[i] == b'%' {
            i += 1;
        } else {
            // `\w` = [A-Za-z0-9_]
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
        }
    }
    i == bytes.len()
}

fn value_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        // Object values like `{ DEFAULT: 'red', light: '#ffeeee' }`
        // resolve to the `DEFAULT` member when used as a bare
        // value (e.g. `bg-theme` looking up
        // `theme.backgroundColor.theme`). Mirrors upstream's
        // `transformThemeValue` handling for nested colour
        // palettes that include a `DEFAULT`.
        Value::Object(map) => map.get("DEFAULT").and_then(|d| match d {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use galeforce_parser::{parse, Modifier, ParseOptions};

    fn parse_candidate(raw: &str) -> ParsedCandidate {
        parse(raw, &ParseOptions::default()).expect("parse")
    }

    #[test]
    fn finds_long_prefix_first() {
        let (util, key) = find_value_utility("mx-4").unwrap();
        assert_eq!(util.class_prefix, "mx");
        assert_eq!(key, "4");
    }

    #[test]
    fn distinguishes_m_from_mx() {
        let (util, key) = find_value_utility("m-4").unwrap();
        assert_eq!(util.class_prefix, "m");
        assert_eq!(key, "4");
    }

    #[test]
    fn bare_prefix_resolves_to_default_key() {
        let (util, key) = find_value_utility("p").unwrap();
        assert_eq!(util.class_prefix, "p");
        assert_eq!(key, "DEFAULT");
    }

    #[test]
    fn unknown_utility_returns_none() {
        assert!(find_value_utility("not-a-thing").is_none());
    }

    fn first_value(util: &ValueUtility, key: &str, p: &ParsedCandidate) -> Option<String> {
        resolve_value(util, key, p, None).and_then(|decls| decls.into_iter().next().map(|(_, v)| v))
    }

    #[test]
    fn resolve_value_reads_default_spacing() {
        let (util, key) = find_value_utility("mt-4").unwrap();
        let p = parse_candidate("mt-4");
        assert_eq!(first_value(util, key, &p).as_deref(), Some("1rem"));
    }

    #[test]
    fn resolve_value_negates() {
        let (util, key) = find_value_utility("mt-4").unwrap();
        let p = parse_candidate("-mt-4");
        assert_eq!(first_value(util, key, &p).as_deref(), Some("-1rem"));
    }

    #[test]
    fn resolve_value_negative_drops_auto() {
        let (util, key) = find_value_utility("mt-auto").unwrap();
        let p = parse_candidate("-mt-auto");
        assert!(resolve_value(util, key, &p, None).is_none());
    }

    #[test]
    fn resolve_value_arbitrary_uses_inner_value() {
        let (util, key) = find_value_utility("mt-[12px]").unwrap();
        let p = parse_candidate("mt-[12px]");
        assert_eq!(first_value(util, key, &p).as_deref(), Some("12px"));
    }

    #[test]
    fn resolve_value_arbitrary_supports_negation() {
        let (util, key) = find_value_utility("mt-[12px]").unwrap();
        let p = parse_candidate("-mt-[12px]");
        assert_eq!(first_value(util, key, &p).as_deref(), Some("-12px"));
    }

    #[test]
    fn padding_does_not_support_negation() {
        let (util, key) = find_value_utility("pt-4").unwrap();
        let p = parse_candidate("-pt-4");
        assert!(resolve_value(util, key, &p, None).is_none());
    }

    #[test]
    fn modifier_disqualifies_simple_value_resolution() {
        let (util, key) = find_value_utility("mt-4").unwrap();
        let mut p = parse_candidate("mt-4");
        p.modifier = Some(Modifier::Named("50"));
        assert!(resolve_value(util, key, &p, None).is_none());
    }

    #[test]
    fn resolve_value_reads_user_config_over_default() {
        let cfg = serde_json::json!({
            "theme": { "margin": { "huge": "999px" } }
        });
        let (util, key) = find_value_utility("mt-huge").unwrap();
        let p = parse_candidate("mt-huge");
        let decls = resolve_value(util, key, &p, Some(&cfg)).unwrap();
        assert_eq!(decls.decls, vec![("margin-top".into(), "999px".into())]);
        assert_eq!(decls.selector_suffix, None);
    }

    // ---- typography ----

    #[test]
    fn font_prefix_returns_two_matches_in_order() {
        let matches = find_value_utilities("font");
        // fontWeight first (declaration order), then fontFamily.
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].0.theme_key, "fontWeight");
        assert_eq!(matches[1].0.theme_key, "fontFamily");
    }

    #[test]
    fn font_bold_resolves_through_font_weight() {
        let matches = find_value_utilities("font-bold");
        // The compiler walks matches; we mirror that here.
        let p = parse_candidate("font-bold");
        let mut decls: Option<Vec<(String, String)>> = None;
        for (util, key) in matches {
            if let Some(d) = resolve_value(util, key, &p, None) {
                decls = Some(d.decls);
                break;
            }
        }
        assert_eq!(decls, Some(vec![("font-weight".into(), "700".into())]));
    }

    #[test]
    fn font_sans_resolves_through_font_family_array_join() {
        // Need a config that brings the default fontFamily into our
        // theme view — Rust-side default doesn't include fontFamily yet
        // (carry-over until we port the default-theme arrays). We pass
        // the resolved tuple manually for the test.
        let cfg = serde_json::json!({
            "theme": { "fontFamily": { "sans": ["ui-sans-serif", "system-ui", "sans-serif"] } }
        });
        let matches = find_value_utilities("font-sans");
        let p = parse_candidate("font-sans");
        let mut decls: Option<Vec<(String, String)>> = None;
        for (util, key) in matches {
            if let Some(d) = resolve_value(util, key, &p, Some(&cfg)) {
                decls = Some(d.decls);
                break;
            }
        }
        assert_eq!(
            decls,
            Some(vec![(
                "font-family".into(),
                "ui-sans-serif, system-ui, sans-serif".into()
            )])
        );
    }

    #[test]
    fn text_sm_unpacks_tuple_into_font_size_and_line_height() {
        let cfg = serde_json::json!({
            "theme": { "fontSize": { "sm": ["0.875rem", { "lineHeight": "1.25rem" }] } }
        });
        let matches = find_value_utilities("text-sm");
        let p = parse_candidate("text-sm");
        let mut decls: Option<Vec<(String, String)>> = None;
        for (util, key) in matches {
            if let Some(d) = resolve_value(util, key, &p, Some(&cfg)) {
                decls = Some(d.decls);
                break;
            }
        }
        assert_eq!(
            decls,
            Some(vec![
                ("font-size".into(), "0.875rem".into()),
                ("line-height".into(), "1.25rem".into()),
            ])
        );
    }

    #[test]
    fn text_with_slash_modifier_overrides_line_height() {
        let cfg = serde_json::json!({
            "theme": {
                "fontSize": { "sm": ["0.875rem", { "lineHeight": "1.25rem" }] },
                "lineHeight": { "6": "1.5rem" }
            }
        });
        let matches = find_value_utilities("text-sm");
        let mut p = parse_candidate("text-sm");
        p.modifier = Some(Modifier::Named("6"));
        let mut decls: Option<Vec<(String, String)>> = None;
        for (util, key) in matches {
            if let Some(d) = resolve_value(util, key, &p, Some(&cfg)) {
                decls = Some(d.decls);
                break;
            }
        }
        assert_eq!(
            decls,
            Some(vec![
                ("font-size".into(), "0.875rem".into()),
                ("line-height".into(), "1.5rem".into()),
            ])
        );
    }

    #[test]
    fn leading_resolves_via_default_line_height() {
        // No Rust-side lineHeight default yet (carry-over); skip when
        // the theme key is absent.
        let cfg = serde_json::json!({
            "theme": { "lineHeight": { "tight": "1.25" } }
        });
        let (util, key) = find_value_utility("leading-tight").unwrap();
        let p = parse_candidate("leading-tight");
        let decls = resolve_value(util, key, &p, Some(&cfg)).unwrap();
        assert_eq!(decls.decls, vec![("line-height".into(), "1.25".into())]);
    }

    #[test]
    fn tracking_supports_negative_letter_spacing() {
        let cfg = serde_json::json!({
            "theme": { "letterSpacing": { "tighter": "-0.05em" } }
        });
        let (util, key) = find_value_utility("tracking-tighter").unwrap();
        let p = parse_candidate("-tracking-tighter");
        // `-(-0.05em)` -> `0.05em` after Tailwind's negate.
        let decls = resolve_value(util, key, &p, Some(&cfg)).unwrap();
        assert_eq!(
            decls.decls,
            vec![("letter-spacing".into(), "0.05em".into())]
        );
    }

    #[test]
    fn space_x_emits_sibling_pair_selector_suffix() {
        let (util, key) = find_value_utility("space-x-4").unwrap();
        let p = parse_candidate("space-x-4");
        let r = resolve_value(util, key, &p, None).unwrap();
        assert_eq!(
            r.selector_suffix,
            Some(" > :not([hidden]) ~ :not([hidden])")
        );
        assert!(r.decls.iter().any(|(p, _)| p == "--tw-space-x-reverse"));
        // Theme.spacing.4 = '1rem'. Margin-right should reference it.
        assert!(r
            .decls
            .iter()
            .any(|(_, v)| v.contains("1rem * var(--tw-space-x-reverse)")));
    }

    #[test]
    fn space_x_zero_uses_zero_px_for_calc() {
        // Tailwind special-cases `0` -> `0px` so the calc()
        // arithmetic doesn't fail. Mirrors upstream's
        // `value === '0' ? '0px' : value`.
        let (util, key) = find_value_utility("space-x-0").unwrap();
        let p = parse_candidate("space-x-0");
        let r = resolve_value(util, key, &p, None).unwrap();
        assert!(r.decls.iter().any(|(_, v)| v.contains("0px")));
    }
}
