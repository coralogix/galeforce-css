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

//! Static (zero-arg) utilities ported from
//! `tailwindcss/src/corePlugins.js` for v3.4.19.
//!
//! "Static" here means the utility has no value and no variant interaction at
//! the property level — `flex` always emits `display: flex`. Utility plugins
//! with theme lookups (`bg-red-500`, `mt-4`) live elsewhere.
//!
//! The exact ordering matters for conflict resolution (a later utility wins
//! when two candidates produce the same property), and Tailwind's order is
//! the order plugins are declared in `corePlugins.js`. We mirror that here
//! so when sorting lands, Galeforce and Tailwind agree.

/// Single static utility: a class name (without the leading `.`) and its
/// declarations as `(property, value)` pairs.
#[derive(Clone, Copy, Debug)]
pub struct StaticUtility {
    pub name: &'static str,
    pub declarations: &'static [(&'static str, &'static str)],
    /// Tailwind plugin name. Used by the `corePlugins` config filter
    /// to selectively disable utility families
    /// (`corePlugins: { display: false }`).
    pub plugin: &'static str,
}

macro_rules! u {
    ($name:literal, $plugin:literal, [ $( ($p:literal, $v:literal) ),* $(,)? ]) => {
        StaticUtility {
            name: $name,
            plugin: $plugin,
            declarations: &[ $( ($p, $v) ),* ],
        }
    };
}

