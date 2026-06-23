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

//! Default Tailwind 3.4.19 screens, plus a tiny resolver from the JSON
//! config the harness passes us.
//!
//! Source of truth:
//!   `tailwindcss/stubs/config.full.js` — the `theme.screens` block —
//!   sm 640px, md 768px, lg 1024px, xl 1280px, 2xl 1536px.
//!
//! Phase D scope is small on purpose:
//! * We only honor *simple string* screens (the form Tailwind itself emits
//!   in its default config). Object-form screens like
//!   `{ md: { min: '768px', max: '1023px' } }` are accepted by Tailwind but
//!   they disable the `min-*`/`max-*` arbitrary variants. We don't generate
//!   from them yet — fixtures opting into them will simply not match until
//!   the directive processor lands in Phase E.
//! * `theme.extend.screens` is merged on top of the defaults; explicit
//!   `theme.screens` replaces the defaults entirely (Tailwind semantics).

use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    pub name: String,
    /// The "min-width" value when the screen is a simple string,
    /// or the literal media query body when sourced from a `{ raw:
    /// '(...)' }` config entry. Disambiguated by `is_raw`.
    pub value: String,
    /// True when `value` is the full media query body (e.g.
    /// `(min-aspect-ratio: 1/10)`) rather than a `min-width: <px>`.
    /// Variant emission picks `@media <value>` for raw screens vs
    /// `@media (min-width: <value>)` for simple ones.
    pub is_raw: bool,
}

pub fn default_screens() -> Vec<Screen> {
    vec![
        Screen {
            name: "sm".into(),
            value: "640px".into(),
            is_raw: false,
        },
        Screen {
            name: "md".into(),
            value: "768px".into(),
            is_raw: false,
        },
        Screen {
            name: "lg".into(),
            value: "1024px".into(),
            is_raw: false,
        },
        Screen {
            name: "xl".into(),
            value: "1280px".into(),
            is_raw: false,
        },
        Screen {
            name: "2xl".into(),
            value: "1536px".into(),
            is_raw: false,
        },
    ]
}

/// Pull `theme.screens` (or `theme.extend.screens` merged on top of defaults)
/// out of a Tailwind config JSON object. Falls back to the default table
/// when no override is present.
pub fn resolve_screens(config: &Value) -> Vec<Screen> {
    let theme = config.get("theme");
    if let Some(t) = theme {
        if let Some(screens) = t.get("screens").and_then(Value::as_object) {
            return read_screen_object(screens);
        }
        if let Some(ext) = t
            .get("extend")
            .and_then(|e| e.get("screens"))
            .and_then(Value::as_object)
        {
            let mut out = default_screens();
            for (k, v) in ext {
                if let Some(screen) = parse_screen_value(k, v) {
                    if let Some(existing) = out.iter_mut().find(|sc| sc.name == *k) {
                        *existing = screen;
                    } else {
                        out.push(screen);
                    }
                }
            }
            return out;
        }
    }
    default_screens()
}

fn read_screen_object(obj: &serde_json::Map<String, Value>) -> Vec<Screen> {
    obj.iter()
        .filter_map(|(k, v)| parse_screen_value(k, v))
        .collect()
}

/// Parse a single screen entry. Supports the simple string form
/// (`'md': '768px'`) and the raw-media-query object form
/// (`'ar-1/10': { raw: '(min-aspect-ratio: 1/10)' }`). The raw form
/// emits `@media <body>` directly; simple strings emit
/// `@media (min-width: <value>)`. Returns `None` for shapes we
/// don't yet model (e.g. `{ min: '768px', max: '1023px' }` — Phase
/// D §14.7 carry-over).
fn parse_screen_value(name: &str, value: &Value) -> Option<Screen> {
    match value {
        Value::String(s) => Some(Screen {
            name: name.to_string(),
            value: s.clone(),
            is_raw: false,
        }),
        Value::Object(map) => {
            if let Some(raw) = map.get("raw").and_then(Value::as_str) {
                return Some(Screen {
                    name: name.to_string(),
                    value: raw.to_string(),
                    is_raw: true,
                });
            }
            // Single-bound `{ min: 'X' }` / `{ 'min-width': 'X' }` is
            // equivalent to a simple string. `{ max: 'X' }` /
            // `{ 'max-width': 'X' }` emits `(max-width: X)`. Combined
            // `{ min, max }` emits `(min-width: X) and (max-width: Y)`.
            // Mirrors upstream's `normalizeScreens.js`. Note: the
            // combined form disables the arbitrary `min-*`/`max-*`
            // variants per Phase D § 14.7 — that's not handled here.
            let min = map
                .get("min")
                .and_then(Value::as_str)
                .or_else(|| map.get("min-width").and_then(Value::as_str));
            let max = map
                .get("max")
                .and_then(Value::as_str)
                .or_else(|| map.get("max-width").and_then(Value::as_str));
            if let (Some(min), None) = (min, max) {
                return Some(Screen {
                    name: name.to_string(),
                    value: min.to_string(),
                    is_raw: false,
                });
            }
            if let (None, Some(max)) = (min, max) {
                return Some(Screen {
                    name: name.to_string(),
                    value: format!("(max-width: {max})"),
                    is_raw: true,
                });
            }
            if let (Some(min), Some(max)) = (min, max) {
                return Some(Screen {
                    name: name.to_string(),
                    value: format!("(min-width: {min}) and (max-width: {max})"),
                    is_raw: true,
                });
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_match_tailwind_3_4_19() {
        let s = default_screens();
        assert_eq!(s.len(), 5);
        assert_eq!(
            s[0],
            Screen {
                name: "sm".into(),
                value: "640px".into(),
                is_raw: false,
            }
        );
        assert_eq!(
            s[4],
            Screen {
                name: "2xl".into(),
                value: "1536px".into(),
                is_raw: false,
            }
        );
    }

    #[test]
    fn theme_screens_replaces_defaults() {
        let cfg = json!({ "theme": { "screens": { "tablet": "900px" } } });
        let s = resolve_screens(&cfg);
        assert_eq!(
            s,
            vec![Screen {
                name: "tablet".into(),
                value: "900px".into(),
                is_raw: false,
            }]
        );
    }

    #[test]
    fn theme_extend_screens_merges() {
        let cfg =
            json!({ "theme": { "extend": { "screens": { "3xl": "1800px", "md": "800px" } } } });
        let s = resolve_screens(&cfg);
        assert!(s.iter().any(|sc| sc.name == "3xl" && sc.value == "1800px"));
        assert!(s.iter().any(|sc| sc.name == "md" && sc.value == "800px"));
        assert!(s.iter().any(|sc| sc.name == "sm")); // default still present
    }

    #[test]
    fn no_theme_falls_back_to_defaults() {
        let cfg = json!({});
        assert_eq!(resolve_screens(&cfg), default_screens());
    }
}
