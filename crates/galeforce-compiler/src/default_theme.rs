//! Subset of Tailwind 3.4.19's default theme that GaleforceCSS needs at the
//! Rust layer.
//!
//! The full default theme is at
//! `vendor/tailwindcss-v3/stubs/config.full.js` and runs to ~1000 lines —
//! most of which describe value-bearing utility families we haven't shipped
//! yet. We port the table for each family as it lands; today that's
//! spacing (and the margin/padding tables that derive from it).
//!
//! User configs are still resolved JS-side via Tailwind's own
//! `resolveConfig.js` (`@cx/galeforcecss-config-loader`), so this Rust-side
//! default is only consulted when no config is supplied — the conformance
//! harness, the `compile-json` CLI without `--config`, etc. When a user
//! passes a resolved config through `CompileOptions.config`, we read from
//! that and ignore these defaults.

use std::sync::OnceLock;

use serde_json::{Map, Value};

use crate::colors::flattened_color_table;

/// Cached default theme, built once. Hot loops through resolvers
/// previously called `default_theme()` per candidate which rebuilt the
/// ~1000-line tree every time — observable in the bench (`bg-red-500`
/// at 318µs/candidate, ~98% of which was JSON tree allocation).
static DEFAULT_THEME: OnceLock<Value> = OnceLock::new();

/// Borrow the cached default theme. The returned `&Value` is valid for
/// the lifetime of the process. Prefer this over `default_theme()` in
/// hot paths.
pub fn default_theme_ref() -> &'static Value {
    DEFAULT_THEME.get_or_init(build_default_theme)
}

/// Build a JSON `theme` object containing every default we currently
/// honor at the Rust layer. Most callers want `default_theme_ref()` —
/// this owned form exists for code that mutates the result (theme
/// merging in the test suite).
#[cfg(test)]
pub fn default_theme() -> Value {
    build_default_theme()
}

