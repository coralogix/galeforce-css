# GaleforceCSS basic CLI example

A single HTML page styled by running the `galeforcecss` CLI directly.
No bundler, no plugin — just a CSS build step.

## Run

```bash
pnpm install
pnpm build
```

That writes `dist/output.css`. Open `index.html` in a browser; it
loads `dist/output.css` via a relative `<link>`.

For live editing:

```bash
pnpm watch
```

## Files

- `src/index.css` — the standard `@tailwind base; @tailwind components;
  @tailwind utilities;` entry.
- `tailwind.config.js` — points `content` at `./*.html` so the CLI
  scans `index.html` for candidate class names.
- `index.html` — the markup.
- `dist/output.css` — the compiled CSS (regenerated on each build).
