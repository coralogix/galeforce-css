# Contributing

## Prerequisites

- Rust 1.82+ (pinned in `rust-toolchain.toml`)
- Node 18.18+, pnpm 9.x

## Setup

```bash
git clone https://github.com/coralogix/internal-galeforce-css
cd galeforcecss
git submodule update --init --recursive
pnpm install
```

## Verification gates

Run all of these before opening a PR. CI mirrors them.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p galeforce-cli --release       # required by conformance tests
pnpm oracle:version                       # confirms tailwindcss@3.4.19
pnpm typecheck
pnpm test
pnpm conformance:test
```

## Porting philosophy

GaleforceCSS is a port, not a clean-room reimplementation. When making a
behavioral decision, the answer comes from reading the JS source at
`vendor/tailwindcss-v3/src/`, not from intuition.

**Mirror the source for behavior, mirror the tests for confidence, but
write idiomatic Rust for everything else.**

## Adding a feature

1. Write the fixture first:

   ```json
   {
     "name": "my-new-feature",
     "candidates": ["my-class-name", "hover:my-class-name"]
   }
   ```

2. Run the harness — it should fail:

   ```bash
   pnpm conformance:test
   ```

3. Implement in Rust. Check `vendor/tailwindcss-v3/src/corePlugins.js` for authoritative behavior.

4. Run the harness — the fixture should pass.

5. Open a PR with the fixture in the diff.

## Probing the oracle

```bash
pnpm --filter @cx/galeforcecss-oracle exec tsx -e "
import('@cx/galeforcecss-oracle').then(async ({ compileWithTailwind3 }) => {
  const { css } = await compileWithTailwind3({
    candidates: ['your-candidate-here'],
    inputCss: '@tailwind utilities;',
    config: { corePlugins: { preflight: false } },
  })
  console.log(css)
})
"
```

## Probing GaleforceCSS

```bash
cargo build -p galeforce-cli --release
echo '{"candidates":["flex","hover:flex"]}' | ./target/release/galeforcecss compile-json | jq
```

## Repository layout

```
crates/
  galeforce-core/             Shared types, errors, diagnostics
  galeforce-scanner/          Content scanning + incremental cache
  galeforce-parser/           Candidate parser
  galeforce-compiler/         Utility + variant compiler, CSS directives
  galeforce-css/              CSS AST + selector escaping
  galeforce-sort/             Rule ordering
  galeforce-cli/              galeforcecss CLI binary
  galeforce-node/             napi-rs bindings (deferred)
packages/
  galeforcecss/               Public Node API
  vite-plugin-galeforcecss/
  galeforcecss-oracle/        Wraps tailwindcss@3.4.19
  galeforcecss-conformance/   Fixture runner
  galeforcecss-config-loader/ tailwind.config.* resolver
conformance/fixtures/      JSON fixtures (one per feature/edge case)
vendor/tailwindcss-v3/     Pinned upstream submodule
```

## Commit style

```
<type>(<scope>): short description

Longer explanation if needed.
```

Types: `feat`, `fix`, `chore`, `docs`, `test`, `refactor`. See `git log --oneline` for the pattern.

## Bumping the Tailwind pin

The npm package and the `vendor/tailwindcss-v3` submodule must stay in sync. `pnpm oracle:version` fails CI on drift.

```bash
# 1. Bump npm
pnpm add -D -w tailwindcss@<new-version>

# 2. Move the submodule
cd vendor/tailwindcss-v3
git fetch origin --tags
git checkout v<new-version>
cd ../..
git add vendor/tailwindcss-v3

# 3. Update TARGET in scripts/verify-tailwind-reference.ts.

# 4. Verify
pnpm oracle:version
pnpm typecheck && pnpm test && pnpm conformance:test
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Review fixture diffs carefully — port real upstream changes, fix regressions, don't rubber-stamp.
