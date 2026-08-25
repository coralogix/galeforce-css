<p>
  Built with 💚 by
  <a href="https://coralogix.com/?utm_source=github&amp;utm_medium=oss&amp;utm_campaign=galeforcecss">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/coralogix/galeforce-css/master/assets/coralogix-horizontal-white-inline.svg">
      <img src="https://raw.githubusercontent.com/coralogix/galeforce-css/master/assets/coralogix-horizontal-black-inline.svg" alt="Coralogix" height="24" align="middle">
    </picture>
  </a>
</p>

# GaleforceCSS

A Rust-powered Tailwind CSS v3-compatible compiler. Drop-in replacement for
Tailwind v3 — same config, same directives, same output — built for speed.

> GaleforceCSS is **not** an official Tailwind Labs project. It is an independent
> port of Tailwind CSS v3, pinned to `tailwindcss@3.4.19` as the conformance
> oracle.

The compiler reaches zero semantic diff against the official Tailwind v3
oracle on eight real open-source projects.

Documentation: <https://coralogix.github.io/galeforce-css/>

---

## Why

Tailwind v3 + Vite is slow to start and slow to HMR because it runs in a
Node.js PostCSS pipeline. GaleforceCSS moves the hot path to Rust:

| Benchmark (horizon-tailwind-react, 2551 candidates) | Time    |
| ---------------------------------------------------- | ------- |
| Tailwind v3 CLI build                                | 246 ms  |
| GaleforceCSS CLI build                                  | 10 ms   |
| Tailwind v3 (warm import)                            | 15 ms   |
| GaleforceCSS warm stream                                | 2 ms    |

Pure compute (no I/O): 1.2 ms on the full corpus. Per-candidate: ~0.4 µs.

---

## Features

- Full Tailwind v3 utility set — spacing, sizing, typography, color, borders,
  effects, transforms, filters, flex, grid, and more.
- All built-in variants — responsive (`sm:`, `md:`, …), dark mode, pseudo-
  classes, pseudo-elements, `group-*`, `peer-*`, `aria-*`, `data-*`,
  `supports-*`, arbitrary variants (`[&>*]:`).
- CSS directives — `@tailwind`, `@layer`, `@apply`, `theme()`, `@screen`.
- Arbitrary values, opacity modifiers, negative utilities, important modifier.
- Prefix support (`prefix: 'tw-'`), `darkMode: 'class'|'media'|'selector'`.
- Reads your existing `tailwind.config.{js,cjs,mjs,ts}` unchanged.
- Plugin output — `addUtilities`, `addComponents`, `addBase`, `addVariant`,
  `matchUtilities`, `matchComponents`, `matchVariant` are all supported via
  a JS-side plugin runner that feeds recorded calls into the Rust compiler.

### Not supported in v1

- Third-party Tailwind plugins that rely on deep PostCSS internals.
- Tailwind v4.
- Byte-for-byte identical CSS formatting (semantics are identical; whitespace
  may differ).

---

## Installation

GaleforceCSS publishes to the public npm registry under the `@coralogix` scope:

```bash
# Vite plugin
npm install -D @coralogix/vite-plugin-galeforcecss

# Node API (optional — the Vite plugin pulls this in automatically)
npm install -D @coralogix/galeforcecss
```

GaleforceCSS ships a platform-specific native binary alongside each npm package.
No build step required.

---

## Vite

```js
// vite.config.js
import { defineConfig } from 'vite'
import galeforcecss from '@coralogix/vite-plugin-galeforcecss'

export default defineConfig({
  plugins: [galeforcecss()],
})
```

That's it. Your existing `@tailwind` / `@apply` / `theme()` / `@layer`
directives in any `.css` / `.scss` / `.sass` / `.less` file are processed
in-place by Vite's CSS pipeline — no app-side import required. Content
changes trigger a sub-millisecond recompile via HMR.

```css
/* src/index.css — unchanged */
@tailwind base;
@tailwind components;
@tailwind utilities;
```

Plugin options (all optional):

```js
galeforcecss({
  config: 'tailwind.config.js',       // auto-discovered if omitted
  content: ['src/**/*.{html,ts}'],    // defaults to tailwind.config `content`
  minify: true,                       // defaults to Vite's build.cssMinify
  sourceMap: true,                    // defaults to Vite's build.sourcemap
  targets: { chrome: '95' },          // Lightning CSS browser targets
})
```