pub static STATIC_UTILITIES: &[StaticUtility] = &[
    // ---- accessibility ----
    u!(
        "sr-only",
        "accessibility",
        [
            ("position", "absolute"),
            ("width", "1px"),
            ("height", "1px"),
            ("padding", "0"),
            ("margin", "-1px"),
            ("overflow", "hidden"),
            ("clip", "rect(0, 0, 0, 0)"),
            ("white-space", "nowrap"),
            ("border-width", "0"),
        ]
    ),
    u!(
        "not-sr-only",
        "accessibility",
        [
            ("position", "static"),
            ("width", "auto"),
            ("height", "auto"),
            ("padding", "0"),
            ("margin", "0"),
            ("overflow", "visible"),
            ("clip", "auto"),
            ("white-space", "normal"),
        ]
    ),
    // ---- pointer-events ----
    u!(
        "pointer-events-none",
        "pointerEvents",
        [("pointer-events", "none")]
    ),
    u!(
        "pointer-events-auto",
        "pointerEvents",
        [("pointer-events", "auto")]
    ),
    // ---- visibility ----
    u!("visible", "visibility", [("visibility", "visible")]),
    u!("invisible", "visibility", [("visibility", "hidden")]),
    u!("collapse", "visibility", [("visibility", "collapse")]),
    // ---- position ----
    u!("static", "position", [("position", "static")]),
    u!("fixed", "position", [("position", "fixed")]),
    u!("absolute", "position", [("position", "absolute")]),
    u!("relative", "position", [("position", "relative")]),
    u!("sticky", "position", [("position", "sticky")]),
    // ---- isolation ----
    u!("isolate", "isolation", [("isolation", "isolate")]),
    u!("isolation-auto", "isolation", [("isolation", "auto")]),
    // ---- float ----
    u!("float-start", "float", [("float", "inline-start")]),
    u!("float-end", "float", [("float", "inline-end")]),
    u!("float-right", "float", [("float", "right")]),
    u!("float-left", "float", [("float", "left")]),
    u!("float-none", "float", [("float", "none")]),
    // ---- clear ----
    u!("clear-start", "clear", [("clear", "inline-start")]),
    u!("clear-end", "clear", [("clear", "inline-end")]),
    u!("clear-left", "clear", [("clear", "left")]),
    u!("clear-right", "clear", [("clear", "right")]),
    u!("clear-both", "clear", [("clear", "both")]),
    u!("clear-none", "clear", [("clear", "none")]),
    // ---- box-sizing ----
    u!("box-border", "boxSizing", [("box-sizing", "border-box")]),
    u!("box-content", "boxSizing", [("box-sizing", "content-box")]),
    // ---- display ----
    u!("block", "display", [("display", "block")]),
    u!("inline-block", "display", [("display", "inline-block")]),
    u!("inline", "display", [("display", "inline")]),
    u!("flex", "display", [("display", "flex")]),
    u!("inline-flex", "display", [("display", "inline-flex")]),
    u!("table", "display", [("display", "table")]),
    u!("inline-table", "display", [("display", "inline-table")]),
    u!("table-caption", "display", [("display", "table-caption")]),
    u!("table-cell", "display", [("display", "table-cell")]),
    u!("table-column", "display", [("display", "table-column")]),
    u!(
        "table-column-group",
        "display",
        [("display", "table-column-group")]
    ),
    u!(
        "table-footer-group",
        "display",
        [("display", "table-footer-group")]
    ),
    u!(
        "table-header-group",
        "display",
        [("display", "table-header-group")]
    ),
    u!(
        "table-row-group",
        "display",
        [("display", "table-row-group")]
    ),
    u!("table-row", "display", [("display", "table-row")]),
    u!("flow-root", "display", [("display", "flow-root")]),
    // ---- table-layout / caption-side / border-collapse ----
    u!("table-auto", "tableLayout", [("table-layout", "auto")]),
    u!("table-fixed", "tableLayout", [("table-layout", "fixed")]),
    u!("caption-top", "captionSide", [("caption-side", "top")]),
    u!("caption-bottom", "captionSide", [("caption-side", "bottom")]),
    u!(
        "border-collapse",
        "borderCollapse",
        [("border-collapse", "collapse")]
    ),
    u!(
        "border-separate",
        "borderCollapse",
        [("border-collapse", "separate")]
    ),
    // ---- break-before / break-inside / break-after (CSS Fragmentation) ----
    u!("break-before-auto", "breakBefore", [("break-before", "auto")]),
    u!("break-before-avoid", "breakBefore", [("break-before", "avoid")]),
    u!("break-before-all", "breakBefore", [("break-before", "all")]),
    u!(
        "break-before-avoid-page",
        "breakBefore",
        [("break-before", "avoid-page")]
    ),
    u!("break-before-page", "breakBefore", [("break-before", "page")]),
    u!("break-before-left", "breakBefore", [("break-before", "left")]),
    u!("break-before-right", "breakBefore", [("break-before", "right")]),
    u!(
        "break-before-column",
        "breakBefore",
        [("break-before", "column")]
    ),
    u!("break-inside-auto", "breakInside", [("break-inside", "auto")]),
    u!("break-inside-avoid", "breakInside", [("break-inside", "avoid")]),
    u!(
        "break-inside-avoid-page",
        "breakInside",
        [("break-inside", "avoid-page")]
    ),
    u!(
        "break-inside-avoid-column",
        "breakInside",
        [("break-inside", "avoid-column")]
    ),
    u!("break-after-auto", "breakAfter", [("break-after", "auto")]),
    u!("break-after-avoid", "breakAfter", [("break-after", "avoid")]),
    u!("break-after-all", "breakAfter", [("break-after", "all")]),
    u!(
        "break-after-avoid-page",
        "breakAfter",
        [("break-after", "avoid-page")]
    ),
    u!("break-after-page", "breakAfter", [("break-after", "page")]),
    u!("break-after-left", "breakAfter", [("break-after", "left")]),
    u!("break-after-right", "breakAfter", [("break-after", "right")]),
    u!(
        "break-after-column",
        "breakAfter",
        [("break-after", "column")]
    ),
    u!("grid", "display", [("display", "grid")]),
    u!("inline-grid", "display", [("display", "inline-grid")]),
    u!("contents", "display", [("display", "contents")]),
    u!("list-item", "display", [("display", "list-item")]),
    u!("hidden", "display", [("display", "none")]),
    // ---- text-align ----
    u!("text-left", "textAlign", [("text-align", "left")]),
    u!("text-center", "textAlign", [("text-align", "center")]),
    u!("text-right", "textAlign", [("text-align", "right")]),
    u!("text-justify", "textAlign", [("text-align", "justify")]),
    u!("text-start", "textAlign", [("text-align", "start")]),
    u!("text-end", "textAlign", [("text-align", "end")]),
    // ---- vertical-align ----
    u!(
        "align-baseline",
        "verticalAlign",
        [("vertical-align", "baseline")]
    ),
    u!("align-top", "verticalAlign", [("vertical-align", "top")]),
    u!(
        "align-middle",
        "verticalAlign",
        [("vertical-align", "middle")]
    ),
    u!(
        "align-bottom",
        "verticalAlign",
        [("vertical-align", "bottom")]
    ),
    u!(
        "align-text-top",
        "verticalAlign",
        [("vertical-align", "text-top")]
    ),
    u!(
        "align-text-bottom",
        "verticalAlign",
        [("vertical-align", "text-bottom")]
    ),
    u!("align-sub", "verticalAlign", [("vertical-align", "sub")]),
    u!(
        "align-super",
        "verticalAlign",
        [("vertical-align", "super")]
    ),
    // ---- text-transform ----
    u!(
        "uppercase",
        "textTransform",
        [("text-transform", "uppercase")]
    ),
    u!(
        "lowercase",
        "textTransform",
        [("text-transform", "lowercase")]
    ),
    u!(
        "capitalize",
        "textTransform",
        [("text-transform", "capitalize")]
    ),
    u!("normal-case", "textTransform", [("text-transform", "none")]),
    // ---- font-style ----
    u!("italic", "fontStyle", [("font-style", "italic")]),
    u!("not-italic", "fontStyle", [("font-style", "normal")]),
    // ---- text-decoration ----
    u!(
        "underline",
        "textDecoration",
        [("text-decoration-line", "underline")]
    ),
    u!(
        "overline",
        "textDecoration",
        [("text-decoration-line", "overline")]
    ),
    u!(
        "line-through",
        "textDecoration",
        [("text-decoration-line", "line-through")]
    ),
    u!(
        "no-underline",
        "textDecoration",
        [("text-decoration-line", "none")]
    ),
    // ---- text-decoration-style ----
    u!(
        "decoration-solid",
        "textDecorationStyle",
        [("text-decoration-style", "solid")]
    ),
    u!(
        "decoration-double",
        "textDecorationStyle",
        [("text-decoration-style", "double")]
    ),
    u!(
        "decoration-dotted",
        "textDecorationStyle",
        [("text-decoration-style", "dotted")]
    ),
    u!(
        "decoration-dashed",
        "textDecorationStyle",
        [("text-decoration-style", "dashed")]
    ),
    u!(
        "decoration-wavy",
        "textDecorationStyle",
        [("text-decoration-style", "wavy")]
    ),
    // ---- text-overflow ----
    // Order matches upstream `textOverflow` corePlugin:
    // `truncate`, then the DEPRECATED `overflow-ellipsis` alias,
    // then the canonical `text-ellipsis` / `text-clip` pair.
    u!(
        "truncate",
        "textOverflow",
        [
            ("overflow", "hidden"),
            ("text-overflow", "ellipsis"),
            ("white-space", "nowrap"),
        ]
    ),
    u!(
        "overflow-ellipsis",
        "textOverflow",
        [("text-overflow", "ellipsis")]
    ),
    u!(
        "text-ellipsis",
        "textOverflow",
        [("text-overflow", "ellipsis")]
    ),
    u!("text-clip", "textOverflow", [("text-overflow", "clip")]),
    // ---- box-decoration-break ----
    // Order mirrors upstream `boxDecorationBreak` corePlugin
    // (deprecated `decoration-*` aliases emit first, then the
    // canonical `box-decoration-*`).
    u!(
        "decoration-slice",
        "boxDecorationBreak",
        [("box-decoration-break", "slice")]
    ),
    u!(
        "decoration-clone",
        "boxDecorationBreak",
        [("box-decoration-break", "clone")]
    ),
    u!(
        "box-decoration-slice",
        "boxDecorationBreak",
        [("box-decoration-break", "slice")]
    ),
    u!(
        "box-decoration-clone",
        "boxDecorationBreak",
        [("box-decoration-break", "clone")]
    ),
    // ---- transform shorthand statics ----
    // The `transform`, `transform-cpu` and `transform-gpu` utilities
    // re-assert the composed `transform` value so any active
    // `--tw-*` vars take effect. `transform-gpu` swaps the leading
    // `translate(...)` for `translate3d(...)` to force the
    // compositor onto the GPU. `transform-none` zeroes out the
    // transform.
    u!(
        "transform",
        "transform",
        [(
            "transform",
            "translate(var(--tw-translate-x), var(--tw-translate-y)) rotate(var(--tw-rotate)) skewX(var(--tw-skew-x)) skewY(var(--tw-skew-y)) scaleX(var(--tw-scale-x)) scaleY(var(--tw-scale-y))"
        )]
    ),
    u!(
        "transform-cpu",
        "transform",
        [(
            "transform",
            "translate(var(--tw-translate-x), var(--tw-translate-y)) rotate(var(--tw-rotate)) skewX(var(--tw-skew-x)) skewY(var(--tw-skew-y)) scaleX(var(--tw-scale-x)) scaleY(var(--tw-scale-y))"
        )]
    ),
    u!(
        "transform-gpu",
        "transform",
        [(
            "transform",
            "translate3d(var(--tw-translate-x), var(--tw-translate-y), 0) rotate(var(--tw-rotate)) skewX(var(--tw-skew-x)) skewY(var(--tw-skew-y)) scaleX(var(--tw-scale-x)) scaleY(var(--tw-scale-y))"
        )]
    ),
    u!("transform-none", "transform", [("transform", "none")]),
    // ---- border-style ----
    u!("border-solid", "borderStyle", [("border-style", "solid")]),
    u!("border-dashed", "borderStyle", [("border-style", "dashed")]),
    u!("border-dotted", "borderStyle", [("border-style", "dotted")]),
    u!("border-double", "borderStyle", [("border-style", "double")]),
    u!("border-hidden", "borderStyle", [("border-style", "hidden")]),
    u!("border-none", "borderStyle", [("border-style", "none")]),
    // ---- flex-direction ----
    u!("flex-row", "flexDirection", [("flex-direction", "row")]),
    u!("flex-row-reverse", "flexDirection", [("flex-direction", "row-reverse")]),
    u!("flex-col", "flexDirection", [("flex-direction", "column")]),
    u!("flex-col-reverse", "flexDirection", [("flex-direction", "column-reverse")]),
    // ---- flex-wrap ----
    u!("flex-wrap", "flexWrap", [("flex-wrap", "wrap")]),
    u!("flex-wrap-reverse", "flexWrap", [("flex-wrap", "wrap-reverse")]),
    u!("flex-nowrap", "flexWrap", [("flex-wrap", "nowrap")]),
    // ---- place-content ----
    u!("place-content-center", "placeContent", [("place-content", "center")]),
    u!("place-content-start", "placeContent", [("place-content", "start")]),
    u!("place-content-end", "placeContent", [("place-content", "end")]),
    u!("place-content-between", "placeContent", [("place-content", "space-between")]),
    u!("place-content-around", "placeContent", [("place-content", "space-around")]),
    u!("place-content-evenly", "placeContent", [("place-content", "space-evenly")]),
    u!("place-content-baseline", "placeContent", [("place-content", "baseline")]),
    u!("place-content-stretch", "placeContent", [("place-content", "stretch")]),
    // ---- place-items ----
    u!("place-items-start", "placeItems", [("place-items", "start")]),
    u!("place-items-end", "placeItems", [("place-items", "end")]),
    u!("place-items-center", "placeItems", [("place-items", "center")]),
    u!("place-items-baseline", "placeItems", [("place-items", "baseline")]),
    u!("place-items-stretch", "placeItems", [("place-items", "stretch")]),
    // ---- align-content ----
    u!("content-normal", "alignContent", [("align-content", "normal")]),
    u!("content-center", "alignContent", [("align-content", "center")]),
    u!("content-start", "alignContent", [("align-content", "flex-start")]),
    u!("content-end", "alignContent", [("align-content", "flex-end")]),
    u!("content-between", "alignContent", [("align-content", "space-between")]),
    u!("content-around", "alignContent", [("align-content", "space-around")]),
    u!("content-evenly", "alignContent", [("align-content", "space-evenly")]),
    u!("content-baseline", "alignContent", [("align-content", "baseline")]),
    u!("content-stretch", "alignContent", [("align-content", "stretch")]),
    // ---- align-items ----
    u!("items-start", "alignItems", [("align-items", "flex-start")]),
    u!("items-end", "alignItems", [("align-items", "flex-end")]),
    u!("items-center", "alignItems", [("align-items", "center")]),
    u!("items-baseline", "alignItems", [("align-items", "baseline")]),
    u!("items-stretch", "alignItems", [("align-items", "stretch")]),
    // ---- justify-content ----
    u!("justify-normal", "justifyContent", [("justify-content", "normal")]),
    u!("justify-start", "justifyContent", [("justify-content", "flex-start")]),
    u!("justify-end", "justifyContent", [("justify-content", "flex-end")]),
    u!("justify-center", "justifyContent", [("justify-content", "center")]),
    u!("justify-between", "justifyContent", [("justify-content", "space-between")]),
    u!("justify-around", "justifyContent", [("justify-content", "space-around")]),
    u!("justify-evenly", "justifyContent", [("justify-content", "space-evenly")]),
    u!("justify-stretch", "justifyContent", [("justify-content", "stretch")]),
    // ---- justify-items ----
    u!("justify-items-start", "justifyItems", [("justify-items", "start")]),
    u!("justify-items-end", "justifyItems", [("justify-items", "end")]),
    u!("justify-items-center", "justifyItems", [("justify-items", "center")]),
    u!("justify-items-stretch", "justifyItems", [("justify-items", "stretch")]),
    // ---- place-self ----
    u!("place-self-auto", "placeSelf", [("place-self", "auto")]),
    u!("place-self-start", "placeSelf", [("place-self", "start")]),
    u!("place-self-end", "placeSelf", [("place-self", "end")]),
    u!("place-self-center", "placeSelf", [("place-self", "center")]),
    u!("place-self-stretch", "placeSelf", [("place-self", "stretch")]),
    // ---- align-self ----
    u!("self-auto", "alignSelf", [("align-self", "auto")]),
    u!("self-start", "alignSelf", [("align-self", "flex-start")]),
    u!("self-end", "alignSelf", [("align-self", "flex-end")]),
    u!("self-center", "alignSelf", [("align-self", "center")]),
    u!("self-stretch", "alignSelf", [("align-self", "stretch")]),
    u!("self-baseline", "alignSelf", [("align-self", "baseline")]),
    // ---- justify-self ----
    u!("justify-self-auto", "justifySelf", [("justify-self", "auto")]),
    u!("justify-self-start", "justifySelf", [("justify-self", "start")]),
    u!("justify-self-end", "justifySelf", [("justify-self", "end")]),
    u!("justify-self-center", "justifySelf", [("justify-self", "center")]),
    u!("justify-self-stretch", "justifySelf", [("justify-self", "stretch")]),
    // ---- grid-auto-flow ----
    u!("grid-flow-row", "gridAutoFlow", [("grid-auto-flow", "row")]),
    u!("grid-flow-col", "gridAutoFlow", [("grid-auto-flow", "column")]),
    u!("grid-flow-dense", "gridAutoFlow", [("grid-auto-flow", "dense")]),
    u!("grid-flow-row-dense", "gridAutoFlow", [("grid-auto-flow", "row dense")]),
    u!("grid-flow-col-dense", "gridAutoFlow", [("grid-auto-flow", "column dense")]),
    // ---- filter / backdrop-filter shorthand statics ----
    u!(
        "filter",
        "filter",
        [(
            "filter",
            "var(--tw-blur) var(--tw-brightness) var(--tw-contrast) var(--tw-grayscale) var(--tw-hue-rotate) var(--tw-invert) var(--tw-saturate) var(--tw-sepia) var(--tw-drop-shadow)"
        )]
    ),
    u!("filter-none", "filter", [("filter", "none")]),
    u!(
        "backdrop-filter",
        "backdropFilter",
        [
            (
                "-webkit-backdrop-filter",
                "var(--tw-backdrop-blur) var(--tw-backdrop-brightness) var(--tw-backdrop-contrast) var(--tw-backdrop-grayscale) var(--tw-backdrop-hue-rotate) var(--tw-backdrop-invert) var(--tw-backdrop-opacity) var(--tw-backdrop-saturate) var(--tw-backdrop-sepia)"
            ),
            (
                "backdrop-filter",
                "var(--tw-backdrop-blur) var(--tw-backdrop-brightness) var(--tw-backdrop-contrast) var(--tw-backdrop-grayscale) var(--tw-backdrop-hue-rotate) var(--tw-backdrop-invert) var(--tw-backdrop-opacity) var(--tw-backdrop-saturate) var(--tw-backdrop-sepia)"
            )
        ]
    ),
    u!(
        "backdrop-filter-none",
        "backdropFilter",
        [("-webkit-backdrop-filter", "none"), ("backdrop-filter", "none")]
    ),
    // ---- outline-style ----
    // Note: `outline-none` is two declarations, not just outline-style.
    u!(
        "outline-none",
        "outlineStyle",
        [("outline", "2px solid transparent"), ("outline-offset", "2px")]
    ),
    u!("outline", "outlineStyle", [("outline-style", "solid")]),
    u!("outline-dashed", "outlineStyle", [("outline-style", "dashed")]),
    u!("outline-dotted", "outlineStyle", [("outline-style", "dotted")]),
    u!("outline-double", "outlineStyle", [("outline-style", "double")]),
    // ---- overflow ----
    u!("overflow-auto", "overflow", [("overflow", "auto")]),
    u!("overflow-hidden", "overflow", [("overflow", "hidden")]),
    u!("overflow-clip", "overflow", [("overflow", "clip")]),
    u!("overflow-visible", "overflow", [("overflow", "visible")]),
    u!("overflow-scroll", "overflow", [("overflow", "scroll")]),
    u!("overflow-x-auto", "overflow", [("overflow-x", "auto")]),
    u!("overflow-y-auto", "overflow", [("overflow-y", "auto")]),
    u!("overflow-x-hidden", "overflow", [("overflow-x", "hidden")]),
    u!("overflow-y-hidden", "overflow", [("overflow-y", "hidden")]),
    u!("overflow-x-clip", "overflow", [("overflow-x", "clip")]),
    u!("overflow-y-clip", "overflow", [("overflow-y", "clip")]),
    u!("overflow-x-visible", "overflow", [("overflow-x", "visible")]),
    u!("overflow-y-visible", "overflow", [("overflow-y", "visible")]),
    u!("overflow-x-scroll", "overflow", [("overflow-x", "scroll")]),
    u!("overflow-y-scroll", "overflow", [("overflow-y", "scroll")]),
    // ---- overscroll ----
    u!("overscroll-auto", "overscrollBehavior", [("overscroll-behavior", "auto")]),
    u!("overscroll-contain", "overscrollBehavior", [("overscroll-behavior", "contain")]),
    u!("overscroll-none", "overscrollBehavior", [("overscroll-behavior", "none")]),
    u!("overscroll-y-auto", "overscrollBehavior", [("overscroll-behavior-y", "auto")]),
    u!("overscroll-y-contain", "overscrollBehavior", [("overscroll-behavior-y", "contain")]),
    u!("overscroll-y-none", "overscrollBehavior", [("overscroll-behavior-y", "none")]),
    u!("overscroll-x-auto", "overscrollBehavior", [("overscroll-behavior-x", "auto")]),
    u!("overscroll-x-contain", "overscrollBehavior", [("overscroll-behavior-x", "contain")]),
    u!("overscroll-x-none", "overscrollBehavior", [("overscroll-behavior-x", "none")]),
    // ---- scroll-behavior ----
    u!("scroll-auto", "scrollBehavior", [("scroll-behavior", "auto")]),
    u!("scroll-smooth", "scrollBehavior", [("scroll-behavior", "smooth")]),
    // ---- whitespace ----
    u!("whitespace-normal", "whitespace", [("white-space", "normal")]),
    u!("whitespace-nowrap", "whitespace", [("white-space", "nowrap")]),
    u!("whitespace-pre", "whitespace", [("white-space", "pre")]),
    u!("whitespace-pre-line", "whitespace", [("white-space", "pre-line")]),
    u!("whitespace-pre-wrap", "whitespace", [("white-space", "pre-wrap")]),
    u!("whitespace-break-spaces", "whitespace", [("white-space", "break-spaces")]),
    // ---- text-wrap ----
    u!("text-wrap", "textWrap", [("text-wrap", "wrap")]),
    u!("text-nowrap", "textWrap", [("text-wrap", "nowrap")]),
    u!("text-balance", "textWrap", [("text-wrap", "balance")]),
    u!("text-pretty", "textWrap", [("text-wrap", "pretty")]),
    // ---- word-break ----
    u!("break-normal", "wordBreak", [("overflow-wrap", "normal"), ("word-break", "normal")]),
    u!("break-words", "wordBreak", [("overflow-wrap", "break-word")]),
    u!("break-all", "wordBreak", [("word-break", "break-all")]),
    u!("break-keep", "wordBreak", [("word-break", "keep-all")]),
    // ---- hyphens ----
    // Note: upstream `corePlugins.js` emits ONLY `hyphens` (no
    // -webkit-hyphens). The vendor prefix is autoprefixer territory.
    u!("hyphens-none", "hyphens", [("hyphens", "none")]),
    u!("hyphens-manual", "hyphens", [("hyphens", "manual")]),
    u!("hyphens-auto", "hyphens", [("hyphens", "auto")]),
    // ---- user-select ----
    u!("select-none", "userSelect", [("user-select", "none")]),
    u!("select-text", "userSelect", [("user-select", "text")]),
    u!("select-all", "userSelect", [("user-select", "all")]),
    u!("select-auto", "userSelect", [("user-select", "auto")]),
    // ---- resize ----
    u!("resize-none", "resize", [("resize", "none")]),
    u!("resize-y", "resize", [("resize", "vertical")]),
    u!("resize-x", "resize", [("resize", "horizontal")]),
    u!("resize", "resize", [("resize", "both")]),
    // ---- appearance ----
    u!("appearance-none", "appearance", [("appearance", "none")]),
    u!("appearance-auto", "appearance", [("appearance", "auto")]),
    // ---- background-attachment ----
    u!("bg-fixed", "backgroundAttachment", [("background-attachment", "fixed")]),
    u!("bg-local", "backgroundAttachment", [("background-attachment", "local")]),
    u!("bg-scroll", "backgroundAttachment", [("background-attachment", "scroll")]),
    // ---- background-clip ----
    u!("bg-clip-border", "backgroundClip", [("background-clip", "border-box")]),
    u!("bg-clip-padding", "backgroundClip", [("background-clip", "padding-box")]),
    u!("bg-clip-content", "backgroundClip", [("background-clip", "content-box")]),
    u!("bg-clip-text", "backgroundClip", [("background-clip", "text")]),
    // ---- background-origin ----
    u!("bg-origin-border", "backgroundOrigin", [("background-origin", "border-box")]),
    u!("bg-origin-padding", "backgroundOrigin", [("background-origin", "padding-box")]),
    u!("bg-origin-content", "backgroundOrigin", [("background-origin", "content-box")]),
    // ---- background-repeat ----
    u!("bg-repeat", "backgroundRepeat", [("background-repeat", "repeat")]),
    u!("bg-no-repeat", "backgroundRepeat", [("background-repeat", "no-repeat")]),
    u!("bg-repeat-x", "backgroundRepeat", [("background-repeat", "repeat-x")]),
    u!("bg-repeat-y", "backgroundRepeat", [("background-repeat", "repeat-y")]),
    u!("bg-repeat-round", "backgroundRepeat", [("background-repeat", "round")]),
    u!("bg-repeat-space", "backgroundRepeat", [("background-repeat", "space")]),
    // ---- list-style-position ----
    u!("list-inside", "listStylePosition", [("list-style-position", "inside")]),
    u!("list-outside", "listStylePosition", [("list-style-position", "outside")]),
    // ---- object-fit ----
    u!("object-contain", "objectFit", [("object-fit", "contain")]),
    u!("object-cover", "objectFit", [("object-fit", "cover")]),
    u!("object-fill", "objectFit", [("object-fit", "fill")]),
    u!("object-none", "objectFit", [("object-fit", "none")]),
    u!("object-scale-down", "objectFit", [("object-fit", "scale-down")]),
    // ---- object-position ----
    u!("object-bottom", "objectPosition", [("object-position", "bottom")]),
    u!("object-center", "objectPosition", [("object-position", "center")]),
    u!("object-left", "objectPosition", [("object-position", "left")]),
    u!("object-left-bottom", "objectPosition", [("object-position", "left bottom")]),
    u!("object-left-top", "objectPosition", [("object-position", "left top")]),
    u!("object-right", "objectPosition", [("object-position", "right")]),
    u!("object-right-bottom", "objectPosition", [("object-position", "right bottom")]),
    u!("object-right-top", "objectPosition", [("object-position", "right top")]),
    u!("object-top", "objectPosition", [("object-position", "top")]),
    // ---- mix-blend-mode ----
    u!("mix-blend-normal", "mixBlendMode", [("mix-blend-mode", "normal")]),
    u!("mix-blend-multiply", "mixBlendMode", [("mix-blend-mode", "multiply")]),
    u!("mix-blend-screen", "mixBlendMode", [("mix-blend-mode", "screen")]),
    u!("mix-blend-overlay", "mixBlendMode", [("mix-blend-mode", "overlay")]),
    u!("mix-blend-darken", "mixBlendMode", [("mix-blend-mode", "darken")]),
    u!("mix-blend-lighten", "mixBlendMode", [("mix-blend-mode", "lighten")]),
    u!("mix-blend-color-dodge", "mixBlendMode", [("mix-blend-mode", "color-dodge")]),
    u!("mix-blend-color-burn", "mixBlendMode", [("mix-blend-mode", "color-burn")]),
    u!("mix-blend-hard-light", "mixBlendMode", [("mix-blend-mode", "hard-light")]),
    u!("mix-blend-soft-light", "mixBlendMode", [("mix-blend-mode", "soft-light")]),
    u!("mix-blend-difference", "mixBlendMode", [("mix-blend-mode", "difference")]),
    u!("mix-blend-exclusion", "mixBlendMode", [("mix-blend-mode", "exclusion")]),
    u!("mix-blend-hue", "mixBlendMode", [("mix-blend-mode", "hue")]),
    u!("mix-blend-saturation", "mixBlendMode", [("mix-blend-mode", "saturation")]),
    u!("mix-blend-color", "mixBlendMode", [("mix-blend-mode", "color")]),
    u!("mix-blend-luminosity", "mixBlendMode", [("mix-blend-mode", "luminosity")]),
    u!("mix-blend-plus-darker", "mixBlendMode", [("mix-blend-mode", "plus-darker")]),
    u!("mix-blend-plus-lighter", "mixBlendMode", [("mix-blend-mode", "plus-lighter")]),
    // ---- isolation already covered ----
    // ---- text-decoration-thickness static fallbacks ----
    u!("decoration-auto", "textDecorationThickness", [("text-decoration-thickness", "auto")]),
    u!("decoration-from-font", "textDecorationThickness", [("text-decoration-thickness", "from-font")]),
    // ---- ring inset ----
    u!("ring-inset", "ringWidth", [("--tw-ring-inset", "inset")]),
    // ---- gradient direction (backgroundImage + bg-gradient-to-*) ----
    // Order matches Tailwind v3's alphabetical emission for the
    // `backgroundImage` `matchUtilities` plugin (theme order would
    // be t, tr, r, br, b, bl, l, tl, none — but matchUtilities
    // sorts alphabetically by class name in the output).
    u!(
        "bg-gradient-to-b",
        "backgroundImage",
        [(
            "background-image",
            "linear-gradient(to bottom, var(--tw-gradient-stops))"
        )]
    ),
    u!(
        "bg-gradient-to-bl",
        "backgroundImage",
        [(
            "background-image",
            "linear-gradient(to bottom left, var(--tw-gradient-stops))"
        )]
    ),
    u!(
        "bg-gradient-to-br",
        "backgroundImage",
        [(
            "background-image",
            "linear-gradient(to bottom right, var(--tw-gradient-stops))"
        )]
    ),
    u!(
        "bg-gradient-to-l",
        "backgroundImage",
        [(
            "background-image",
            "linear-gradient(to left, var(--tw-gradient-stops))"
        )]
    ),
    u!(
        "bg-gradient-to-r",
        "backgroundImage",
        [(
            "background-image",
            "linear-gradient(to right, var(--tw-gradient-stops))"
        )]
    ),
    u!(
        "bg-gradient-to-t",
        "backgroundImage",
        [(
            "background-image",
            "linear-gradient(to top, var(--tw-gradient-stops))"
        )]
    ),
    u!(
        "bg-gradient-to-tl",
        "backgroundImage",
        [(
            "background-image",
            "linear-gradient(to top left, var(--tw-gradient-stops))"
        )]
    ),
    u!(
        "bg-gradient-to-tr",
        "backgroundImage",
        [(
            "background-image",
            "linear-gradient(to top right, var(--tw-gradient-stops))"
        )]
    ),
    u!("bg-none", "backgroundImage", [("background-image", "none")]),
    // ---- font-smoothing ----
    u!(
        "antialiased",
        "fontSmoothing",
        [
            ("-webkit-font-smoothing", "antialiased"),
            ("-moz-osx-font-smoothing", "grayscale"),
        ]
    ),
    u!(
        "subpixel-antialiased",
        "fontSmoothing",
        [
            ("-webkit-font-smoothing", "auto"),
            ("-moz-osx-font-smoothing", "auto"),
        ]
    ),
    // ---- font-variant-numeric ----
    // `normal-nums` is the unambiguous reset; the named utilities each
    // SET one of the cascade vars and emit the same composed shorthand
    // so multiple `*-nums` stack on a single element. Vendored verbatim
    // from upstream's `fontVariantNumeric` plugin.
    u!(
        "normal-nums",
        "fontVariantNumeric",
        [("font-variant-numeric", "normal")]
    ),
    u!(
        "ordinal",
        "fontVariantNumeric",
        [
            ("--tw-ordinal", "ordinal"),
            (
                "font-variant-numeric",
                "var(--tw-ordinal) var(--tw-slashed-zero) var(--tw-numeric-figure) var(--tw-numeric-spacing) var(--tw-numeric-fraction)"
            ),
        ]
    ),
    u!(
        "slashed-zero",
        "fontVariantNumeric",
        [
            ("--tw-slashed-zero", "slashed-zero"),
            (
                "font-variant-numeric",
                "var(--tw-ordinal) var(--tw-slashed-zero) var(--tw-numeric-figure) var(--tw-numeric-spacing) var(--tw-numeric-fraction)"
            ),
        ]
    ),
    u!(
        "lining-nums",
        "fontVariantNumeric",
        [
            ("--tw-numeric-figure", "lining-nums"),
            (
                "font-variant-numeric",
                "var(--tw-ordinal) var(--tw-slashed-zero) var(--tw-numeric-figure) var(--tw-numeric-spacing) var(--tw-numeric-fraction)"
            ),
        ]
    ),
    u!(
        "oldstyle-nums",
        "fontVariantNumeric",
        [
            ("--tw-numeric-figure", "oldstyle-nums"),
            (
                "font-variant-numeric",
                "var(--tw-ordinal) var(--tw-slashed-zero) var(--tw-numeric-figure) var(--tw-numeric-spacing) var(--tw-numeric-fraction)"
            ),
        ]
    ),
    u!(
        "proportional-nums",
        "fontVariantNumeric",
        [
            ("--tw-numeric-spacing", "proportional-nums"),
            (
                "font-variant-numeric",
                "var(--tw-ordinal) var(--tw-slashed-zero) var(--tw-numeric-figure) var(--tw-numeric-spacing) var(--tw-numeric-fraction)"
            ),
        ]
    ),
    u!(
        "tabular-nums",
        "fontVariantNumeric",
        [
            ("--tw-numeric-spacing", "tabular-nums"),
            (
                "font-variant-numeric",
                "var(--tw-ordinal) var(--tw-slashed-zero) var(--tw-numeric-figure) var(--tw-numeric-spacing) var(--tw-numeric-fraction)"
            ),
        ]
    ),
    u!(
        "diagonal-fractions",
        "fontVariantNumeric",
        [
            ("--tw-numeric-fraction", "diagonal-fractions"),
            (
                "font-variant-numeric",
                "var(--tw-ordinal) var(--tw-slashed-zero) var(--tw-numeric-figure) var(--tw-numeric-spacing) var(--tw-numeric-fraction)"
            ),
        ]
    ),
    u!(
        "stacked-fractions",
        "fontVariantNumeric",
        [
            ("--tw-numeric-fraction", "stacked-fractions"),
            (
                "font-variant-numeric",
                "var(--tw-ordinal) var(--tw-slashed-zero) var(--tw-numeric-figure) var(--tw-numeric-spacing) var(--tw-numeric-fraction)"
            ),
        ]
    ),
    // ---- line-clamp ----
    // The numeric scale is value-bearing (`line-clamp-1` ... `line-clamp-6`)
    // — handled by the `lineClamp` value-utility resolver. The static
    // entry below is the `none` reset.
    u!(
        "line-clamp-none",
        "lineClamp",
        [
            ("overflow", "visible"),
            ("display", "block"),
            ("-webkit-box-orient", "horizontal"),
            ("-webkit-line-clamp", "none"),
        ]
    ),
    // ---- SVG fill / stroke keywords ----
    // `fill-none` and `stroke-none` are NOT static — they resolve
    // through `theme.fill` / `theme.stroke` which include
    // `none: 'none'` per Tailwind v3 (`config.full.js:253-256` and
    // `:878-881`). Having them as static utilities here gave them a
    // different `within_plugin_order` than the rest of the fill/stroke
    // family, scrambling cascade order whenever both static and
    // value-utility candidates for the same plugin appeared in one
    // bundle. `current` likewise lands via the color resolver because
    // it's a CSS keyword.
    // ---- content (reset) ----
    // Static `content-none` mirrors upstream's content plugin: sets
    // both the cascade var and the property explicitly to `none` so
    // `before:content-none` collapses any inherited content.
    u!(
        "content-none",
        "content",
        [("--tw-content", "none"), ("content", "var(--tw-content)")]
    ),
    // ---- aspect-ratio ----
    u!("aspect-auto", "aspectRatio", [("aspect-ratio", "auto")]),
    u!("aspect-square", "aspectRatio", [("aspect-ratio", "1 / 1")]),
    u!("aspect-video", "aspectRatio", [("aspect-ratio", "16 / 9")]),
    // Upstream registers both `grow` and `flex-grow` as aliases for the
    // same plugin (in two separate matchUtilities groups). Numeric
    // forms (`grow-0`, `flex-grow-0`) are handled via value
    // utilities; the bare aliases ARE value utilities too (resolving
    // to `theme.flexGrow.DEFAULT = 1` / `theme.flexShrink.DEFAULT = 1`)
    // so we don't register them as statics — that would split them
    // across two within-plugin-order spaces and break the cascade
    // ordering of `class="shrink flex-shrink"`.
    //
    // Old aliases (removed):
    //   u!("flex-grow", "flexGrow", [("flex-grow", "1")]),
    //   u!("flex-shrink", "flexShrink", [("flex-shrink", "1")]),
    // ---- flex-grow / flex-shrink legacy aliases (now value-driven) ----
    // (intentionally empty — value-utility table carries these.)
    // Upstream registers both `grow` and `flex-grow` as aliases for the
    // same plugin; same for `shrink` / `flex-shrink`. Static handling
    // is fine since the numeric forms (`grow-0`, `flex-grow-0`) are
    // value-bearing and live in the value-utility table.
    // ---- transform-origin (named keywords) ----
    // `origin-<keyword>` — matches the `transformOrigin` plugin's named
    // theme keys. The arbitrary form `origin-[65%_0%]` flows through
    // the value-utility entry below.
    // `transformOrigin` is a `matchUtilities` plugin — output is
    // alphabetical by class name (theme order would be
    // center, top, top-right, right, ...).
    u!(
        "origin-bottom",
        "transformOrigin",
        [("transform-origin", "bottom")]
    ),
    u!(
        "origin-bottom-left",
        "transformOrigin",
        [("transform-origin", "bottom left")]
    ),
    u!(
        "origin-bottom-right",
        "transformOrigin",
        [("transform-origin", "bottom right")]
    ),
    u!(
        "origin-center",
        "transformOrigin",
        [("transform-origin", "center")]
    ),
    u!(
        "origin-left",
        "transformOrigin",
        [("transform-origin", "left")]
    ),
    u!(
        "origin-right",
        "transformOrigin",
        [("transform-origin", "right")]
    ),
    u!(
        "origin-top",
        "transformOrigin",
        [("transform-origin", "top")]
    ),
    u!(
        "origin-top-left",
        "transformOrigin",
        [("transform-origin", "top left")]
    ),
    u!(
        "origin-top-right",
        "transformOrigin",
        [("transform-origin", "top right")]
    ),
    // ---- touch-action ----
    // The named modes (`touch-auto`/`touch-none`/`touch-manipulation`)
    // emit a single property; the directional modes (`touch-pan-x`,
    // `touch-pan-y`, `touch-pinch-zoom`) write a `--tw-*` cascade var
    // and emit the composed shorthand referencing all three vars.
    // Mirrors upstream's `touchAction` plugin.
    u!("touch-auto", "touchAction", [("touch-action", "auto")]),
    u!("touch-none", "touchAction", [("touch-action", "none")]),
    u!(
        "touch-pan-x",
        "touchAction",
        [
            ("--tw-pan-x", "pan-x"),
            (
                "touch-action",
                "var(--tw-pan-x) var(--tw-pan-y) var(--tw-pinch-zoom)"
            ),
        ]
    ),
    u!(
        "touch-pan-left",
        "touchAction",
        [
            ("--tw-pan-x", "pan-left"),
            (
                "touch-action",
                "var(--tw-pan-x) var(--tw-pan-y) var(--tw-pinch-zoom)"
            ),
        ]
    ),
    u!(
        "touch-pan-right",
        "touchAction",
        [
            ("--tw-pan-x", "pan-right"),
            (
                "touch-action",
                "var(--tw-pan-x) var(--tw-pan-y) var(--tw-pinch-zoom)"
            ),
        ]
    ),
    u!(
        "touch-pan-y",
        "touchAction",
        [
            ("--tw-pan-y", "pan-y"),
            (
                "touch-action",
                "var(--tw-pan-x) var(--tw-pan-y) var(--tw-pinch-zoom)"
            ),
        ]
    ),
    u!(
        "touch-pan-up",
        "touchAction",
        [
            ("--tw-pan-y", "pan-up"),
            (
                "touch-action",
                "var(--tw-pan-x) var(--tw-pan-y) var(--tw-pinch-zoom)"
            ),
        ]
    ),
    u!(
        "touch-pan-down",
        "touchAction",
        [
            ("--tw-pan-y", "pan-down"),
            (
                "touch-action",
                "var(--tw-pan-x) var(--tw-pan-y) var(--tw-pinch-zoom)"
            ),
        ]
    ),
    u!(
        "touch-pinch-zoom",
        "touchAction",
        [
            ("--tw-pinch-zoom", "pinch-zoom"),
            (
                "touch-action",
                "var(--tw-pan-x) var(--tw-pan-y) var(--tw-pinch-zoom)"
            ),
        ]
    ),
    u!(
        "touch-manipulation",
        "touchAction",
        [("touch-action", "manipulation")]
    ),
    // ---- will-change ----
    // `willChange` is a `matchUtilities` plugin — output is
    // alphabetical by class name regardless of theme key order.
    u!(
        "will-change-auto",
        "willChange",
        [("will-change", "auto")]
    ),
    u!(
        "will-change-contents",
        "willChange",
        [("will-change", "contents")]
    ),
    u!(
        "will-change-scroll",
        "willChange",
        [("will-change", "scroll-position")]
    ),
    u!(
        "will-change-transform",
        "willChange",
        [("will-change", "transform")]
    ),
    // ---- forced-color-adjust ----
    u!(
        "forced-color-adjust-auto",
        "forcedColorAdjust",
        [("forced-color-adjust", "auto")]
    ),
    u!(
        "forced-color-adjust-none",
        "forcedColorAdjust",
        [("forced-color-adjust", "none")]
    ),
    // ---- scroll-snap-type / strictness / align / stop ----
    u!(
        "snap-none",
        "scrollSnapType",
        [("scroll-snap-type", "none")]
    ),
    u!(
        "snap-x",
        "scrollSnapType",
        [("scroll-snap-type", "x var(--tw-scroll-snap-strictness)")]
    ),
    u!(
        "snap-y",
        "scrollSnapType",
        [("scroll-snap-type", "y var(--tw-scroll-snap-strictness)")]
    ),
    u!(
        "snap-both",
        "scrollSnapType",
        [("scroll-snap-type", "both var(--tw-scroll-snap-strictness)")]
    ),
    u!(
        "snap-mandatory",
        "scrollSnapType",
        [("--tw-scroll-snap-strictness", "mandatory")]
    ),
    u!(
        "snap-proximity",
        "scrollSnapType",
        [("--tw-scroll-snap-strictness", "proximity")]
    ),
    u!(
        "snap-start",
        "scrollSnapAlign",
        [("scroll-snap-align", "start")]
    ),
    u!(
        "snap-end",
        "scrollSnapAlign",
        [("scroll-snap-align", "end")]
    ),
    u!(
        "snap-center",
        "scrollSnapAlign",
        [("scroll-snap-align", "center")]
    ),
    u!(
        "snap-align-none",
        "scrollSnapAlign",
        [("scroll-snap-align", "none")]
    ),
    u!(
        "snap-normal",
        "scrollSnapStop",
        [("scroll-snap-stop", "normal")]
    ),
    u!(
        "snap-always",
        "scrollSnapStop",
        [("scroll-snap-stop", "always")]
    ),
    // ---- background-blend-mode ----
    u!(
        "bg-blend-normal",
        "backgroundBlendMode",
        [("background-blend-mode", "normal")]
    ),
    u!(
        "bg-blend-multiply",
        "backgroundBlendMode",
        [("background-blend-mode", "multiply")]
    ),
    u!(
        "bg-blend-screen",
        "backgroundBlendMode",
        [("background-blend-mode", "screen")]
    ),
    u!(
        "bg-blend-overlay",
        "backgroundBlendMode",
        [("background-blend-mode", "overlay")]
    ),
    u!(
        "bg-blend-darken",
        "backgroundBlendMode",
        [("background-blend-mode", "darken")]
    ),
    u!(
        "bg-blend-lighten",
        "backgroundBlendMode",
        [("background-blend-mode", "lighten")]
    ),
    u!(
        "bg-blend-color-dodge",
        "backgroundBlendMode",
        [("background-blend-mode", "color-dodge")]
    ),
    u!(
        "bg-blend-color-burn",
        "backgroundBlendMode",
        [("background-blend-mode", "color-burn")]
    ),
    u!(
        "bg-blend-hard-light",
        "backgroundBlendMode",
        [("background-blend-mode", "hard-light")]
    ),
    u!(
        "bg-blend-soft-light",
        "backgroundBlendMode",
        [("background-blend-mode", "soft-light")]
    ),
    u!(
        "bg-blend-difference",
        "backgroundBlendMode",
        [("background-blend-mode", "difference")]
    ),
    u!(
        "bg-blend-exclusion",
        "backgroundBlendMode",
        [("background-blend-mode", "exclusion")]
    ),
    u!(
        "bg-blend-hue",
        "backgroundBlendMode",
        [("background-blend-mode", "hue")]
    ),
    u!(
        "bg-blend-saturation",
        "backgroundBlendMode",
        [("background-blend-mode", "saturation")]
    ),
    u!(
        "bg-blend-color",
        "backgroundBlendMode",
        [("background-blend-mode", "color")]
    ),
    u!(
        "bg-blend-luminosity",
        "backgroundBlendMode",
        [("background-blend-mode", "luminosity")]
    ),
    // NOTE: space/divide reverse + divide-style utilities deliberately
    // omitted from STATIC_UTILITIES because they need the
    // sibling-pair selector suffix that only the value-utility
    // pipeline knows how to apply. See `static_utilities_with_suffix`
    // below — the compiler walks both tables.
];

