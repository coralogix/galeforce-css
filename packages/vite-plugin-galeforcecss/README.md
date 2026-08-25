# @coralogix/vite-plugin-galeforcecss

Vite plugin for [GaleforceCSS](https://github.com/coralogix/galeforce-css), a
Rust-powered Tailwind CSS v3-compatible compiler. Drop-in replacement for
`tailwindcss` v3 in a Vite project: same config, same directives, same output,
23x faster builds and sub-millisecond HMR.

> Not an official Tailwind Labs project. GaleforceCSS is an independent port of
> Tailwind CSS v3, pinned to `tailwindcss@3.4.19` as its conformance oracle.

## Install

```bash
npm install -D @coralogix/vite-plugin-galeforcecss
```

## Usage

```js
// vite.config.js
import { defineConfig } from 'vite'
import galeforcecss from '@coralogix/vite-plugin-galeforcecss'

export default defineConfig({
  plugins: [galeforcecss()],
})
```

Your existing `@tailwind` / `@apply` / `theme()` / `@layer` directives are
processed in place by Vite's CSS pipeline. No app-side import, no PostCSS
config required.

```css
/* src/index.css - unchanged */
@tailwind base;
@tailwind components;
@tailwind utilities;
```

## Options

All optional:

```js
galeforcecss({
  config: 'tailwind.config.js',    // auto-discovered if omitted
  content: ['src/**/*.{html,ts}'], // defaults to tailwind.config `content`
  minify: true,                    // defaults to Vite's build.cssMinify
  sourceMap: true,                 // defaults to Vite's build.sourcemap
  targets: { chrome: '95' },       // Lightning CSS browser targets
})
```

## Documentation

- [Vite plugin guide](https://coralogix.github.io/galeforce-css/guide/vite)
- [Migrating from Tailwind v3](https://coralogix.github.io/galeforce-css/guide/migration)
- [Unsupported features](https://coralogix.github.io/galeforce-css/guide/unsupported)

## License

Apache License 2.0. Copyright 2026 Coralogix Ltd.

---

<p align="center">
  Built with 💚 by
  <a href="https://coralogix.com/?utm_source=npm&amp;utm_medium=oss&amp;utm_campaign=galeforcecss">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/coralogix/galeforce-css/master/assets/coralogix-horizontal-white-inline.svg">
      <img src="https://raw.githubusercontent.com/coralogix/galeforce-css/master/assets/coralogix-horizontal-black-inline.svg" alt="Coralogix" height="24" align="middle">
    </picture>
  </a>
</p>
