# Getting Started

GaleforceCSS reads your existing `tailwind.config.{js,ts}` and produces the
same CSS output as Tailwind v3 — 23x faster.

## Prerequisites

- Node 18.18+
- A Vite project using Tailwind v3 (or starting fresh)

## Installation

```bash
npm install -D @coralogix/vite-plugin-galeforcecss
```

Platform binaries ship in the package. No Rust toolchain required.

## Vite setup

The plugin registers itself as a PostCSS plugin inside Vite's CSS
pipeline. Your existing `@tailwind` / `@apply` / `theme()` / `@layer`
directives in any `.css` / `.scss` / `.sass` / `.less` file work
unchanged:

```js
// vite.config.js
import { defineConfig } from 'vite'
import galeforcecss from '@coralogix/vite-plugin-galeforcecss'

export default defineConfig({
  plugins: [galeforcecss()],
})
```

```css
/* src/index.css — unchanged */
@tailwind base;
@tailwind components;
@tailwind utilities;
```

`@apply` in component-scoped styles also works:

```scss
.btn {
  @apply px-4 py-2 rounded-lg;
}
```

## Migrating from Tailwind's PostCSS plugin

1. Remove `tailwindcss` from your PostCSS config:

   ```js
   // postcss.config.js
   export default {
     plugins: {
       // tailwindcss: {},   ← remove
       autoprefixer: {},
     },
   }
   ```

2. Add `vite-plugin-galeforcecss` to `vite.config.js` (above).

3. Uninstall `tailwindcss`:

   ```bash
   npm uninstall tailwindcss
   ```

Your `tailwind.config.{js,ts}` and CSS files are untouched. Output is
semantically identical (verified on 894 fixtures + 7 real projects).

## Plugin options

All optional:

```js
galeforcecss({
  config: 'tailwind.config.js',          // auto-discovered if omitted
  content: ['src/**/*.{html,ts}'],       // defaults to tailwind.config `content`
  minify: true,                          // defaults to Vite's build.cssMinify
  sourceMap: true,                       // defaults to Vite's build.sourcemap
  targets: { chrome: '95' },             // Lightning CSS browser targets
})
```

## Optional: virtual-module mode

For a single plugin-owned stylesheet instead of inline directive expansion:

```js
galeforcecss({ input: 'src/index.css' })
```

```js
// src/main.ts
import 'virtual:galeforcecss.css'
```

Most projects don't need this — the drop-in PostCSS mode covers what Tailwind v3 did.

## Next

- [Vite plugin options →](/guide/vite)
- [CLI reference →](/guide/cli)
- [Node API →](/guide/node-api)
- [Configuration →](/guide/configuration)