For a single plugin-owned stylesheet instead of inline directive expansion,
set `input` and import the virtual module — see
[guide/vite.md](./docs/guide/vite.md).

---

## CLI

`@coralogix/galeforcecss` installs a `galeforcecss` command (use `npx
galeforcecss` outside an npm script):

```bash
# One-shot build
npx galeforcecss build --input src/index.css --output dist/output.css --content src

# Scan content and print candidates as JSON
npx galeforcecss scan --content src --json
```

Also available: `watch` (debounced rebuilds), `init` (scaffold a config),
`list-classes` / `list-variants` (editor tooling). See
[docs/guide/cli.md](./docs/guide/cli.md).

---

## Node API

```ts
import { compile } from '@coralogix/galeforcecss'

const result = await compile({
  candidates: ['flex', 'hover:bg-blue-500', 'md:px-4'],
  inputCss: '@tailwind utilities;\n',
  config: { corePlugins: { preflight: false } },
})

console.log(result.css)
```

For long-running tooling (avoiding per-call process startup):

```ts
import { createCompileStream } from '@coralogix/galeforcecss'

const stream = createCompileStream()

const a = await stream.compile({ candidates: ['flex'] })
const b = await stream.compile({ candidates: ['block'] })

stream.close()
```

---

## Conformance

GaleforceCSS maintains a conformance harness that compares its output against
the live Tailwind v3.4.19 oracle after PostCSS normalization. 941 checks pass:
88 fixture diffs, 501 test cases ported from upstream's suite, 186
byte-sensitive snapshots, and 166 ordering / plugin / config cases. 68 upstream
cases are skipped where they depend on PostCSS internals GaleforceCSS doesn't
model.

Eight real open-source projects compile with zero semantic diff: notus-nextjs,
notus-react, horizon-tailwind-react, material-tailwind,
shadcn-nextjs-boilerplate, merakiui, soft-ui-dashboard, and flowbite. Those
runs are local-only, since the projects aren't vendored into the repo. See
[docs/reference/conformance.md](./docs/reference/conformance.md).

---

## Contributing

### Prerequisites

- Rust 1.82+ (`rust-toolchain.toml` pins the version)
- Node 20.19+ or 22.12+, pnpm 10.x

### Setup

```bash
git clone https://github.com/coralogix/galeforce-css
cd galeforce-css
git submodule update --init --recursive   # vendor/tailwindcss-v3 source ref
pnpm install
```

### Verification gates

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p galeforce-cli --release       # binary required by conformance tests
pnpm oracle:version                       # confirms tailwindcss@3.4.19
pnpm typecheck
pnpm test
pnpm conformance:test
```

### Repository layout

```
crates/
  galeforce-core/          # Shared types, errors, diagnostics
  galeforce-scanner/       # Content scanning + incremental cache
  galeforce-parser/        # Candidate parser
  galeforce-compiler/      # Utility + variant compiler, CSS directives
  galeforce-css/           # CSS AST + selector escaping
  galeforce-sort/          # Rule ordering
  galeforce-cli/           # galeforcecss CLI binary
  galeforce-node/          # napi-rs bindings (wired in Phase I)
packages/
  galeforcecss/            # Public Node API
  vite-plugin-galeforcecss/
  galeforcecss-oracle/     # Wraps tailwindcss@3.4.19 for conformance
  galeforcecss-conformance/# Fixture runner: oracle vs. Galeforce
conformance/fixtures/   # JSON fixtures (one per feature/edge case)
vendor/tailwindcss-v3/  # Pinned upstream submodule (source reference)
```

The `vendor/tailwindcss-v3` submodule is the authoritative behavioral
reference. When implementing a utility or variant, read its source in
`src/corePlugins.js` and port edge cases from its `tests/` directory. See
`CLAUDE.md` for the full porting philosophy.

---

## License

Apache License 2.0. See [`LICENSE`](./LICENSE).

Copyright 2026 Coralogix Ltd.

## Contributor License Agreement

Contributions to GaleforceCSS require signing the Coralogix Contributor
License Agreement. See [`CLA.md`](./CLA.md). The CLA is enforced via
[CLA Assistant](https://cla-assistant.io/) — first-time contributors are
prompted to sign before their pull request can be merged.

## Security

To report a vulnerability, see [`SECURITY.md`](./SECURITY.md).