fn build_default_theme() -> Value {
    let mut theme = Map::new();
    theme.insert("spacing".into(), spacing());
    theme.insert("margin".into(), margin_table());
    theme.insert("padding".into(), spacing());
    theme.insert("space".into(), spacing());
    theme.insert("width".into(), width_table());
    theme.insert("height".into(), height_table());
    theme.insert("size".into(), size_table());
    theme.insert("minWidth".into(), min_width_table());
    theme.insert("minHeight".into(), min_height_table());
    theme.insert("maxWidth".into(), max_width_table());
    theme.insert("maxHeight".into(), max_height_table());
    theme.insert("fontFamily".into(), font_family_table());
    theme.insert("fontSize".into(), font_size_table());
    theme.insert("fontWeight".into(), font_weight_table());
    theme.insert("lineHeight".into(), line_height_table());
    theme.insert("letterSpacing".into(), letter_spacing_table());
    theme.insert("textIndent".into(), spacing());
    let colors = flattened_color_table();
    theme.insert("colors".into(), colors.clone());
    theme.insert("backgroundColor".into(), colors.clone());
    theme.insert("textColor".into(), colors.clone());
    // Tailwind v3 `borderColor: { ...colors, DEFAULT: gray-200 }`
    // (config.full.js:82-85). Without DEFAULT, bare `border` (no
    // color suffix) falls through to currentColor instead of the
    // theme's gray-200 (#e5e7eb).
    theme.insert("borderColor".into(), {
        let mut o = match colors.clone() {
            Value::Object(o) => o,
            _ => Map::new(),
        };
        o.insert("DEFAULT".into(), Value::String("#e5e7eb".into()));
        Value::Object(o)
    });
    theme.insert("textDecorationColor".into(), colors.clone());
    theme.insert("placeholderColor".into(), colors.clone());
    theme.insert("ringColor".into(), colors.clone());
    // `caretColor` is later overwritten with `colors_for_extras` below
    // (same value, but the second assignment is the canonical one
    // alongside `accentColor`'s extra). Skipping the early insert
    // keeps the build order linear and matches the upstream theme
    // resolution sequence in `vendor/tailwindcss-v3/stubs/config.full.js`.
    theme.insert("accentColor".into(), colors.clone());
    // Tailwind v3 `fill: { none: 'none', ...theme('colors') }`
    // (config.full.js:253-256). Missing `none` meant `fill-none`
    // didn't resolve.
    theme.insert("fill".into(), {
        let mut o = match colors.clone() {
            Value::Object(o) => o,
            _ => Map::new(),
        };
        o.insert("none".into(), Value::String("none".into()));
        Value::Object(o)
    });
    // Tailwind v3 `stroke: { none: 'none', ...theme('colors') }`
    // (config.full.js:878-881). Same `none` rationale as `fill`.
    theme.insert("stroke".into(), {
        let mut o = match colors {
            Value::Object(o) => o,
            _ => Map::new(),
        };
        o.insert("none".into(), Value::String("none".into()));
        Value::Object(o)
    });
    theme.insert("opacity".into(), opacity_table());
    // Per-property opacity scales — every one of these defaults to
    // `theme.opacity` per upstream's `defaultConfig`. Listing them
    // explicitly so the Simple resolver can read each via its own
    // theme key (no fallback chain).
    theme.insert("backgroundOpacity".into(), opacity_table());
    theme.insert("borderOpacity".into(), opacity_table());
    theme.insert("divideOpacity".into(), opacity_table());
    theme.insert("placeholderOpacity".into(), opacity_table());
    theme.insert("textOpacity".into(), opacity_table());
    theme.insert("translate".into(), translate_table());
    theme.insert("rotate".into(), rotate_table());
    theme.insert("scale".into(), scale_table());
    theme.insert("skew".into(), skew_table());
    theme.insert("borderWidth".into(), border_width_table());
    theme.insert("borderRadius".into(), border_radius_table());
    theme.insert("borderSpacing".into(), spacing());
    theme.insert("flex".into(), flex_table());
    theme.insert("flexBasis".into(), flex_basis_table());
    theme.insert("flexGrow".into(), flex_grow_table());
    theme.insert("flexShrink".into(), flex_shrink_table());
    theme.insert("order".into(), order_table());
    theme.insert("gap".into(), spacing());
    theme.insert("gridTemplateColumns".into(), grid_template_table());
    theme.insert("gridTemplateRows".into(), grid_template_table());
    theme.insert("gridColumn".into(), grid_column_table());
    theme.insert("gridRow".into(), grid_column_table());
    theme.insert("gridColumnStart".into(), grid_start_end_table());
    theme.insert("gridColumnEnd".into(), grid_start_end_table());
    theme.insert("gridRowStart".into(), grid_start_end_table());
    theme.insert("gridRowEnd".into(), grid_start_end_table());
    theme.insert("gridAutoColumns".into(), grid_auto_table());
    theme.insert("gridAutoRows".into(), grid_auto_table());
    theme.insert("blur".into(), blur_table());
    theme.insert("backdropBlur".into(), blur_table());
    theme.insert("brightness".into(), brightness_table());
    theme.insert("backdropBrightness".into(), brightness_table());
    theme.insert("contrast".into(), contrast_table());
    theme.insert("backdropContrast".into(), contrast_table());
    theme.insert("grayscale".into(), grayscale_invert_sepia_table());
    theme.insert("backdropGrayscale".into(), grayscale_invert_sepia_table());
    theme.insert("invert".into(), grayscale_invert_sepia_table());
    theme.insert("backdropInvert".into(), grayscale_invert_sepia_table());
    theme.insert("sepia".into(), grayscale_invert_sepia_table());
    theme.insert("backdropSepia".into(), grayscale_invert_sepia_table());
    theme.insert("hueRotate".into(), hue_rotate_table());
    theme.insert("backdropHueRotate".into(), hue_rotate_table());
    theme.insert("saturate".into(), saturate_table());
    theme.insert("backdropSaturate".into(), saturate_table());
    theme.insert("backdropOpacity".into(), opacity_table());
    theme.insert("dropShadow".into(), drop_shadow_table());
    theme.insert("outlineWidth".into(), outline_width_table());
    theme.insert("outlineOffset".into(), outline_offset_table());
    let colors_for_outline = flattened_color_table();
    theme.insert("outlineColor".into(), colors_for_outline);
    theme.insert("inset".into(), inset_table());
    theme.insert("zIndex".into(), z_index_table());
    theme.insert("cursor".into(), cursor_table());
    theme.insert("transitionProperty".into(), transition_property_table());
    theme.insert("transitionDuration".into(), transition_duration_table());
    theme.insert(
        "transitionTimingFunction".into(),
        transition_timing_function_table(),
    );
    theme.insert("transitionDelay".into(), transition_delay_table());
    theme.insert("animation".into(), animation_table());
    theme.insert("scrollMargin".into(), scroll_margin_table());
    theme.insert("scrollPadding".into(), spacing());
    theme.insert("backgroundImage".into(), background_image_table());
    theme.insert("backgroundPosition".into(), background_position_table());
    theme.insert("backgroundSize".into(), background_size_table());
    theme.insert("listStyleType".into(), list_style_type_table());
    theme.insert("listStyleImage".into(), list_style_image_table());
    let colors_for_extras = flattened_color_table();
    theme.insert("accentColor".into(), {
        let mut o = match colors_for_extras.clone() {
            Value::Object(o) => o,
            _ => Map::new(),
        };
        insert_string(&mut o, "auto", "auto");
        Value::Object(o)
    });
    theme.insert("caretColor".into(), colors_for_extras);
    // `space` and `divide*` derive from `spacing` / `borderWidth` /
    // `borderColor` per upstream's
    //   space:        ({theme}) => theme('spacing')
    //   divideWidth:  ({theme}) => theme('borderWidth')
    //   divideColor:  ({theme}) => theme('borderColor')
    theme.insert("divideWidth".into(), border_width_table());
    theme.insert("divideColor".into(), flattened_color_table());
    theme.insert("boxShadow".into(), box_shadow_table());
    theme.insert("boxShadowColor".into(), flattened_color_table());
    theme.insert("ringWidth".into(), ring_width_table());
    theme.insert("ringColor".into(), {
        let mut o = match flattened_color_table() {
            Value::Object(o) => o,
            _ => Map::new(),
        };
        // Tailwind 3's default ring color (DEFAULT) is sky-blue.
        o.insert("DEFAULT".into(), Value::String("#3b82f6".into()));
        Value::Object(o)
    });
    theme.insert("ringOffsetWidth".into(), ring_offset_width_table());
    theme.insert("ringOffsetColor".into(), flattened_color_table());
    theme.insert("ringOpacity".into(), {
        let mut o = match opacity_table() {
            Value::Object(o) => o,
            _ => Map::new(),
        };
        o.insert("DEFAULT".into(), Value::String("0.5".into()));
        Value::Object(o)
    });
    theme.insert("gradientColorStops".into(), flattened_color_table());
    theme.insert(
        "gradientColorStopPositions".into(),
        gradient_position_table(),
    );
    theme.insert("screens".into(), Value::Object(screens()));
    theme.insert("lineClamp".into(), line_clamp_table());
    theme.insert("textUnderlineOffset".into(), text_underline_offset_table());
    theme.insert(
        "textDecorationThickness".into(),
        text_decoration_thickness_table(),
    );
    theme.insert("textDecorationColor".into(), flattened_color_table());
    theme.insert("placeholderColor".into(), flattened_color_table());
    theme.insert("content".into(), content_table());
    theme.insert("strokeWidth".into(), stroke_width_table());
    theme.insert("columns".into(), columns_table());
    theme.insert("aspectRatio".into(), aspect_ratio_table());
    theme.insert("objectPosition".into(), object_position_table());
    theme.insert("willChange".into(), will_change_table());
    theme.insert("transformOrigin".into(), transform_origin_table());
    Value::Object(theme)
}