/// Static utilities whose emitted rule needs a sibling-pair selector
/// suffix (`> :not([hidden]) ~ :not([hidden])`). Mirrors upstream's
/// `space-x-reverse`, `space-y-reverse`, `divide-x-reverse`,
/// `divide-y-reverse`, plus the divide-style family.
pub struct SiblingStatic {
    pub name: &'static str,
    pub declarations: &'static [(&'static str, &'static str)],
}

// Order mirrors upstream's `addUtilities` calls in
// `vendor/tailwindcss-v3/src/corePlugins.js` — `space-y-reverse`
// before `space-x-reverse` (lines 1396-1397), `divide-y-reverse`
// before `divide-x-reverse`, then the divide-style group. Within-
// plugin order is derived from this table's index (offset to come
// AFTER value utilities in the same plugin family).
pub static SIBLING_STATIC_UTILITIES: &[SiblingStatic] = &[
    SiblingStatic {
        name: "space-y-reverse",
        declarations: &[("--tw-space-y-reverse", "1")],
    },
    SiblingStatic {
        name: "space-x-reverse",
        declarations: &[("--tw-space-x-reverse", "1")],
    },
    SiblingStatic {
        name: "divide-y-reverse",
        declarations: &[("--tw-divide-y-reverse", "1")],
    },
    SiblingStatic {
        name: "divide-x-reverse",
        declarations: &[("--tw-divide-x-reverse", "1")],
    },
    SiblingStatic {
        name: "divide-solid",
        declarations: &[("border-style", "solid")],
    },
    SiblingStatic {
        name: "divide-dashed",
        declarations: &[("border-style", "dashed")],
    },
    SiblingStatic {
        name: "divide-dotted",
        declarations: &[("border-style", "dotted")],
    },
    SiblingStatic {
        name: "divide-double",
        declarations: &[("border-style", "double")],
    },
    SiblingStatic {
        name: "divide-none",
        declarations: &[("border-style", "none")],
    },
];

