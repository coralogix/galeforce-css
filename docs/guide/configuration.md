# Configuration

GaleforceCSS reads your existing `tailwind.config.{js,cjs,mjs,ts}` unchanged. No migration required.

```js
/** @type {import('tailwindcss').Config} */
export default {
  content: ['./src/**/*.{html,js,jsx,ts,tsx,vue,svelte}'],
  theme: {
    extend: {
      colors: { brand: '#3b82f6' },
      fontFamily: { sans: ['Inter', 'sans-serif'] },
    },
  },
  plugins: [require('@tailwindcss/forms')],
  darkMode: 'class',
  prefix: '',
}
```

## Supported fields

| Field | Status |
| --- | --- |
| `content` | Used by the scanner when no explicit `content` option is passed to the plugin/CLI. |
| `theme` / `theme.extend` | Merged + resolved; `theme()` calls resolved at compile time. |
| `plugins` | `addUtilities`, `addComponents`, `addBase`, `addVariant`, `matchUtilities`, `matchComponents`, `matchVariant`. |
| `darkMode` | `'media'`, `'class'`, `'selector'`. |
| `prefix` | Full — `prefix: 'tw-'` prefixes every utility (including `@apply` targets). |
| `corePlugins` | Object and array forms. `preflight: false` skips the preflight reset. |
| `separator` | Custom variant separator (default `:`). |

## Plugins

Each plugin runs through a JS-side recording context that captures
`addUtilities`, `matchUtilities`, `addVariant`, etc., and feeds the
calls into the Rust compiler. Standard plugins (`@tailwindcss/forms`,
`@tailwindcss/typography`, `@tailwindcss/aspect-ratio`) work unchanged.

Plugins that manipulate the PostCSS AST beyond the shim's surface emit
a warning in diagnostics and produce no output.

## TypeScript configs

`.ts` configs load via [jiti](https://github.com/unjs/jiti). No setup needed.

## `theme()` and `@apply`

```css
.my-element {
  color: theme('colors.blue.500');
  font-size: theme('fontSize.lg');
}

.btn {
  @apply px-4 py-2 rounded bg-blue-600 text-white;
  @apply hover:bg-blue-700;
  @apply focus:outline-none focus:ring-2;
}
```

`@apply` honors variants and the configured `prefix`. The opacity
shorthand (`theme('colors.red.500/50')`) is not yet supported — use a
separate `bg-opacity` utility.

## Known limitations

- Third-party plugins that manipulate the PostCSS AST beyond `walkRules` / `modifySelectors`.
- `theme()` opacity shorthand.
- Object-form screen configs (`{ md: { min: '768px', max: '1023px' } }`).
- Nested `@layer` inside `@media` / `@supports`.

See [Unsupported features](/guide/unsupported) for the full list.

## Vite plugin options

```ts
galeforcecss({
  input?: string        // CSS entry. Default: 'src/index.css'
  config?: string       // tailwind.config path. Default: auto-discovered
  content?: string[]    // Content roots. Default: from tailwind.config `content`
  minify?: boolean      // Default: Vite's build.cssMinify
  sourceMap?: boolean   // Default: Vite's build.sourcemap
  targets?: object      // Lightning CSS browser targets
  virtualId?: string    // Default: 'virtual:galeforcecss.css'
})
```

## CLI flags (`build` / `watch`)

| Flag | Default | Description |
| --- | --- | --- |
| `--input` / `-i` | `src/index.css` | CSS entry containing `@tailwind` directives. |
| `--output` / `-o` | stdout | Output path. |
| `--config` | auto | `tailwind.config.{js,cjs,mjs,ts}` path. |
| `--content` | `.` | Content root (repeatable). |
| `--verbose` | off | Print diagnostics + timing to stderr. |
| `--diagnostics <path>` | off | Write diagnostics JSON. Use `-` for stderr. |

## Node API — `CompileInput`

```ts
{
  candidates?: string[]   // Class names to compile directly
  inputCss?: string       // CSS with @tailwind directives
  config?: object         // Resolved Tailwind config
  content?: string[]      // Content roots (alternative to candidates)
}
```

## Environment variables

| Variable | Description |
| --- | --- |
| `GALEFORCE_CLI_BIN` | Override the path to the `galeforcecss` binary. |
| `GALEFORCE_DEBUG` | Set to `1` for scan/compile timing on every build. |
| `GALEFORCE_LOG` | Set to `debug` for verbose compiler output. |
