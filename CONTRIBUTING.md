# Contributing to GaleforceCSS

## Prerequisites

- Node.js 18.18+ (see `engines` in `package.json`)
- pnpm 9.x
- Rust toolchain pinned by [`rust-toolchain.toml`](./rust-toolchain.toml)

## First-time setup

```bash
git clone git@github.com:coralogix/internal-galeforce-css.git
cd internal-galeforce-css
git submodule update --init --recursive   # pulls vendor/tailwindcss-v3
pnpm install
pnpm oracle:version
```

## Development loop

```bash
# JS / TS
pnpm typecheck
pnpm test

# Rust
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Conformance
pnpm conformance:test            # fail on diff
pnpm conformance:update          # rewrite oracle snapshots
```

## Conformance philosophy

Every utility, variant, or directive ships with a fixture under
`conformance/fixtures/`. Implementations are validated by **comparing Galeforce's
CSS against the official Tailwind 3.4.19 oracle**. Semantic equivalence is the
contract, not byte-equality.

When you add a fixture:

1. Write the JSON fixture (`name`, `candidates`, `inputCss?`, `config?`).
2. Run `pnpm conformance:update` to record the oracle snapshot.
3. Implement the feature.
4. `pnpm conformance:test` should now pass for that fixture.

## Updating the Tailwind v3 reference

The pinned commit lives in `.gitmodules`. To bump:

```bash
cd vendor/tailwindcss-v3
git fetch origin
git checkout <new-commit>
cd ../..
git add vendor/tailwindcss-v3
# Bump tailwindcss in package.json to the matching release
pnpm install
pnpm oracle:version
```

## Code style

- Rust: `cargo fmt`, `cargo clippy -D warnings`.
- TS: Prettier (config in `.prettierrc`), strict TS in `tsconfig.base.json`.
- No emojis in code or docs unless explicitly requested.

## Commit hygiene

- Keep PRs focused. A new utility + its fixture is one PR.
- Conformance changes need oracle snapshot regeneration in the same PR.

## Releases

Releases are cut manually via the **Release** workflow (`workflow_dispatch`
with a `bump` input of `patch`/`minor`/`major`). The workflow cross-builds the
CLI + napi-rs addons for all five platform triples, publishes every `@cx`
package to Coralogix's internal JFrog Artifactory, and pushes a `vX.Y.Z` tag.

The in-repo `package.json` `version` fields are **not** the source of truth —
they're frozen at their initial value and never updated by the release
workflow (which stamps the resolved version into the manifests ephemerally in
CI and discards the edits when the job exits). Nothing is ever committed or
pushed to `master`; this keeps the workflow clear of branch-protection rules.

The latest `v*` git tag is authoritative. To see what version is currently on
JFrog:

```bash
git tag --list 'v*' --sort=-v:refname | head -n1
```

Pass `dry_run: true` to exercise the build + pack path without publishing or
tagging.