/// Position of a sibling-static utility in `SIBLING_STATIC_UTILITIES`.
/// Used as a `within_plugin_order` offset so sibling-statics emit AFTER
/// the corresponding value utilities in the same plugin family
/// (mirroring upstream's `matchUtilities` → `addUtilities` registration
/// sequence inside corePlugins like `spaceBetween` / `divideWidth`).
pub fn sibling_static_table_index(name: &str) -> Option<u32> {
    sibling_static_index_with_position().get(name).copied()
}

fn sibling_static_index_with_position() -> &'static rustc_hash::FxHashMap<&'static str, u32> {
    static IDX: std::sync::OnceLock<rustc_hash::FxHashMap<&'static str, u32>> =
        std::sync::OnceLock::new();
    IDX.get_or_init(|| {
        let mut m: rustc_hash::FxHashMap<&'static str, u32> = rustc_hash::FxHashMap::default();
        for (i, u) in SIBLING_STATIC_UTILITIES.iter().enumerate() {
            m.insert(u.name, i as u32);
        }
        m
    })
}

pub fn find_sibling_static(name: &str) -> Option<&'static SiblingStatic> {
    sibling_static_index().get(name).copied()
}

/// Look up a static utility by its bare name (no bare class form, no
/// variants, no modifier). Hits a `OnceLock<FxHashMap>` indexed once
/// at first call. Profiling showed `find_static` was the per-candidate
/// hot path: every candidate without a value-utility prefix tries it
/// first, including the 1,800+ JS-noise tokens that miss. The linear
/// scan over ~160 entries was 30 µs per build worth of string
/// compares — switching to a hash lookup saves it.
pub fn find_static(name: &str) -> Option<&'static StaticUtility> {
    static_index().get(name).copied()
}

