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

//! Lightning CSS integration for minification + source maps + browser
//! targeting. Single entry point: `transform()` parses our emitted
//! CSS string and re-prints it according to the requested options.
//!
//! Lightning CSS is a CSS parser/transformer/minifier in Rust. It owns
//! the "lower, prefix, minify, source-map" pipeline upstream Tailwind
//! delegates to PostCSS plugins. We use it for the same job — keep the
//! semantic compilation in Galeforce, hand the final-stage transforms to
//! Lightning.
//!
//! Failure mode: if Lightning CSS can't parse our output (shouldn't
//! happen — we emit standard CSS), the caller falls back to the
//! unminified Galeforce string and emits a diagnostic. Minify-or-die is
//! not a useful failure mode.

use lightningcss::{
    printer::PrinterOptions,
    stylesheet::{ParserOptions, StyleSheet},
    targets::{Browsers, Targets},
};
use std::str::FromStr;

/// Output of the Lightning CSS pass.
pub struct LcssOutput {
    pub css: String,
    /// Source map JSON (v3) when the caller requested one, otherwise `None`.
    pub map: Option<String>,
}

/// Options for `transform`.
///
/// - `minify`: drop whitespace + collapse declarations.
/// - `source_map`: emit a source map. Requires `source_path` for the
///   map's `sources` field; if missing, the map points at `<galeforcecss>`.
/// - `targets`: optional per-browser minimum-version JSON
///   (`{"chrome":"95","firefox":"94", ...}`). When set, Lightning lowers
///   modern CSS features (nesting, color-mix, etc.) and adds vendor
///   prefixes for those targets. We accept structured JSON rather than
///   a browserslist query string because the `browserslist-rs` data
///   pack pulled in by the `browserslist` feature requires a newer
///   rustc than our toolchain pins (Rust 1.82). JS-side callers can
///   resolve a browserslist query with the standard `browserslist`
///   npm package and ship the result here.
#[derive(Default)]
pub struct LcssOptions<'a> {
    pub minify: bool,
    pub source_map: bool,
    pub source_path: Option<&'a str>,
    pub source_content: Option<&'a str>,
    pub targets: Option<&'a serde_json::Value>,
}

/// Run the Lightning CSS pass. Returns `Ok` on success or an error
/// string the caller can surface as a `Diagnostic`. The caller is
/// expected to fall back to the un-transformed CSS on error.
pub fn transform(css: &str, opts: LcssOptions<'_>) -> Result<LcssOutput, String> {
    let stylesheet = StyleSheet::parse(css, ParserOptions::default())
        .map_err(|e| format!("lightningcss parse: {e}"))?;

    // Resolve the structured browser targets into Lightning's
    // `Targets`. Missing / empty means `Targets::default()` — no
    // lowering, no prefixing.
    let targets = match opts.targets {
        Some(v) if !v.is_null() => Targets::from(browsers_from_json(v)?),
        _ => Targets::default(),
    };

    // Configure the source-map sink up front. Lightning needs a
    // `&mut SourceMap` borrow for the lifetime of the print call.
    let source_path = opts.source_path.unwrap_or("<galeforcecss>");
    let mut source_map_sink = if opts.source_map {
        let mut sm = parcel_sourcemap::SourceMap::new("/");
        // Pre-register the source so DevTools can resolve clicks.
        let _ = sm.add_source(source_path);
        if let Some(content) = opts.source_content {
            let _ = sm.set_source_content(0, content);
        }
        Some(sm)
    } else {
        None
    };

    let printer_opts = PrinterOptions {
        minify: opts.minify,
        targets,
        source_map: source_map_sink.as_mut(),
        ..PrinterOptions::default()
    };

    let printed = stylesheet
        .to_css(printer_opts)
        .map_err(|e| format!("lightningcss print: {e}"))?;

    let map_json = if let Some(mut sm) = source_map_sink {
        Some(
            sm.to_json(None)
                .map_err(|e| format!("sourcemap json: {e}"))?,
        )
    } else {
        None
    };

    Ok(LcssOutput {
        css: printed.code,
        map: map_json,
    })
}

/// Build a Lightning `Browsers` struct from a structured JSON object.
/// Recognized keys: `chrome`, `firefox`, `safari`, `edge`, `ie`,
/// `opera`, `samsung`, `android`, `ios_saf`. Values are version strings
/// (`"95"`, `"95.0"`, `"15.4"`). Unknown keys are ignored.
fn browsers_from_json(v: &serde_json::Value) -> Result<Browsers, String> {
    let mut b = Browsers::default();
    let obj = v
        .as_object()
        .ok_or_else(|| format!("targets must be an object, got {v}"))?;
    for (key, value) in obj {
        let Some(s) = value.as_str() else { continue };
        let parsed = parse_browser_version(s)
            .ok_or_else(|| format!("invalid version `{s}` for browser `{key}`"))?;
        match key.as_str() {
            "chrome" => b.chrome = Some(parsed),
            "firefox" => b.firefox = Some(parsed),
            "safari" => b.safari = Some(parsed),
            "edge" => b.edge = Some(parsed),
            "ie" => b.ie = Some(parsed),
            "opera" => b.opera = Some(parsed),
            "samsung" => b.samsung = Some(parsed),
            "android" => b.android = Some(parsed),
            "ios_saf" => b.ios_saf = Some(parsed),
            _ => {}
        }
    }
    Ok(b)
}

/// Parse a version string (`"95"`, `"95.0"`, `"15.4.1"`) into Lightning's
/// packed u32 representation. Lightning stores versions as
/// `major << 16 | minor << 8 | patch`.
fn parse_browser_version(s: &str) -> Option<u32> {
    let mut parts = s.split('.');
    let major: u32 = u32::from_str(parts.next()?).ok()?;
    let minor: u32 = parts
        .next()
        .and_then(|p| u32::from_str(p).ok())
        .unwrap_or(0);
    let patch: u32 = parts
        .next()
        .and_then(|p| u32::from_str(p).ok())
        .unwrap_or(0);
    Some((major << 16) | (minor << 8) | patch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minify_collapses_whitespace() {
        let css = ".a {\n  color: red;\n}\n.b {\n  margin: 0;\n}";
        let out = transform(
            css,
            LcssOptions {
                minify: true,
                ..LcssOptions::default()
            },
        )
        .unwrap();
        assert!(!out.css.contains('\n'));
        assert!(out.css.contains(".a") && out.css.contains(".b"));
        assert!(out.map.is_none());
    }

    #[test]
    fn pretty_prints_without_minify() {
        let css = ".a{color:red}";
        let out = transform(css, LcssOptions::default()).unwrap();
        // Pretty output has newlines + spaces.
        assert!(out.css.contains('\n'));
    }

    #[test]
    fn source_map_returns_json() {
        let css = ".a { color: red; }";
        let out = transform(
            css,
            LcssOptions {
                source_map: true,
                source_path: Some("input.css"),
                source_content: Some(css),
                ..LcssOptions::default()
            },
        )
        .unwrap();
        let map = out.map.expect("map present");
        assert!(map.contains("\"version\""));
        assert!(map.contains("input.css"));
    }

    #[test]
    fn invalid_css_returns_error() {
        let out = transform(".a { color }", LcssOptions::default());
        assert!(out.is_err(), "missing value should fail parse");
    }
}