/// Tailwind v3 `theme.transformOrigin` (config.full.js:912-922): eight
/// named keywords. Galeforce previously handled these via
/// STATIC_UTILITIES + an arbitrary-form fallback in `value_utilities.rs`,
/// but a registered theme table lets user `theme.extend.transformOrigin`
/// overrides flow through the value-utility plugin path consistently.
fn transform_origin_table() -> Value {
    let mut obj = Map::new();
    insert_string(&mut obj, "center", "center");
    insert_string(&mut obj, "top", "top");
    insert_string(&mut obj, "top-right", "top right");
    insert_string(&mut obj, "right", "right");
    insert_string(&mut obj, "bottom-right", "bottom right");
    insert_string(&mut obj, "bottom", "bottom");
    insert_string(&mut obj, "bottom-left", "bottom left");
    insert_string(&mut obj, "left", "left");
    insert_string(&mut obj, "top-left", "top left");
    Value::Object(obj)
}

fn stroke_width_table() -> Value {
    let mut t = Map::new();
    for n in [0u32, 1, 2] {
        t.insert(n.to_string(), Value::String(n.to_string()));
    }
    Value::Object(t)
}

fn aspect_ratio_table() -> Value {
    let mut t = Map::new();
    t.insert("auto".into(), Value::String("auto".into()));
    t.insert("square".into(), Value::String("1 / 1".into()));
    t.insert("video".into(), Value::String("16 / 9".into()));
    Value::Object(t)
}

fn object_position_table() -> Value {
    let mut t = Map::new();
    for (k, v) in [
        ("bottom", "bottom"),
        ("center", "center"),
        ("left", "left"),
        ("left-bottom", "left bottom"),
        ("left-top", "left top"),
        ("right", "right"),
        ("right-bottom", "right bottom"),
        ("right-top", "right top"),
        ("top", "top"),
    ] {
        t.insert(k.into(), Value::String(v.into()));
    }
    Value::Object(t)
}

fn will_change_table() -> Value {
    let mut t = Map::new();
    for (k, v) in [
        ("auto", "auto"),
        ("scroll", "scroll-position"),
        ("contents", "contents"),
        ("transform", "transform"),
    ] {
        t.insert(k.into(), Value::String(v.into()));
    }
    Value::Object(t)
}

fn columns_table() -> Value {
    let mut t = Map::new();
    t.insert("auto".into(), Value::String("auto".into()));
    for n in 1u32..=12 {
        t.insert(n.to_string(), Value::String(n.to_string()));
    }
    for (k, v) in [
        ("3xs", "16rem"),
        ("2xs", "18rem"),
        ("xs", "20rem"),
        ("sm", "24rem"),
        ("md", "28rem"),
        ("lg", "32rem"),
        ("xl", "36rem"),
        ("2xl", "42rem"),
        ("3xl", "48rem"),
        ("4xl", "56rem"),
        ("5xl", "64rem"),
        ("6xl", "72rem"),
        ("7xl", "80rem"),
    ] {
        t.insert(k.into(), Value::String(v.into()));
    }
    Value::Object(t)
}

fn text_decoration_thickness_table() -> Value {
    let mut t = Map::new();
    t.insert("auto".into(), Value::String("auto".into()));
    t.insert("from-font".into(), Value::String("from-font".into()));
    for n in [0u32, 1, 2, 4, 8] {
        t.insert(n.to_string(), Value::String(format!("{n}px")));
    }
    Value::Object(t)
}

fn content_table() -> Value {
    let mut t = Map::new();
    t.insert("none".into(), Value::String("none".into()));
    Value::Object(t)
}

fn line_clamp_table() -> Value {
    let mut t = Map::new();
    for n in 1..=6 {
        t.insert(n.to_string(), Value::String(n.to_string()));
    }
    Value::Object(t)
}

fn text_underline_offset_table() -> Value {
    let mut t = Map::new();
    t.insert("auto".into(), Value::String("auto".into()));
    for n in [0u32, 1, 2, 4, 8] {
        t.insert(n.to_string(), Value::String(format!("{n}px")));
    }
    Value::Object(t)
}