/// Tailwind v3 emits a plugin's utilities in declaration order:
/// `display` registers `block, inline-block, …, hidden` and emits the
/// rules in that exact sequence, so `hidden` always wins the cascade
/// over `flex` / `block` etc. We mirror that by exposing the
/// `STATIC_UTILITIES` index of `name`, which the compiler uses as a
/// secondary sort key inside the same plugin (`plugin_order`).
/// Returns `None` for non-static candidates (value utilities, plugin
/// utilities) — they sort via the existing prefix-hash tiebreaker.
pub fn static_table_index(name: &str) -> Option<u32> {
    static_index_with_position().get(name).copied()
}

fn static_index_with_position() -> &'static rustc_hash::FxHashMap<&'static str, u32> {
    static IDX: std::sync::OnceLock<rustc_hash::FxHashMap<&'static str, u32>> =
        std::sync::OnceLock::new();
    IDX.get_or_init(|| {
        let mut m: rustc_hash::FxHashMap<&'static str, u32> = rustc_hash::FxHashMap::default();
        for (i, u) in STATIC_UTILITIES.iter().enumerate() {
            m.insert(u.name, i as u32);
        }
        m
    })
}

/// Iterate every registered static utility name. Used by the
/// `getClassList()` editor API to populate class-name autocomplete.
pub fn static_names() -> impl Iterator<Item = &'static str> {
    STATIC_UTILITIES.iter().map(|u| u.name)
}

