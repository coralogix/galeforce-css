# Notes for Claude / coding agents working on GaleforceCSS

This is durable context. The current task / phase status lives in
[`todo.md`](./todo.md); the long-form plan lives there too.

## What this project is

A Rust-powered, **Tailwind CSS v3-compatible** compiler with a Vite
plugin. Compatibility is pinned to `tailwindcss@3.4.19` — that exact
release is the conformance oracle. We are not Tailwind Labs; we are
re-implementing the no-plugin v3 surface in Rust for speed and stability.

## Porting philosophy

**Treat this as a port, not a clean-room reimplementation.** When you
need to make a behavioral decision — what does this variant emit, what
does this directive expand to, how does this util look up a theme key —
the answer comes from reading the JS source at the path below, not from
intuition. Mirror Tailwind's *observable output* exactly (modulo the
PostCSS normalizer); the conformance harness will catch you if you
guess.

Where the JS implementation has tests that capture a behavior corner
(e.g. `tests/apply.test.js`, `tests/variants.test.js`), use those as
the basis for our Rust tests — same inputs, same expected outputs. A
test that ports cleanly is a test we can trust.

That said, **internal implementation is free to differ** wherever it
doesn't change observable output:

- Content scanning, file I/O, and the candidate cache can be parallelised
  or rewritten with different data structures — Tailwind is single-
  threaded JS; we don't have to be.
- Bracket-aware tokenizers, selector escape, sort-key construction, etc.
  can be byte-tables or hand-rolled state machines instead of regex.
- The CSS AST shape, the rule-emit format, and crate boundaries are
  ours to choose.

The rule of thumb: **mirror the source for behavior, mirror the tests
for confidence, but write idiomatic Rust for everything else.**

## Authoritative references

When a question is ambiguous, do not guess. Read the source. There are
two pinned copies of `tailwindcss@3.4.19` in this repo and they serve
different purposes:

- **`vendor/tailwindcss-v3/`** is a git submodule pinned at the v3.4.19
  commit (`f38be227df384504a170409c2131ca5ca8bfe025`). This is the
  authoritative source path — it includes `tests/` (28k lines of upstream
  test cases we can mine for fixture inputs and expected outputs).
  Important paths:
  - `src/corePlugins.js` — every utility/variant definition
  - `src/util/escapeClassName.js`, `src/util/escapeCommas.js` — selector escape
  - `src/util/defaultTheme.js`, `stubs/config.full.js` — default theme values
  - `src/css/preflight.css` — preflight reset
  - `src/lib/setupContextUtils.js` — candidate parsing, variant logic
  - `src/lib/expandTailwindAtRules.js` — `@tailwind` / `@layer` / `@apply`
  - `src/lib/expandApplyAtRules.js` — `@apply` resolution
  - `src/lib/evaluateTailwindFunctions.js` — `theme()` / `screen()` calls
  - `src/lib/substituteScreenAtRules.js` — `@screen md` rewriting
  - `tests/apply.test.js`, `tests/variants.test.js`, `tests/dark-mode.test.js`,
    `tests/arbitrary-values.test.js`, `tests/escapeClassName.test.js`, etc.
    — port edge cases from these into our fixtures.

  Bootstrap with `git submodule update --init --recursive` after a
  fresh clone. `pnpm oracle:version` verifies the submodule + npm
  versions match.

- **Executable oracle**: `tailwindcss@3.4.19` is also installed as an
  npm dependency at `node_modules/.pnpm/tailwindcss@3.4.19_*/`. Same
  bytes as the submodule, but importable into Node code — that's how
  `@coralogix/galeforcecss-oracle` calls into Tailwind for conformance testing. Use
  this when you want to run `compileWithTailwind3` to see what Tailwind
  emits for some input.

  Use the **JS reference**, not your memory, when implementing or
  debugging an escape rule, parsing detail, or default theme value.

