# Architecture

Rust workspace handles compilation; JS workspace handles config loading and Vite/CLI integration.

## Pipeline

```
content files ─► scanner ─► candidate set ─► parser ─► compiler ─► sort ─► emit ─► CSS
                                                          ▲
                                input CSS ─► directives processor (@tailwind / @apply / @layer / theme())
                                                          ▲
                                                       resolved config (JS-side loader)
```

A single `compile(opts) -> CompileResult` drives the whole thing. The
directive processor works on byte spans, not an AST round-trip.

## Crates

| Crate | Responsibility |
|---|---|
| `galeforce-core` | Shared types (`CompileOptions`, `CompileResult`, `Diagnostic`, …). |
| `galeforce-scanner` | File walking (`ignore` + `globset`), candidate tokenizer, incremental cache. Parallel via `rayon`. |
| `galeforce-parser` | Splits variant chains, modifiers, prefix, important, arbitrary values. |
| `galeforce-css` | CSS rule model, selector escape, emit. |
| `galeforce-compiler` | Utility lookup, variant resolution, directive processor, `@apply`/`theme()`, `compile()` entry. |
| `galeforce-cli` | `galeforcecss` binary (`build`, `watch`, `scan`, `init`, `compile-json`, `compile-stream`). |
| `galeforce-node` | napi-rs binding stub (not yet wired). |

## Packages

| Package | Responsibility |
|---|---|
| `galeforcecss` | Public Node API. Routes through the CLI bridge. |
| `galeforcecss-config-loader` | Loads `tailwind.config.*`, runs Tailwind's `resolveConfig`, captures plugin output. |
| `vite-plugin-galeforcecss` | Vite integration. Drop-in PostCSS plugin + optional `virtual:galeforcecss.css`. |
| `@cx/galeforcecss-oracle` | Wraps `tailwindcss@3.4.19` for conformance. |
| `@cx/galeforcecss-conformance` | Fixture runner. Diffs after PostCSS normalization. |

## Scanner

Walks `content` roots with the `ignore` crate. Standard prunes:
`node_modules`, `.git`, `dist`, `build`, `coverage`, `.vite`. The
tokenizer is **deliberately over-permissive** — any run of valid
class-character bytes becomes a candidate. The parser rejects non-utility tokens.

Incremental cache stores per-file `(hash, size, mtime, tokens)` plus a
global `HashMap<candidate, refcount>`. Uses **net-change accounting**:
a no-op edit produces an empty `ScanDelta`. The HMR fast path depends on this.

## Parser

Candidates split top-to-bottom: variants → important → prefix → utility
segment → modifier → arbitrary value. Bracketed runs are balanced (so
`bg-[rgb(0,0,0)]` survives the comma). Variant chains stay in source
order; the compiler applies them outermost-first.

## Utility compiler

Two `OnceLock`-initialized tables:

- **Static utilities** — `flex`, `block`, `sr-only`, … Direct `&str -> &[(prop, val)]` map. ~600 entries.
- **Value utilities** — `bg-`, `text-`, `mt-`, … Prefix-keyed `HashMap<&str, Vec<&'static ValueUtility>>`. Each entry knows its theme key, value types, opacity handling, negation support.

Color resolution (`color.rs`) handles theme values, nested palettes,
`DEFAULT` keys, hex/rgb/hsl, CSS variables, slash-form opacity.

## Variant compiler

```rust
enum Variant {
    PseudoClass(&'static str),
    PseudoElement(&'static str),
    AtRule { rule: String, body: ... },
    Selector(String),                  // arbitrary [&>*]
    Group(...), Peer(...),
    Aria(...), Data(...), Supports(...),
    ...
}
```

Outermost-first application: `dark:hover:md:flex` →
`@media (prefers-color-scheme: dark) { @media (min-width: 768px) { .dark\:hover\:md\:flex:hover { display: flex; } } }`.
Pseudo-elements are terminal — chaining a pseudo-class after one is rejected.

## Rule sorting

5-tuple `(tier, min_width, prefix, numeric, input_index)`:

| Component | Meaning |
|---|---|
| `tier` | 0 = no variants, 1 = pseudo-class, 2 = at-rule wrapped. |
| `min_width` | Parsed `min-width` in pixels for responsive variants. |
| `prefix` | Class-name prefix bucket (`p`, `bg`, `text`, …). |
| `numeric` | Numeric tail. Negatives sort before positives. |
| `input_index` | Source order fallback for last-wins. |

Simplified port of upstream's bigint `Offsets` system. The conformance
harness diffs unordered, so the simplification is invisible to fixtures.

## Directives processor

Single-pass byte scan recognising:

- `@tailwind base|components|utilities` — replaces with vendored preflight + cascade vars + candidate output.
- `@layer base|components|utilities { … }` — pre-collected, emitted at the matching `@tailwind` slot.
- `@apply <utils>` — resolves each utility (variant-aware), inlines declarations. Variant `@apply` emits sibling rules.
- `theme('a.b.c')` — walks the resolved theme tree, supports alpha shorthand.
- `screen(md)` / `@screen md` — rewrites to `@media (min-width: 768px)`.

Pre-pass is top-level only — nested `@layer` inside `@media`/`@supports` is silently dropped.

## Adding a utility

1. Static or value-bearing?
2. Static → `static_utilities.rs` with the `u!` macro:

   ```rust
   u!("aspect-square", "aspectRatio", [("aspect-ratio", "1 / 1")]),
   ```

3. Value → `value_utilities.rs`:

   ```rust
   ValueUtility::new("aspect", "aspectRatio", &["aspect-ratio"])
       .with_supports_arbitrary(true),
   ```

4. Fixture in `conformance/fixtures/value-utilities/aspect.json`.
5. `pnpm conformance:test` — empty diff → done.

## Adding a variant

1. Pseudo-class, pseudo-element, at-rule wrap, or selector transform?
2. Register in `variants.rs` (`resolve_variant`).
3. Fixture in `conformance/fixtures/variants/`.
4. `pnpm conformance:test`.

For interactions with `group-*` / `peer-*` / `@apply` / important /
prefix, write **separate fixtures per interaction**.

## Updating oracle snapshots

When bumping the Tailwind pin:

```bash
pnpm oracle:version
pnpm fixtures:rebuild
git diff conformance/snapshots/    # what upstream now emits differently
```

Decide what's a real upstream change (port it) vs. a Galeforce regression (fix it).
