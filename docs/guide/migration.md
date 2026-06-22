# Migrating from Tailwind CSS v3

Drop-in replacement for Tailwind v3. Same config, same CSS output
(after PostCSS normalization). Swap the CLI/plugin and you're done.

## With Vite

```bash
pnpm remove tailwindcss @tailwindcss/vite
pnpm add -D @coralogix/galeforcecss @coralogix/vite-plugin-galeforcecss
```

```ts
// vite.config.ts
import { defineConfig } from 'vite'
import galeforce from '@coralogix/vite-plugin-galeforcecss'

export default defineConfig({
  plugins: [
    galeforce({
      content: ['src/**/*.{html,js,jsx,ts,tsx,vue,svelte}'],
    }),
  ],
})
```

```css
/* src/index.css — unchanged */
@tailwind base;
@tailwind components;
@tailwind utilities;
```

Your `tailwind.config.{js,ts}` is read as-is. Tailwind's own
`resolveConfig` does the merge, so default theme + `theme.extend`
behave bit-for-bit identically.

## With PostCSS / Tailwind CLI

Flags map 1:1:

```bash
# Before
npx tailwindcss -i src/index.css -o dist/output.css --content 'src/**/*.{html,jsx,tsx}'

# After
npx galeforcecss build -i src/index.css -o dist/output.css --content src
```

`--input` / `-i`, `--output` / `-o`, `--config`, `--content` all
unchanged. `--watch` becomes `galeforcecss watch`.

## What works on day one

- All built-in utilities (margin, padding, sizing, colors, typography, flex, grid, backgrounds, borders, shadows, filters, transforms, interactivity, SVG).
- All variants (pseudo-class, pseudo-element, group/peer, named groups, arbitrary `[&>*]`, aria/data, supports, dark mode in all three forms, responsive, container).
- `@tailwind base|components|utilities`, `@layer`, `@apply`, `theme()`, `@screen`, `screen()`.
- Plugins authored against the common surface (`addUtilities`, `matchUtilities`, `addVariant`, …).
- Arbitrary values, opacity modifiers, prefix, important, dark mode.

## Validate the swap

Run with `--diagnostics diagnostics.json`. Empty array = clean migration.
For high-confidence validation, diff the CSS output:

```bash
pnpm --filter @coralogix/galeforcecss-conformance exec tsx scripts/galeforcecss-compare.ts --project /path/to/your/app
```

## When to stay on Tailwind

- You're on Tailwind v4. GaleforceCSS is a v3 port and won't follow v4.
- A third-party plugin uses PostCSS container semantics beyond `walkRules` + `modifySelectors`. Try a build first — most plugins work.
- You need source maps. Not yet generated; see [Unsupported features](./unsupported.md).