- **Conformance harness** (`packages/galeforcecss-conformance`) compares your
  output against the live oracle output. Semantic equivalence after the
  PostCSS-based normalizer — not byte equality. If you wonder "what
  would Tailwind emit here?", the answer is `pnpm exec tsx -e "…"` with
  `compileWithTailwind3`.

- **`todo.md`** is the long-form spec and Phase plan.

## Repo layout

```
crates/                       # Rust workspace
  galeforce-core/                # shared types: errors, diagnostics, options, results
  galeforce-scanner/             # file walking + candidate tokenizer + incremental cache
  galeforce-parser/              # candidate parser (variants, important, prefix, modifier)
  galeforce-theme/               # theme resolution (stub)
  galeforce-css/                 # CSS AST + selector escaping + emitter
  galeforce-sort/                # rule ordering (stub)
  galeforce-compiler/            # parser -> utility lookup -> Rule, with diagnostics
  galeforce-cli/                 # `galeforcecss compile-json` (and others, stubbed)
  galeforce-node/                # napi-rs bindings (stub; not wired yet)

packages/                     # pnpm workspace
  galeforcecss/                  # public Node API (stub)
  vite-plugin-galeforcecss/      # Vite plugin (stub)
  galeforcecss-oracle/           # wraps tailwindcss@3.4.19 for conformance
  galeforcecss-conformance/      # fixture runner; spawns target/release/galeforcecss

conformance/fixtures/         # JSON fixtures consumed by the harness
scripts/                      # verify-tailwind-reference.ts, etc.
```

## Verification gates

