# Contributing to GaleforceCSS

## Contributor License Agreement

Before your first pull request can be merged, you must sign the Coralogix
Contributor License Agreement ([`CLA.md`](./CLA.md)). This is enforced
automatically via [CLA Assistant](https://cla-assistant.io/): when you open
your first PR, a bot will prompt you to sign. A PR cannot be approved until
the CLA is signed.

## Prerequisites

- Node.js 18.18+ (see `engines` in `package.json`)
- pnpm 9.x
- Rust toolchain pinned by [`rust-toolchain.toml`](./rust-toolchain.toml)

## First-time setup

```bash
git clone git@github.com:coralogix/galeforce-css.git
cd galeforce-css
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

## License headers

Every first-party source file must carry the Apache 2.0 header. This is
enforced in CI (the **License headers** job) via
[hawkeye](https://github.com/korandoru/hawkeye); the header template lives in
[`license-header.txt`](./license-header.txt) and the include/exclude rules in
[`licenserc.toml`](./licenserc.toml).

New files won't have the header until you add it. Install hawkeye once
(`cargo install hawkeye`, `brew install korandoru/tap/hawkeye`, or use the
Docker image), then:

```bash
pnpm license:fix      # insert missing headers
pnpm license:check    # verify (what CI runs)
```

Without a local install you can run the same check through Docker:

```bash
docker run --rm -v "$PWD:/github/workspace" ghcr.io/korandoru/hawkeye:v6 check
```

## Dependency licenses

The **Dependency licenses** CI job runs
[cargo-deny](https://embarkstudios.github.io/cargo-deny/) to verify every crate
in the dependency tree carries a license we can redistribute under Apache 2.0
(allow-list in [`deny.toml`](./deny.toml)). To run it locally:

```bash
cargo install cargo-deny
cargo deny check licenses
```

Note: cargo-deny resolves the full dependency graph, which includes wasm-only
deps that need Cargo's `edition2024`. The repo is pinned to Rust 1.82, so run
this with a newer toolchain, e.g. `RUSTUP_TOOLCHAIN=stable cargo deny check
licenses` (CI does this automatically).

## Commit hygiene

- Keep PRs focused. A new utility + its fixture is one PR.
- Conformance changes need oracle snapshot regeneration in the same PR.

## Releases

Releases are cut manually via the **Release** workflow (`workflow_dispatch`
with a `bump` input of `patch`/`minor`/`major`). The workflow cross-builds the
CLI + napi-rs addons for all five platform triples, publishes every
`@coralogix` package to the public npm registry, and pushes a `vX.Y.Z` tag.

The in-repo `package.json` `version` fields are **not** the source of truth —
they're frozen at their initial value and never updated by the release
workflow (which stamps the resolved version into the manifests ephemerally in
CI and discards the edits when the job exits). Nothing is ever committed or
pushed to `master`; this keeps the workflow clear of branch-protection rules.

The latest `v*` git tag is authoritative. To see what version is currently
published:

```bash
git tag --list 'v*' --sort=-v:refname | head -n1
```

Pass `dry_run: true` to exercise the build + pack path without publishing or
tagging.
