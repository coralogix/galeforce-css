# Debugging

When output isn't what you expect, work from the outside in.

## 1. Compare against the oracle

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

Whatever Tailwind emits is the contract. If Galeforce differs, it's a bug.

## 2. Probe Galeforce the same way

```bash
cargo build -p galeforce-cli --release
echo '{"candidates":["flex","hover:flex"]}' | ./target/release/galeforcecss compile-json | jq
```

Look for `output.css`, `diagnostics`, `candidateCount`, `ruleCount`.
`unsupported-candidate` codes mean Galeforce parses the candidate but
doesn't model that feature yet.

## 3. Diagnostic flags

```bash
galeforcecss build -i src/index.css -o dist/output.css --content src --verbose
galeforcecss build … --diagnostics diagnostics.json   # JSON file
galeforcecss build … --diagnostics -                  # stderr
```

## 4. `GALEFORCE_DEBUG=1`

Prints timing on every build:

```
[galeforce-debug] scan=3.3ms compile=375µs total=3.6ms candidates=2551 rules=706
```

Useful when you suspect the scanner is sweeping a directory it shouldn't.

## 5. Check the scanner

```bash
galeforcecss scan --content src --json | jq 'length'
```

If the count is wildly off from `npx tailwindcss --list`, your
`content` paths probably miss a file extension. The scanner respects
`.gitignore` and prunes `node_modules` / `.git` / `dist` / `build` /
`coverage` / `.vite` by default.

## 6. Diff against the oracle on your real project

```bash
pnpm --filter @coralogix/galeforcecss-conformance exec tsx \
  scripts/galeforcecss-compare.ts \
  --project /path/to/your/app
```

Zero output = parity. Any output is the exact CSS diff to file.

## 7. Read the oracle source

When you need to know what Tailwind does for an edge case:

```
node_modules/.pnpm/tailwindcss@3.4.19_*/node_modules/tailwindcss/src/
```

Worth opening:

- `corePlugins.js` — utility/variant definitions.
- `util/escapeClassName.js` — selector escape.
- `lib/setupContextUtils.js` — candidate parsing, variants.
- `lib/expandTailwindAtRules.js` — `@tailwind` / `@layer` / `@apply`.
- `lib/evaluateTailwindFunctions.js` — `theme()` / `screen()`.

## Common pitfalls

- **No utilities in output** — `content` glob didn't match anything. Galeforce prints `warning — no utility classes were detected`. Widen the glob.
- **Plugin produces empty output** — plugin relies on PostCSS internals GaleforceCSS doesn't model. See [Unsupported features](./unsupported.md).
- **`@apply` fails with `unsupported-apply`** — the utility parses but uses something that doesn't compose with `@apply` (e.g. `space-x` needs `> * + *`, which `@apply` can't inline).
- **Output looks right but classes don't apply** — if you're using virtual-module mode, make sure your entry has `import 'virtual:galeforcecss.css'`.