Run all of these before declaring a phase done:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p galeforce-cli --release        # required for conformance bridge
pnpm oracle:version                         # confirm tailwindcss@3.4.19
pnpm typecheck
pnpm test                                   # vitest, all packages
pnpm conformance:test                       # fixture runner (alias of vitest)
pnpm --filter @coralogix/galeforcecss-conformance exec tsx src/cli.ts   # human-readable run
```

CI mirrors these. The conformance harness needs the Rust binary at
`target/release/galeforcecss` (or `target/debug/`, or `GALEFORCE_CLI_BIN`)
because the bridge spawns it. If the binary is absent the bridge raises
`GaleforceNotImplemented`, which marks fixtures as `unimplemented` rather
than failing.

## Toolchain pins

- Rust **1.82.0** (pinned in `rust-toolchain.toml`).
- Cargo workspace deps use exact `=x.y.z` pins because indexmap 2.14+
  needs edition 2024 which 1.82 doesn't have. Don't loosen them; bump
  the toolchain instead if a newer dep is needed.
- pnpm **9.x**, Node **18.18+** (CI uses 20).
- `tailwindcss@3.4.19` is a hard pin — `pnpm oracle:version` will fail
  CI if it drifts.

## Conformance philosophy

Every utility / variant / directive must ship with a fixture in
`conformance/fixtures/`. A fixture is a JSON file with `name` and
`candidates`; everything else is optional (see `Fixture` interface in
`packages/galeforcecss-conformance/src/fixture.ts`).

The runner:
1. Compiles the fixture's candidates with the official Tailwind oracle.
2. Compiles the same candidates with Galeforce.
3. Normalizes both outputs via `normalizeCss` (PostCSS-backed).
4. Diffs them. Empty diff → fixture passes.

Default config for fixtures is `{ corePlugins: { preflight: false } }`
so we focus on utility output, not the 3kB preflight reset. Fixtures
needing preflight set `mode: "full"`.

When you implement a new feature, **write the fixture first**. If the
oracle compiles and Galeforce doesn't, the fixture fails loudly — that's
the test-driven seed the work-queue calls for.

## Conventions

- **No emojis** in code, docs, or commit messages unless the user
  explicitly asks. CLAUDE.md and README.md included.
- **Phase-scoped commits**: one commit per Phase A/B/C/… landing,
  message includes a short summary of every public change. See git log
  for the established pattern. Use `git log --oneline` to confirm
  before adding new commits.
- **Test-first** for new utilities/variants. The fixture is the contract.
- **Mirror Tailwind exactly** for behavior/output. Read the JS source,
  don't guess. If your tests against the live JS reference disagree
  with your mental model, the model is wrong.
- **Tokenizer over-extracts on purpose.** The scanner produces noise
  tokens like `div`, `class`, `className` — that's correct; the parser
  rejects them. Don't add filters in the scanner to "clean it up."
- **Cache uses net-change accounting.** The HMR fast path depends on
  a no-op edit producing an empty `ScanDelta`. Don't decrement-then-
  increment refcounts in two passes.
- **Selector escaping matches `cssesc({isIdentifier:true})` + `escapeCommas`
  byte-for-byte.** It's been verified against the live JS reference
  (see `crates/galeforce-css/src/escape.rs` test cases). Run that JS one-
  liner whenever you doubt an output.

## Quick recipe: probing the oracle

```bash
pnpm --filter @coralogix/galeforcecss-oracle exec tsx -e "
import('@coralogix/galeforcecss-oracle').then(async ({ compileWithTailwind3 }) => {
  const { css } = await compileWithTailwind3({
    candidates: ['hover:flex', 'md:bg-red-500'],
    inputCss: '@tailwind utilities;',
    config: { corePlugins: { preflight: false } },
  })
  console.log(css)
})
"
```

This is the fastest way to settle "what should the output be?"
arguments. Use it freely.

## Quick recipe: probing Galeforce

```bash
cargo build -p galeforce-cli --release
echo '{"candidates":["flex","hover:flex"]}' | ./target/release/galeforcecss compile-json | jq
```

The output is a `CompileResult` JSON: `output.css`, `diagnostics`,
`candidateCount`, `ruleCount`. Diagnostics with code `unsupported-
candidate` mean Galeforce parses the candidate but doesn't support that
feature yet (variants, modifiers, negatives during Phase C).

## Phase status

See [`todo.md`](./todo.md) for the full plan; current commit history
shows what's landed.

```text
A. Foundation                 done    (commit 973de75)
B. Scanner + parser           done    (commit 2ade0b6)
C. Minimal static compiler    done    (commit 9639319)
D. Variants                   done
E. CSS directives             done
F. Vite + CLI + tooling       done    (CLI bridge instead of napi-rs — see carry-over)
G. Project conformance        done    (smoke vs horizon-tailwind-react: 0 diff on 706 rules)
H. Performance                in progress (baseline established — see below)
I. Release                    pending
```

## Phase H — current numbers

Run via `pnpm --filter @coralogix/galeforcecss-conformance bench` (synthetic) or
`bench -- --project <path>` (real project), `cargo bench -p galeforce-compiler`
for the pure compute path.

End-to-end on horizon-tailwind-react (2,551 candidates, 706 rules):

```
Tailwind 3 CLI build          246 ms
Galeforce CLI build               10 ms     <-- 23x faster