pub fn sibling_static_names() -> impl Iterator<Item = &'static str> {
    SIBLING_STATIC_UTILITIES.iter().map(|u| u.name)
}

fn static_index() -> &'static rustc_hash::FxHashMap<&'static str, &'static StaticUtility> {
    static IDX: std::sync::OnceLock<rustc_hash::FxHashMap<&'static str, &'static StaticUtility>> =
        std::sync::OnceLock::new();
    IDX.get_or_init(|| {
        let mut m: rustc_hash::FxHashMap<&'static str, &'static StaticUtility> =
            rustc_hash::FxHashMap::default();
        for u in STATIC_UTILITIES {
            m.insert(u.name, u);
        }
        m
    })
}

fn sibling_static_index() -> &'static rustc_hash::FxHashMap<&'static str, &'static SiblingStatic> {
    static IDX: std::sync::OnceLock<rustc_hash::FxHashMap<&'static str, &'static SiblingStatic>> =
        std::sync::OnceLock::new();
    IDX.get_or_init(|| {
        let mut m: rustc_hash::FxHashMap<&'static str, &'static SiblingStatic> =
            rustc_hash::FxHashMap::default();
        for u in SIBLING_STATIC_UTILITIES {
            m.insert(u.name, u);
        }
        m
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_duplicate_names() {
        let mut names: Vec<&str> = STATIC_UTILITIES.iter().map(|u| u.name).collect();
        names.sort();
        let len = names.len();
        names.dedup();
        assert_eq!(names.len(), len, "duplicate static utility names detected");
    }

    #[test]
    fn flex_resolves_to_display_flex() {
        let u = find_static("flex").expect("flex registered");
        assert_eq!(u.declarations, &[("display", "flex")]);
    }

    #[test]
    fn hidden_resolves_to_display_none() {
        let u = find_static("hidden").expect("hidden registered");
        assert_eq!(u.declarations, &[("display", "none")]);
    }

    #[test]
    fn sr_only_has_nine_declarations() {
        // Verbatim from Tailwind 3.4.19 corePlugins.js — if this changes,
        // we want the failure to be loud and obvious.
        let u = find_static("sr-only").expect("sr-only registered");
        assert_eq!(u.declarations.len(), 9);
    }

    #[test]
    fn unknown_returns_none() {
        assert!(find_static("definitely-not-a-utility").is_none());
    }
}