fn box_shadow_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("sm", "0 1px 2px 0 rgb(0 0 0 / 0.05)"),
        (
            "DEFAULT",
            "0 1px 3px 0 rgb(0 0 0 / 0.1), 0 1px 2px -1px rgb(0 0 0 / 0.1)",
        ),
        (
            "md",
            "0 4px 6px -1px rgb(0 0 0 / 0.1), 0 2px 4px -2px rgb(0 0 0 / 0.1)",
        ),
        (
            "lg",
            "0 10px 15px -3px rgb(0 0 0 / 0.1), 0 4px 6px -4px rgb(0 0 0 / 0.1)",
        ),
        (
            "xl",
            "0 20px 25px -5px rgb(0 0 0 / 0.1), 0 8px 10px -6px rgb(0 0 0 / 0.1)",
        ),
        ("2xl", "0 25px 50px -12px rgb(0 0 0 / 0.25)"),
        ("inner", "inset 0 2px 4px 0 rgb(0 0 0 / 0.05)"),
        ("none", "none"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn ring_width_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("DEFAULT", "3px"),
        ("0", "0px"),
        ("1", "1px"),
        ("2", "2px"),
        ("4", "4px"),
        ("8", "8px"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

/// `theme.gradientColorStopPositions` — `0%` through `100%` in 5%
/// increments. Mirrors `stubs/config.full.js`.
fn gradient_position_table() -> Value {
    let mut obj = Map::new();
    for n in (0..=100).step_by(5) {
        let key = format!("{n}%");
        let value = format!("{n}%");
        insert_string(&mut obj, &key, &value);
    }
    Value::Object(obj)
}

fn ring_offset_width_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("0", "0px"),
        ("1", "1px"),
        ("2", "2px"),
        ("4", "4px"),
        ("8", "8px"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn inset_table() -> Value {
    // Tailwind v3 `theme.inset` only includes quarter fractions
    // (1/2, 1/3, 2/3, 1/4, 2/4, 3/4) — NOT the /5 and /6 ramps that
    // `width`/`height`/`size`/`basis` get. Mirrors
    // `vendor/tailwindcss-v3/stubs/config.full.js inset:` block.
    // top/right/bottom/left/inset-x/inset-y all resolve through this
    // table (see `value_utilities.rs` UV entries with theme_key="inset").
    let mut obj = match spacing() {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    insert_string(&mut obj, "auto", "auto");
    insert_fractions_through_four(&mut obj);
    insert_string(&mut obj, "full", "100%");
    Value::Object(obj)
}

fn z_index_table() -> Value {
    let mut obj = Map::new();
    insert_string(&mut obj, "auto", "auto");
    for n in [0, 10, 20, 30, 40, 50] {
        insert_string(&mut obj, &n.to_string(), &n.to_string());
    }
    Value::Object(obj)
}

fn cursor_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("auto", "auto"),
        ("default", "default"),
        ("pointer", "pointer"),
        ("wait", "wait"),
        ("text", "text"),
        ("move", "move"),
        ("help", "help"),
        ("not-allowed", "not-allowed"),
        ("none", "none"),
        ("context-menu", "context-menu"),
        ("progress", "progress"),
        ("cell", "cell"),
        ("crosshair", "crosshair"),
        ("vertical-text", "vertical-text"),
        ("alias", "alias"),
        ("copy", "copy"),
        ("no-drop", "no-drop"),
        ("grab", "grab"),
        ("grabbing", "grabbing"),
        ("all-scroll", "all-scroll"),
        ("col-resize", "col-resize"),
        ("row-resize", "row-resize"),
        ("n-resize", "n-resize"),
        ("e-resize", "e-resize"),
        ("s-resize", "s-resize"),
        ("w-resize", "w-resize"),
        ("ne-resize", "ne-resize"),
        ("nw-resize", "nw-resize"),
        ("se-resize", "se-resize"),
        ("sw-resize", "sw-resize"),
        ("ew-resize", "ew-resize"),
        ("ns-resize", "ns-resize"),
        ("nesw-resize", "nesw-resize"),
        ("nwse-resize", "nwse-resize"),
        ("zoom-in", "zoom-in"),
        ("zoom-out", "zoom-out"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn transition_property_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("none", "none"),
        ("all", "all"),
        ("DEFAULT", "color, background-color, border-color, text-decoration-color, fill, stroke, opacity, box-shadow, transform, filter, backdrop-filter"),
        ("colors", "color, background-color, border-color, text-decoration-color, fill, stroke"),
        ("opacity", "opacity"),
        ("shadow", "box-shadow"),
        ("transform", "transform"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn transition_duration_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("DEFAULT", "150ms"),
        ("0", "0s"),
        ("75", "75ms"),
        ("100", "100ms"),
        ("150", "150ms"),
        ("200", "200ms"),
        ("300", "300ms"),
        ("500", "500ms"),
        ("700", "700ms"),
        ("1000", "1000ms"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn transition_timing_function_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("DEFAULT", "cubic-bezier(0.4, 0, 0.2, 1)"),
        ("linear", "linear"),
        ("in", "cubic-bezier(0.4, 0, 1, 1)"),
        ("out", "cubic-bezier(0, 0, 0.2, 1)"),
        ("in-out", "cubic-bezier(0.4, 0, 0.2, 1)"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn transition_delay_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("0", "0s"),
        ("75", "75ms"),
        ("100", "100ms"),
        ("150", "150ms"),
        ("200", "200ms"),
        ("300", "300ms"),
        ("500", "500ms"),
        ("700", "700ms"),
        ("1000", "1000ms"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn animation_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("none", "none"),
        ("spin", "spin 1s linear infinite"),
        ("ping", "ping 1s cubic-bezier(0, 0, 0.2, 1) infinite"),
        ("pulse", "pulse 2s cubic-bezier(0.4, 0, 0.6, 1) infinite"),
        ("bounce", "bounce 1s infinite"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn scroll_margin_table() -> Value {
    let mut obj = match spacing() {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    insert_string(&mut obj, "auto", "auto");
    Value::Object(obj)
}

fn background_image_table() -> Value {
    // Mirrors `stubs/config.full.js` — `none` plus the eight
    // `gradient-to-*` shorthands that compose `--tw-gradient-stops`.
    let entries: &[(&str, &str)] = &[
        ("none", "none"),
        (
            "gradient-to-t",
            "linear-gradient(to top, var(--tw-gradient-stops))",
        ),
        (
            "gradient-to-tr",
            "linear-gradient(to top right, var(--tw-gradient-stops))",
        ),
        (
            "gradient-to-r",
            "linear-gradient(to right, var(--tw-gradient-stops))",
        ),
        (
            "gradient-to-br",
            "linear-gradient(to bottom right, var(--tw-gradient-stops))",
        ),
        (
            "gradient-to-b",
            "linear-gradient(to bottom, var(--tw-gradient-stops))",
        ),
        (
            "gradient-to-bl",
            "linear-gradient(to bottom left, var(--tw-gradient-stops))",
        ),
        (
            "gradient-to-l",
            "linear-gradient(to left, var(--tw-gradient-stops))",
        ),
        (
            "gradient-to-tl",
            "linear-gradient(to top left, var(--tw-gradient-stops))",
        ),
    ];
    let mut t = Map::new();
    for (k, v) in entries {
        t.insert((*k).into(), Value::String((*v).into()));
    }
    Value::Object(t)
}

fn background_position_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("bottom", "bottom"),
        ("center", "center"),
        ("left", "left"),
        ("left-bottom", "left bottom"),
        ("left-top", "left top"),
        ("right", "right"),
        ("right-bottom", "right bottom"),
        ("right-top", "right top"),
        ("top", "top"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn background_size_table() -> Value {
    let entries: &[(&str, &str)] = &[("auto", "auto"), ("cover", "cover"), ("contain", "contain")];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn list_style_type_table() -> Value {
    let entries: &[(&str, &str)] = &[("none", "none"), ("disc", "disc"), ("decimal", "decimal")];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn list_style_image_table() -> Value {
    let mut obj = Map::new();
    insert_string(&mut obj, "none", "none");
    Value::Object(obj)
}

fn blur_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("0", "0"),
        ("none", ""),
        ("sm", "4px"),
        ("DEFAULT", "8px"),
        ("md", "12px"),
        ("lg", "16px"),
        ("xl", "24px"),
        ("2xl", "40px"),
        ("3xl", "64px"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn brightness_table() -> Value {
    let mut obj = Map::new();
    for (k, v) in [
        ("0", "0"),
        ("50", ".5"),
        ("75", ".75"),
        ("90", ".9"),
        ("95", ".95"),
        ("100", "1"),
        ("105", "1.05"),
        ("110", "1.1"),
        ("125", "1.25"),
        ("150", "1.5"),
        ("200", "2"),
    ] {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn contrast_table() -> Value {
    let mut obj = Map::new();
    for (k, v) in [
        ("0", "0"),
        ("50", ".5"),
        ("75", ".75"),
        ("100", "1"),
        ("125", "1.25"),
        ("150", "1.5"),
        ("200", "2"),
    ] {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn grayscale_invert_sepia_table() -> Value {
    let mut obj = Map::new();
    insert_string(&mut obj, "0", "0");
    insert_string(&mut obj, "DEFAULT", "100%");
    Value::Object(obj)
}

fn hue_rotate_table() -> Value {
    let mut obj = Map::new();
    for (k, v) in [
        ("0", "0deg"),
        ("15", "15deg"),
        ("30", "30deg"),
        ("60", "60deg"),
        ("90", "90deg"),
        ("180", "180deg"),
    ] {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn saturate_table() -> Value {
    let mut obj = Map::new();
    for (k, v) in [
        ("0", "0"),
        ("50", ".5"),
        ("100", "1"),
        ("150", "1.5"),
        ("200", "2"),
    ] {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn drop_shadow_table() -> Value {
    let mut obj = Map::new();
    insert_string(&mut obj, "sm", "0 1px 1px rgb(0 0 0 / 0.05)");
    obj.insert(
        "DEFAULT".into(),
        Value::Array(vec![
            Value::String("0 1px 2px rgb(0 0 0 / 0.1)".into()),
            Value::String("0 1px 1px rgb(0 0 0 / 0.06)".into()),
        ]),
    );
    obj.insert(
        "md".into(),
        Value::Array(vec![
            Value::String("0 4px 3px rgb(0 0 0 / 0.07)".into()),
            Value::String("0 2px 2px rgb(0 0 0 / 0.06)".into()),
        ]),
    );
    obj.insert(
        "lg".into(),
        Value::Array(vec![
            Value::String("0 10px 8px rgb(0 0 0 / 0.04)".into()),
            Value::String("0 4px 3px rgb(0 0 0 / 0.1)".into()),
        ]),
    );
    obj.insert(
        "xl".into(),
        Value::Array(vec![
            Value::String("0 20px 13px rgb(0 0 0 / 0.03)".into()),
            Value::String("0 8px 5px rgb(0 0 0 / 0.08)".into()),
        ]),
    );
    insert_string(&mut obj, "2xl", "0 25px 25px rgb(0 0 0 / 0.15)");
    insert_string(&mut obj, "none", "0 0 #0000");
    Value::Object(obj)
}

fn outline_width_table() -> Value {
    let mut obj = Map::new();
    for (k, v) in [
        ("0", "0px"),
        ("1", "1px"),
        ("2", "2px"),
        ("4", "4px"),
        ("8", "8px"),
    ] {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn outline_offset_table() -> Value {
    let mut obj = Map::new();
    for (k, v) in [
        ("0", "0px"),
        ("1", "1px"),
        ("2", "2px"),
        ("4", "4px"),
        ("8", "8px"),
    ] {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn flex_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("1", "1 1 0%"),
        ("auto", "1 1 auto"),
        ("initial", "0 1 auto"),
        ("none", "none"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn flex_basis_table() -> Value {
    // theme.flexBasis = ({ theme }) => ({ auto, ...spacing, fractions, full })
    // Same shape as theme.width for the fraction part.
    let mut obj = match spacing() {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    insert_string(&mut obj, "auto", "auto");
    insert_fractions_through_six(&mut obj);
    insert_twelfths(&mut obj);
    insert_string(&mut obj, "full", "100%");
    Value::Object(obj)
}

fn flex_grow_table() -> Value {
    let mut obj = Map::new();
    insert_string(&mut obj, "0", "0");
    insert_string(&mut obj, "DEFAULT", "1");
    Value::Object(obj)
}

fn flex_shrink_table() -> Value {
    let mut obj = Map::new();
    insert_string(&mut obj, "0", "0");
    insert_string(&mut obj, "DEFAULT", "1");
    Value::Object(obj)
}

fn order_table() -> Value {
    let mut obj = Map::new();
    insert_string(&mut obj, "first", "-9999");
    insert_string(&mut obj, "last", "9999");
    insert_string(&mut obj, "none", "0");
    for n in 1..=12 {
        insert_string(&mut obj, &n.to_string(), &n.to_string());
    }
    Value::Object(obj)
}

fn grid_template_table() -> Value {
    let mut obj = Map::new();
    insert_string(&mut obj, "none", "none");
    insert_string(&mut obj, "subgrid", "subgrid");
    for n in 1..=12 {
        insert_string(
            &mut obj,
            &n.to_string(),
            &format!("repeat({n}, minmax(0, 1fr))"),
        );
    }
    Value::Object(obj)
}

fn grid_column_table() -> Value {
    let mut obj = Map::new();
    insert_string(&mut obj, "auto", "auto");
    for n in 1..=12 {
        insert_string(
            &mut obj,
            &format!("span-{n}"),
            &format!("span {n} / span {n}"),
        );
    }
    insert_string(&mut obj, "span-full", "1 / -1");
    Value::Object(obj)
}

fn grid_start_end_table() -> Value {
    let mut obj = Map::new();
    insert_string(&mut obj, "auto", "auto");
    for n in 1..=13 {
        insert_string(&mut obj, &n.to_string(), &n.to_string());
    }
    Value::Object(obj)
}

fn grid_auto_table() -> Value {
    let mut obj = Map::new();
    insert_string(&mut obj, "auto", "auto");
    insert_string(&mut obj, "min", "min-content");
    insert_string(&mut obj, "max", "max-content");
    insert_string(&mut obj, "fr", "minmax(0, 1fr)");
    Value::Object(obj)
}

fn border_width_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("DEFAULT", "1px"),
        ("0", "0px"),
        ("2", "2px"),
        ("4", "4px"),
        ("8", "8px"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn border_radius_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("none", "0px"),
        ("sm", "0.125rem"),
        ("DEFAULT", "0.25rem"),
        ("md", "0.375rem"),
        ("lg", "0.5rem"),
        ("xl", "0.75rem"),
        ("2xl", "1rem"),
        ("3xl", "1.5rem"),
        ("full", "9999px"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

/// `theme.translate = ({theme}) => ({ ...spacing, '1/2'..'3/4', full })`.
fn translate_table() -> Value {
    let mut obj = match spacing() {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    let entries: &[(&str, &str)] = &[
        ("1/2", "50%"),
        ("1/3", "33.333333%"),
        ("2/3", "66.666667%"),
        ("1/4", "25%"),
        ("2/4", "50%"),
        ("3/4", "75%"),
        ("full", "100%"),
    ];
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn rotate_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("0", "0deg"),
        ("1", "1deg"),
        ("2", "2deg"),
        ("3", "3deg"),
        ("6", "6deg"),
        ("12", "12deg"),
        ("45", "45deg"),
        ("90", "90deg"),
        ("180", "180deg"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn scale_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("0", "0"),
        ("50", ".5"),
        ("75", ".75"),
        ("90", ".9"),
        ("95", ".95"),
        ("100", "1"),
        ("105", "1.05"),
        ("110", "1.1"),
        ("125", "1.25"),
        ("150", "1.5"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn skew_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("0", "0deg"),
        ("1", "1deg"),
        ("2", "2deg"),
        ("3", "3deg"),
        ("6", "6deg"),
        ("12", "12deg"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn opacity_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("0", "0"),
        ("5", "0.05"),
        ("10", "0.1"),
        ("15", "0.15"),
        ("20", "0.2"),
        ("25", "0.25"),
        ("30", "0.3"),
        ("35", "0.35"),
        ("40", "0.4"),
        ("45", "0.45"),
        ("50", "0.5"),
        ("55", "0.55"),
        ("60", "0.6"),
        ("65", "0.65"),
        ("70", "0.7"),
        ("75", "0.75"),
        ("80", "0.8"),
        ("85", "0.85"),
        ("90", "0.9"),
        ("95", "0.95"),
        ("100", "1"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

/// Mirror of Tailwind's default `theme.spacing` table from
/// `stubs/config.full.js`. Keys are stringified rationals (`"0.5"` etc.)
/// because Tailwind authors them as JS keys.
fn spacing() -> Value {
    let entries: &[(&str, &str)] = &[
        ("px", "1px"),
        ("0", "0px"),
        ("0.5", "0.125rem"),
        ("1", "0.25rem"),
        ("1.5", "0.375rem"),
        ("2", "0.5rem"),
        ("2.5", "0.625rem"),
        ("3", "0.75rem"),
        ("3.5", "0.875rem"),
        ("4", "1rem"),
        ("5", "1.25rem"),
        ("6", "1.5rem"),
        ("7", "1.75rem"),
        ("8", "2rem"),
        ("9", "2.25rem"),
        ("10", "2.5rem"),
        ("11", "2.75rem"),
        ("12", "3rem"),
        ("14", "3.5rem"),
        ("16", "4rem"),
        ("20", "5rem"),
        ("24", "6rem"),
        ("28", "7rem"),
        ("32", "8rem"),
        ("36", "9rem"),
        ("40", "10rem"),
        ("44", "11rem"),
        ("48", "12rem"),
        ("52", "13rem"),
        ("56", "14rem"),
        ("60", "15rem"),
        ("64", "16rem"),
        ("72", "18rem"),
        ("80", "20rem"),
        ("96", "24rem"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        obj.insert((*k).to_string(), Value::String((*v).to_string()));
    }
    Value::Object(obj)
}

/// `theme.margin = ({ theme }) => ({ auto: 'auto', ...spacing })` per
/// `stubs/config.full.js`.
fn margin_table() -> Value {
    let mut obj = match spacing() {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    obj.insert("auto".into(), Value::String("auto".into()));
    Value::Object(obj)
}

/// `theme.height = ({theme}) => ({ auto, ...spacing, '1/2', '1/3'..'5/6',
/// full, screen, svh, lvh, dvh, min, max, fit })`.
fn height_table() -> Value {
    let mut obj = match spacing() {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    insert_string(&mut obj, "auto", "auto");
    insert_fractions_through_six(&mut obj);
    insert_string(&mut obj, "full", "100%");
    insert_string(&mut obj, "screen", "100vh");
    insert_string(&mut obj, "svh", "100svh");
    insert_string(&mut obj, "lvh", "100lvh");
    insert_string(&mut obj, "dvh", "100dvh");
    insert_string(&mut obj, "min", "min-content");
    insert_string(&mut obj, "max", "max-content");
    insert_string(&mut obj, "fit", "fit-content");
    Value::Object(obj)
}

/// `theme.size = ({theme}) => ({ auto, ...spacing, '1/2'..'5/6', full,
/// min, max, fit })`. (Size doesn't include viewport units.)
fn size_table() -> Value {
    // Tailwind v3 `theme.size = ({theme}) => ({ auto, ...spacing,
    // '1/2'..'5/6', '1/12'..'11/12', full, min, max, fit })`. The
    // /12 ramp is part of `size` just like it is part of `width` —
    // mirrors `vendor/tailwindcss-v3/stubs/config.full.js size:` block
    // at lines 991-1001.
    let mut obj = match spacing() {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    insert_string(&mut obj, "auto", "auto");
    insert_fractions_through_six(&mut obj);
    insert_twelfths(&mut obj);
    insert_string(&mut obj, "full", "100%");
    insert_string(&mut obj, "min", "min-content");
    insert_string(&mut obj, "max", "max-content");
    insert_string(&mut obj, "fit", "fit-content");
    Value::Object(obj)
}

/// `theme.width = ({theme}) => ({ auto, ...spacing, '1/2'..'11/12', full,
/// screen/svw/lvw/dvw, min/max/fit })`. The /12 ramp is unique to width.
fn width_table() -> Value {
    let mut obj = match spacing() {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    insert_string(&mut obj, "auto", "auto");
    insert_fractions_through_six(&mut obj);
    insert_twelfths(&mut obj);
    insert_string(&mut obj, "full", "100%");
    insert_string(&mut obj, "screen", "100vw");
    insert_string(&mut obj, "svw", "100svw");
    insert_string(&mut obj, "lvw", "100lvw");
    insert_string(&mut obj, "dvw", "100dvw");
    insert_string(&mut obj, "min", "min-content");
    insert_string(&mut obj, "max", "max-content");
    insert_string(&mut obj, "fit", "fit-content");
    Value::Object(obj)
}

fn min_width_table() -> Value {
    let mut obj = match spacing() {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    insert_string(&mut obj, "full", "100%");
    insert_string(&mut obj, "min", "min-content");
    insert_string(&mut obj, "max", "max-content");
    insert_string(&mut obj, "fit", "fit-content");
    Value::Object(obj)
}

fn min_height_table() -> Value {
    let mut obj = match spacing() {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    insert_string(&mut obj, "full", "100%");
    insert_string(&mut obj, "screen", "100vh");
    insert_string(&mut obj, "svh", "100svh");
    insert_string(&mut obj, "lvh", "100lvh");
    insert_string(&mut obj, "dvh", "100dvh");
    insert_string(&mut obj, "min", "min-content");
    insert_string(&mut obj, "max", "max-content");
    insert_string(&mut obj, "fit", "fit-content");
    Value::Object(obj)
}

fn max_height_table() -> Value {
    let mut obj = match spacing() {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    insert_string(&mut obj, "none", "none");
    insert_string(&mut obj, "full", "100%");
    insert_string(&mut obj, "screen", "100vh");
    insert_string(&mut obj, "svh", "100svh");
    insert_string(&mut obj, "lvh", "100lvh");
    insert_string(&mut obj, "dvh", "100dvh");
    insert_string(&mut obj, "min", "min-content");
    insert_string(&mut obj, "max", "max-content");
    insert_string(&mut obj, "fit", "fit-content");
    Value::Object(obj)
}

/// `theme.maxWidth = ({theme, breakpoints}) => ({ ...spacing, none, xs..7xl,
/// full, min, max, fit, prose, screen-sm..screen-2xl })`. The `breakpoints`
/// helper expands `screens` into `screen-<name>` keys; we duplicate that
/// mapping here so the default works without needing a second-pass theme
/// resolution.
fn max_width_table() -> Value {
    let mut obj = match spacing() {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    insert_string(&mut obj, "none", "none");
    let scale: &[(&str, &str)] = &[
        ("xs", "20rem"),
        ("sm", "24rem"),
        ("md", "28rem"),
        ("lg", "32rem"),
        ("xl", "36rem"),
        ("2xl", "42rem"),
        ("3xl", "48rem"),
        ("4xl", "56rem"),
        ("5xl", "64rem"),
        ("6xl", "72rem"),
        ("7xl", "80rem"),
    ];
    for (k, v) in scale {
        insert_string(&mut obj, k, v);
    }
    insert_string(&mut obj, "full", "100%");
    insert_string(&mut obj, "min", "min-content");
    insert_string(&mut obj, "max", "max-content");
    insert_string(&mut obj, "fit", "fit-content");
    insert_string(&mut obj, "prose", "65ch");
    // `breakpoints(theme('screens'))` -> screen-sm: 640px, etc.
    for (name, value) in [
        ("sm", "640px"),
        ("md", "768px"),
        ("lg", "1024px"),
        ("xl", "1280px"),
        ("2xl", "1536px"),
    ] {
        insert_string(&mut obj, &format!("screen-{name}"), value);
    }
    Value::Object(obj)
}

fn insert_string(obj: &mut Map<String, Value>, key: &str, value: &str) {
    obj.insert(key.to_string(), Value::String(value.to_string()));
}

/// Quarter-stop fractions only: `1/2`..`3/4`. Used by `theme.inset`
/// in Tailwind v3 (which is sparser than `theme.width`/`height`/etc.).
fn insert_fractions_through_four(obj: &mut Map<String, Value>) {
    let entries: &[(&str, &str)] = &[
        ("1/2", "50%"),
        ("1/3", "33.333333%"),
        ("2/3", "66.666667%"),
        ("1/4", "25%"),
        ("2/4", "50%"),
        ("3/4", "75%"),
    ];
    for (k, v) in entries {
        insert_string(obj, k, v);
    }
}

fn insert_fractions_through_six(obj: &mut Map<String, Value>) {
    let entries: &[(&str, &str)] = &[
        ("1/2", "50%"),
        ("1/3", "33.333333%"),
        ("2/3", "66.666667%"),
        ("1/4", "25%"),
        ("2/4", "50%"),
        ("3/4", "75%"),
        ("1/5", "20%"),
        ("2/5", "40%"),
        ("3/5", "60%"),
        ("4/5", "80%"),
        ("1/6", "16.666667%"),
        ("2/6", "33.333333%"),
        ("3/6", "50%"),
        ("4/6", "66.666667%"),
        ("5/6", "83.333333%"),
    ];
    for (k, v) in entries {
        insert_string(obj, k, v);
    }
}

fn insert_twelfths(obj: &mut Map<String, Value>) {
    let entries: &[(&str, &str)] = &[
        ("1/12", "8.333333%"),
        ("2/12", "16.666667%"),
        ("3/12", "25%"),
        ("4/12", "33.333333%"),
        ("5/12", "41.666667%"),
        ("6/12", "50%"),
        ("7/12", "58.333333%"),
        ("8/12", "66.666667%"),
        ("9/12", "75%"),
        ("10/12", "83.333333%"),
        ("11/12", "91.666667%"),
    ];
    for (k, v) in entries {
        insert_string(obj, k, v);
    }
}

/// `theme.fontFamily` defaults from `stubs/config.full.js`. Values are
/// arrays of font name strings; the FontFamily resolver joins them with
/// `, ` to produce a CSS `font-family` value.
fn font_family_table() -> Value {
    let mut obj = Map::new();
    obj.insert(
        "sans".into(),
        Value::Array(
            [
                "ui-sans-serif",
                "system-ui",
                "sans-serif",
                "\"Apple Color Emoji\"",
                "\"Segoe UI Emoji\"",
                "\"Segoe UI Symbol\"",
                "\"Noto Color Emoji\"",
            ]
            .iter()
            .map(|s| Value::String((*s).into()))
            .collect(),
        ),
    );
    obj.insert(
        "serif".into(),
        Value::Array(
            [
                "ui-serif",
                "Georgia",
                "Cambria",
                "\"Times New Roman\"",
                "Times",
                "serif",
            ]
            .iter()
            .map(|s| Value::String((*s).into()))
            .collect(),
        ),
    );
    obj.insert(
        "mono".into(),
        Value::Array(
            [
                "ui-monospace",
                "SFMono-Regular",
                "Menlo",
                "Monaco",
                "Consolas",
                "\"Liberation Mono\"",
                "\"Courier New\"",
                "monospace",
            ]
            .iter()
            .map(|s| Value::String((*s).into()))
            .collect(),
        ),
    );
    Value::Object(obj)
}

/// `theme.fontSize` defaults — each key maps to a `[size, options]`
/// tuple where `options` is `{ lineHeight: '...' }`. Mirrors
/// `stubs/config.full.js` exactly.
fn font_size_table() -> Value {
    let entries: &[(&str, &str, &str)] = &[
        ("xs", "0.75rem", "1rem"),
        ("sm", "0.875rem", "1.25rem"),
        ("base", "1rem", "1.5rem"),
        ("lg", "1.125rem", "1.75rem"),
        ("xl", "1.25rem", "1.75rem"),
        ("2xl", "1.5rem", "2rem"),
        ("3xl", "1.875rem", "2.25rem"),
        ("4xl", "2.25rem", "2.5rem"),
        ("5xl", "3rem", "1"),
        ("6xl", "3.75rem", "1"),
        ("7xl", "4.5rem", "1"),
        ("8xl", "6rem", "1"),
        ("9xl", "8rem", "1"),
    ];
    let mut obj = Map::new();
    for (k, size, line_height) in entries {
        let mut options = Map::new();
        options.insert(
            "lineHeight".into(),
            Value::String((*line_height).to_string()),
        );
        obj.insert(
            (*k).to_string(),
            Value::Array(vec![
                Value::String((*size).to_string()),
                Value::Object(options),
            ]),
        );
    }
    Value::Object(obj)
}

fn font_weight_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("thin", "100"),
        ("extralight", "200"),
        ("light", "300"),
        ("normal", "400"),
        ("medium", "500"),
        ("semibold", "600"),
        ("bold", "700"),
        ("extrabold", "800"),
        ("black", "900"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn line_height_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("none", "1"),
        ("tight", "1.25"),
        ("snug", "1.375"),
        ("normal", "1.5"),
        ("relaxed", "1.625"),
        ("loose", "2"),
        ("3", ".75rem"),
        ("4", "1rem"),
        ("5", "1.25rem"),
        ("6", "1.5rem"),
        ("7", "1.75rem"),
        ("8", "2rem"),
        ("9", "2.25rem"),
        ("10", "2.5rem"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn letter_spacing_table() -> Value {
    let entries: &[(&str, &str)] = &[
        ("tighter", "-0.05em"),
        ("tight", "-0.025em"),
        ("normal", "0em"),
        ("wide", "0.025em"),
        ("wider", "0.05em"),
        ("widest", "0.1em"),
    ];
    let mut obj = Map::new();
    for (k, v) in entries {
        insert_string(&mut obj, k, v);
    }
    Value::Object(obj)
}

fn screens() -> Map<String, Value> {
    let mut obj = Map::new();
    obj.insert("sm".into(), Value::String("640px".into()));
    obj.insert("md".into(), Value::String("768px".into()));
    obj.insert("lg".into(), Value::String("1024px".into()));
    obj.insert("xl".into(), Value::String("1280px".into()));
    obj.insert("2xl".into(), Value::String("1536px".into()));
    obj
}

/// Look up a dot-path inside an arbitrary `theme` object. Returns the
/// matched value, traversing nested maps. Used by both the JS-resolved
/// config path and the Rust default fallback.
pub fn lookup_theme<'a>(theme: &'a Value, path: &str) -> Option<&'a Value> {
    let mut node = theme;
    for seg in path.split('.') {
        node = node.get(seg)?;
    }
    Some(node)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spacing_scale_has_thirty_five_entries() {
        let s = spacing();
        let obj = s.as_object().unwrap();
        // Verbatim from Tailwind's stub: px + 0..96 with the half-step ramps.
        assert_eq!(obj.len(), 35);
        assert_eq!(obj["4"], Value::String("1rem".into()));
        assert_eq!(obj["px"], Value::String("1px".into()));
        assert_eq!(obj["0.5"], Value::String("0.125rem".into()));
    }

    #[test]
    fn margin_includes_auto_on_top_of_spacing() {
        let m = margin_table();
        let obj = m.as_object().unwrap();
        assert_eq!(obj["auto"], Value::String("auto".into()));
        // Inherits the spacing scale.
        assert_eq!(obj["4"], Value::String("1rem".into()));
    }

    #[test]
    fn lookup_theme_traverses_dot_paths() {
        let t = default_theme();
        assert_eq!(
            lookup_theme(&t, "spacing.4").and_then(Value::as_str),
            Some("1rem")
        );
        assert_eq!(
            lookup_theme(&t, "margin.auto").and_then(Value::as_str),
            Some("auto")
        );
        assert!(lookup_theme(&t, "spacing.bogus").is_none());
    }
}
