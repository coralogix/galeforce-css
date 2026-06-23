# Conformance

GaleforceCSS is verified against the live `tailwindcss@3.4.19` oracle on every
push. Two layers:

| Layer | Passing | What it checks |
| --- | ---: | --- |
| Fixture diff | **894** | Compile each fixture with both compilers, diff after PostCSS normalization. |
| Byte-sensitive snapshot | **186** | Exact-byte output for utilities where formatting/order matters. |
| Real-world projects | 7 | Full project compiles produce zero semantic diff. |

Skipped: 67 upstream tests that rely on deep PostCSS internals GaleforceCSS doesn't model.

## How a fixture works

A fixture is a JSON file in `conformance/fixtures/`:

```json
{
  "name": "hover-flex",
  "candidates": ["flex", "hover:flex"]
}
```

The runner compiles the candidates with both compilers, normalizes with
PostCSS, and diffs. Empty diff → pass. Default config is
`{ corePlugins: { preflight: false } }`; fixtures needing preflight set
`mode: "full"`.

## Running it

```bash
cargo build -p galeforce-cli --release   # required: the harness spawns the binary
pnpm conformance:test
```

Against a real project:

```bash
pnpm --filter @coralogix/galeforcecss-conformance exec tsx src/cli.ts --project /path/to/project
```

## Probing the oracle

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

## Known gaps

Tracked in CLAUDE.md; invisible to the harness but worth knowing:

- **`theme()` opacity shorthand** — `theme('colors.red.500/50')` not resolved.
- **Object-form screens** — `{ md: { min: '768px', max: '1023px' } }` treated as a simple string.
- **Nested `@layer`** inside `@media`/`@supports` is silently dropped.
- **Sort order** — simplified 5-tuple key instead of upstream's bigint offset system. Diff-unordered, so invisible to fixtures.
