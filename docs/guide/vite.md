# Vite Plugin

`vite-plugin-galeforcecss` integrates GaleforceCSS into Vite's dev server and build pipeline.

## Installation

```bash
npm install -D @coralogix/vite-plugin-galeforcecss
```

## Configuration

```js
// vite.config.js
import { defineConfig } from 'vite'
import galeforcecss from '@coralogix/vite-plugin-galeforcecss'

export default defineConfig({
  plugins: [
    galeforcecss({
      config: 'tailwind.config.js',
      content: ['src'],
    }),
  ],
})
```

### Options

| Option | Type | Default | Description |
| --- | --- | --- | --- |
| `input` | `string` | `'src/index.css'` | CSS entry for virtual-module mode. |
| `config` | `string` | auto | `tailwind.config.{js,cjs,mjs,ts}`. |
| `content` | `string[]` | from config | Scan roots. Re-scanned on change. |
| `minify` | `boolean` | from Vite | Defaults to `build.cssMinify`. |
| `sourceMap` | `boolean` | from Vite | Defaults to `build.sourcemap`. |
| `targets` | `object` | none | Lightning CSS browser targets. |
| `virtualId` | `string` | `'virtual:galeforcecss.css'` | Virtual module name. |

## Drop-in vs virtual-module mode

**Drop-in (default).** Acts as a PostCSS plugin inside Vite's CSS pipeline.
Your existing `@tailwind` directives in any `.css` / `.scss` / `.sass` /
`.less` file are processed transparently. No app-side changes.

**Virtual-module.** Set `input` and import the virtual module in your entry:

```js
import 'virtual:galeforcecss.css'
```

Use this when you want a single plugin-owned stylesheet.

## HMR behaviour

| Change | What happens |
| --- | --- |
| Content file | Single-file scan → candidate diff → selective recompile → reload if CSS changed. |
| `@tailwind` / `@layer` entry | Full recompile → reload if CSS changed. |
| `tailwind.config.js` | Config cache cleared → full recompile → reload if CSS changed. |
| No-op edit | No recompile, no reload. |

## Vite versions

5.x, 6.x, 7.x, 8.x. Uses `server.hot.send` with `server.ws.send` fallback.