Tailwind 3 (warm import)       15 ms
Galeforce warm stream              2 ms     <-- 7.5x faster
```

End-to-end on the synthetic smoke project (734 candidates):

```
Tailwind 3 CLI build          195 ms
Galeforce CLI build                5 ms     <-- 38x faster
```

Pure compute (no spawn / JSON I/O), full real corpus: 1.24ms.

Per-candidate cost (`cargo bench -p galeforce-compiler`, 1000-element
loops):

```
static `flex`                       0.34µs
unknown `AbortController`           0.21µs
`bg-red-500`                        0.41µs
`dark:hover:bg-blue-500/50`         0.41µs
```

How we got here (chronological):

1. **`default_theme()` cached in `OnceLock`** — was being rebuilt
   per-candidate (~12k JSON-tree allocations on the real corpus).
   245ms → 16.5ms (15x).
2. **Prefix-indexed `find_value_utilities`** — replaces a linear
   walk over ~150 entries with a `HashMap<&str, Vec<&'static
   ValueUtility>>`. 16.5ms → 2.4ms (6.8x).
3. **Rayon parallelization** — `par_iter` over candidates ≥ 256;
   serial loop below the threshold to avoid fork-join overhead on
   small projects. 2.4ms → 1.2ms (1.9x).
4. **Direct JSON `.get()` walks in `lookup_theme_value`** — drop
   the `format!("{theme_key}.{value_key}")` + `path.split('.')`
   round-trip. Marginal but free.

Hot loop is now near-optimal at 0.4µs/candidate for color resolution.
Further gains would need lower-level changes (skipping String
allocations in the rule emitter, tighter rayon chunking).

## Phase carry-over

Items intentionally deferred from a phase that landed. Pick these up
when their containing surface gets re-touched, or in a "loose ends"
sweep before pre-alpha:

- **Sort key — simplified port shipped.** Candidate-derived rules are
  ordered by a 5-tuple `(tier, min_width, prefix, numeric, input_index)`
  so `class="p-2 p-4"` last-wins correctly under standard CSS cascade
  semantics. Tiers: 0 = no variants, 1 = pseudo-class, 2 = at-rule
  wrapped (responsive). Within tier 2 rules sort by parsed `min-width`
  in pixels so `sm: < md: < lg: < xl: < 2xl:`. Within a tier, rules
  group by class-prefix bucket then by numeric value (`p-1 < p-2 <
  p-4`; negatives sort before positives). Carry-over: this isn't a
  faithful port of upstream's `Offsets` bigint bitfield system —
  rules from different plugin families are bucketed by lexical
  prefix rather than the canonical Tailwind plugin order, and
  arbitrary-variant ranking isn't modeled. The conformance harness
  diffs unordered so this is invisible to fixtures; revisit if a
  user reports a real-world cascade conflict that the simplified key
  doesn't resolve.

- **Phase D § 14.7** — object-form screens (`{ md: { min: '768px',
  max: '1023px' } }`) and the matching `complex-screen-config` /
  `mixed-screen-units` / `minmax-have-mixed-units` warnings. Tailwind
  3.4.19 disables `min-*`/`max-*` arbitrary variants in those configs;
  Galeforce currently treats every screen as simple-string and would
  emit divergent output if a user supplied an object config. No
  fixture exercises this surface today.

- **Phase E § 15.2 / 15.3 (partial)** — `@tailwind base;` is shipped:
  emits both the cascade-var defaults (`*, ::before, ::after { --tw-*:
  ... }` plus the `::backdrop` mirror) AND the preflight reset, both
  vendored from a one-shot oracle compile against the DEFAULT config
  (see `crates/galeforce-compiler/vendor/tailwind-base-cascade-vars.css`
  and `tailwind-base-preflight.css`). `corePlugins.preflight: false`
  skips the preflight portion only — cascade vars always emit because
  upstream sources them from transform/filter/ring/scroll-snap/etc.
  corePlugins, not preflight. **User theme overrides** for the four
  `theme()` paths preflight reads (`borderColor.DEFAULT`,
  `fontFamily.sans/mono`, `colors.gray.400`) are now resolved
  JS-side: `processRawConfigAsync` detects the override and runs
  Tailwind itself on `@tailwind base;` to capture the resolved
  preflight, ships it under `config.__resolvedBase`. The Rust
  compiler prefers that over the vendored form when present.

- **Phase E § 15.4 (mostly shipped)** — `@layer base/components/utilities`
  blocks are now collected in a pre-pass and emitted at the matching
  `@tailwind X;` slot regardless of source position. Tree-shaking
  is wired for utilities + components: each top-level child of those
  layers is kept only if at least one class referenced in its
  selectors (or any nested at-rule's selectors) appears in the
  candidate set. Unknown layer names (`@layer custom { … }`) pass
  through verbatim. Carry-over: nested `@layer base|components|utilities`
  inside `@media`/`@supports` is silently dropped (the precollect
  is top-level only); sort-key precedence between candidates and
  `@layer utilities` siblings isn't modeled (today it's source order
  + insertion order, matching the oracle for the common cases).

- **Phase E § 15.5 (partial)** — `@apply` now resolves both static
  AND value-bearing utilities, with full variant support. Variant-
  free `@apply <util>` inlines decls into the parent rule;
  `@apply hover:<util>` / `@apply md:<util>` emits sibling rules
  with the variants applied to the parent selector. Sibling-pair
  plugins (`@apply space-x-4`, `@apply hover:divide-x-2`) emit on
  the parent + plugin-specific selector suffix; `@apply animate-*`
  ships matching `@keyframes` blocks at top level. Direct port of
  upstream's `expandApplyAtRules.js`. `@apply` now also honors a
  configured `prefix` — `@apply tw-flex` works under `prefix: 'tw-'`.

- **Phase E § 15.6** — `theme()` opacity shorthand
  (`theme('colors.red.500/50')`). Implementing it requires
  `withAlphaValue` / color parsing, which lands with value-bearing
  utilities.

- **Plugins (full).** `addUtilities`, `addComponents`, `addBase`,
  `addVariant` (string + array + function form), `matchUtilities`,
  `matchComponents`, `matchVariant` are all wired. JS-side
  `@coralogix/galeforcecss-config-loader` runs each plugin against a recording
  context (`packages/galeforcecss-config-loader/src/plugin-runner.ts`),
  captures the calls into a structured `PluginOutput`, and embeds it
  under `config.__pluginOutput`. The Rust compiler reads that field,
  indexes plugin utilities by primary class name (matched alongside
  built-in static/value tables in `compile_candidate`), and registers
  plugin variants in `VariantContext.plugin_variants` (looked up
  before the built-in match in `resolve_variant`).
  - `matchUtilities`/`matchVariant` `options.values` are
    materialized at plugin-run time.
  - **Arbitrary-value forms** (`my-[checked]`, `tab-[3.5]`) are
    materialized at config-load time when the candidate set is
    available: `processRawConfig{,Async}` accepts a `candidates`
    parameter; the runner keeps live function refs and invokes them
    per unique arbitrary value found in the candidates. Mirrors
    Tailwind v3's JIT behavior.
  - Function-form `addVariant` gets a minimal postcss-container
    shim with `walkRules` + `modifySelectors`. Plugins that rely on
    deeper postcss semantics still produce empty records and emit
    a warning.

- **Phase F — napi-rs binding.** The public `galeforcecss` package + the
  Vite plugin currently route through the CLI bridge (spawn the Rust
  binary, JSON-in/JSON-out). For long-running tooling we expose
  `createCompileStream()` which spawns one process and streams JSONL
  requests/responses, so per-call startup is amortised. A future
  napi-rs binding can replace the spawn without changing the JS API
  surface — `compile()` and `createCompileStream()` stay; only the
  module that backs them changes. Worth doing once we have shipped
  binaries (Phase I-ish), since napi-rs adds platform-specific
  `.node` build complexity that doesn't pull weight while we're
  pre-alpha.

- **Phase F — CLI surfaces (mostly shipped).** `galeforcecss watch` is
  in: debounced filesystem watcher (notify 6.1.1) on input CSS +
  config + content roots, calls the same `do_one_build` body as
  `build`. `galeforcecss init` scaffolds `tailwind.config.js` +
  `src/index.css` with `--force` and `--input <path>` overrides.
  Carry-over: `--minify` and `--diagnostics` flags on `build` /
  `watch`.

- **Phase F — Vite plugin scanner wiring.** The plugin compiles via
  the Rust compiler and HMRs on file change, but the candidate set
  it sends is empty — actual content scanning is Phase G. The wiring
  is in place (`watcher.on('change', invalidate)`); the only missing
  piece is "walk content roots, deduplicate tokens, send them as
  `candidates`."

When you finish a phase, commit it with a message that lists the
public additions and the post-phase test counts (Rust tests, JS tests,
conformance fixtures). Keep the working tree clean before handing off.
