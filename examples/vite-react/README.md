# GaleforceCSS + Vite + React example

Minimal Vite + React app driven by GaleforceCSS instead of Tailwind.

## Run

```bash
pnpm install
pnpm dev
```

## What's wired

- `vite.config.ts` — the `vite-plugin-galeforcecss` plugin, configured with
  `input: 'src/index.css'`, `config: 'tailwind.config.js'`, and the
  React/TSX content globs.
- `src/index.css` — the standard `@tailwind base; @tailwind components;
  @tailwind utilities;` entry.
- `src/main.tsx` — imports `virtual:galeforcecss.css`. The Vite plugin
  resolves this virtual module to the compiled CSS.
- `src/App.tsx` — a small component exercising responsive, dark mode,
  hover, active, and the custom `brand` theme palette defined in
  `tailwind.config.js`.

## Comparing against Tailwind

Run the build, then swap `vite-plugin-galeforcecss` for `@tailwindcss/vite`
and re-run. The CSS output should be byte-for-byte equivalent after
PostCSS normalization — that's the contract GaleforceCSS upholds.
