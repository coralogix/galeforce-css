---
layout: home

hero:
  name: GaleforceCSS
  text: Tailwind v3 at Rust speed
  tagline: Drop-in replacement for Tailwind v3 — same config, same output, 23x faster builds.
  actions:
    - theme: brand
      text: Get Started
      link: /guide/getting-started
    - theme: alt
      text: View on GitHub
      link: https://github.com/coralogix/galeforce-css

features:
  - title: 23x faster builds
    details: 10 ms vs 246 ms on a 2,551-candidate real project. Pure compute hits 1.2 ms — 0.4 µs per candidate.
  - title: Verified Tailwind v3 parity
    details: 894 conformance fixtures pass against the live Tailwind oracle; 186 byte-sensitive snapshots confirm formatting matches.
  - title: Incremental HMR
    details: Per-file candidate cache. Content-file edits trigger a single-file scan and selective recompile; no-op edits produce zero traffic.
  - title: Full Tailwind v3 surface
    details: All utilities, all variants, @tailwind / @layer / @apply / theme() / @screen, plugins (addUtilities, matchUtilities, addVariant, …), arbitrary values, opacity modifiers.
---

## Benchmarks

Real-world project: **horizon-tailwind-react** (2,551 candidates, 706 rules)

| Scenario | Tailwind v3 | GaleforceCSS | Speedup |
| --- | --- | --- | --- |
| CLI build (cold) | 246 ms | 10 ms | **23x** |
| Warm stream | 15 ms | 2 ms | **7.5x** |
| Pure compute | — | 1.2 ms | — |

Synthetic smoke project (734 candidates): **195 ms → 5 ms (38x)**.

## Quick start

```bash
npm install -D @coralogix/vite-plugin-galeforcecss
```

```js
// vite.config.js
import { defineConfig } from 'vite'
import galeforcecss from '@coralogix/vite-plugin-galeforcecss'

export default defineConfig({
  plugins: [galeforcecss()],
})
```

[Full guide →](/guide/getting-started)

## Status

**Alpha.** Zero semantic diff against the official Tailwind v3 oracle on
seven real-world projects, 894 conformance fixtures, and 186 byte-sensitive
snapshots. APIs are stable for the documented surface but may change before 1.0.

> GaleforceCSS is **not** an official Tailwind Labs project.
