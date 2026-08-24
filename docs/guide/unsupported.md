# Unsupported features

GaleforceCSS targets Tailwind v3. The core utility/variant/directive surface
is fully implemented and reaches **zero diffs** against the official
Tailwind v3 oracle on eight real-world projects. The list below captures
what's out of scope or deferred.

## Out of scope

- **Tailwind v4.** Different architecture (CSS-first, no config). Not on the roadmap.
- **Third-party plugins** that rely on full PostCSS container semantics. The plugin runner ships a `walkRules` + `modifySelectors` shim. Plugins beyond that surface produce empty records + a warning.
- **Byte-for-byte CSS formatting.** Output is *semantically* equivalent to Tailwind after PostCSS normalization; whitespace and rule-order differences are not bugs.
- **Browser WASM build.** Node-only today.
- **Tailwind v2 → v3 migration tooling.** Use upstream `npx tailwindcss-upgrade` first.

## Implemented with caveats

- **Object-form `screens` config** (`{ md: { min: '768px', max: '1023px' } }`). Treated as simple-string. Use the string form for now.
- **Sort key.** Simplified 5-tuple `(tier, min_width, prefix, numeric, input_index)`. Conformance harness diffs unordered, so the simplification is invisible to fixtures. File an issue if you hit a real cascade conflict.
- **`@tailwind` banner.** Oracle emits `/* ! tailwindcss ... */`; Galeforce doesn't. Output is semantically equivalent.
- **napi-rs native bindings** deferred. JS package + Vite plugin route through the `compile-stream` CLI bridge. Bindings will replace the bridge without changing `compile` / `createCompileStream`.
- **`@apply` inside nested `@media` / `@supports` `@layer` blocks.** Pre-collect pass is top-level only; nested layers are silently dropped.
- **Source maps.** Not generated.
- **Lightning CSS prefixing.** Output unprefixed. Add a downstream autoprefixer pass for legacy browsers.

## Diagnostic codes

Surface with `--verbose` or `--diagnostics <path>`, or read `CompileResult.diagnostics` in JS.

| Code | Meaning |
|---|---|
| `unknown-utility` | Candidate parsed but matched no utility. |
| `unsupported-candidate` | Candidate uses a feature Galeforce doesn't model yet. |
| `unsupported-apply` | `@apply <util>` referenced a utility Galeforce can't compile. |
| `unknown-theme-path` | `theme('x.y.z')` didn't resolve. |

If you hit one on something that should work, file an issue with the candidate + a minimal config.
