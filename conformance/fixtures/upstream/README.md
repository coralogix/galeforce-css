# Upstream-mined fixtures

Fixtures in this directory are derived from the test suite in
`vendor/tailwindcss-v3/tests/`. The submodule ships ~28k lines of upstream
Jest test cases — too valuable to ignore, since they encode edge cases
that the Tailwind team hit during five years of v3 development. Mining
them gives us regression coverage for free.

## Workflow

1. **Pick a test file.** Likely candidates:
   - `apply.test.js`, `arbitrary-variants.test.js`, `arbitrary-values.test.js`,
     `dark-mode.test.js`, `escapeClassName.test.js`, `important.test.js`,
     `prefix.test.js`, `variants.test.js`.

2. **Find a self-contained `test(...)` block.** Each block typically
   sets a `config`, defines a candidate set (often via the `html`
   tagged template), and asserts on an expected CSS output via
   `toMatchFormattedCss(css\`…\`)`.

3. **Port it to a fixture JSON.** Map:
   - `config` -> the fixture's `config` field (same shape).
   - the `html` candidate set -> `candidates`.
   - the test's `input` (usually `@tailwind base; @tailwind components;
     @tailwind utilities;`) -> `inputCss`. Trim to `@tailwind utilities;`
     unless `@tailwind base` is what's being tested.

4. **Swap unsupported utilities** if the test uses something we haven't
   implemented yet (most early-phase upstream tests use value-bearing
   utilities like `font-bold` or `bg-red-500`). Replace them with
   static utilities Galeforce does support — `flex`, `hidden`, `block`,
   `relative`, `sr-only`, etc. The fixture's behavior is then a port
   of the *variant or directive* logic, not the utility.

5. **Document the source.** Add a `description` field that names the
   original test file and the test block's title — future contributors
   should be able to diff our fixture against the upstream to spot
   drift after a Tailwind bump.

6. **Run the harness** (`pnpm conformance:test`). The oracle compiles
   the same candidates and Galeforce must match.

## Mined so far

| Fixture | Source | What it captures |
|---|---|---|
| `dark-mode-class-mode.json` | `dark-mode.test.js`, "should be possible to use the darkMode 'class' mode" | Class-mode dark variant emits `&:is(.dark *)`. |
| `dark-mode-custom-class-name.json` | `dark-mode.test.js`, "should be possible to change the class name" | `darkMode: ['class', '.test-dark']` swaps the selector. |
| `arbitrary-variant-validity.json` | `arbitrary-variants.test.js`, "variants without & or an at-rule are ignored" | **Caught a real Galeforce divergence**: we used to prepend `&` to any arbitrary variant, but Tailwind drops formats that lack `&` and don't start with `@` (per `isValidVariantFormatString` in `setupContextUtils.js`). Fixed in the same commit that landed this fixture. This is exactly why we mine. |

## Naming convention

`upstream/<source-file>-<short-name>.json`

Examples:
- `upstream/dark-mode-class-mode.json` - dark-mode.test.js, "should be possible to use the darkMode class mode"
- `upstream/variants-order-matters.json` - variants.test.js, "order matters and produces different behaviour"

## What NOT to do

- **Don't modify `vendor/tailwindcss-v3/`.** The submodule is
  read-only; verify-tailwind-reference.ts fails CI when it's dirty.
- **Don't blindly snapshot whatever Tailwind emits.** Read the test,
  understand what edge case it captures, and write a description that
  records it. A fixture without context is debt.
- **Don't port tests that depend on features we haven't shipped.**
  Park them with a TODO comment in this README so we pick them up when
  the prerequisite phase lands.
