## Summary

Describe what changed and why.

## Checklist

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `pnpm typecheck` and `pnpm test`
- [ ] `pnpm conformance:test` (new/changed utilities, variants, or directives ship with a fixture)
- [ ] `pnpm license:check` (new source files carry the Apache 2.0 header)
- [ ] I updated docs where needed
