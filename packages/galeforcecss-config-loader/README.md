# @coralogix/galeforcecss-config-loader

Loads and resolves a `tailwind.config.{js,cjs,mjs,ts}` file into the JSON shape
[GaleforceCSS](https://github.com/coralogix/galeforce-css)'s Rust compiler
consumes, including running any configured Tailwind plugins against a recording
context so their `addUtilities` / `matchVariant` / … calls can be replayed
inside the compiler.

This is an internal building block of `@coralogix/galeforcecss`. You generally
don't need to depend on it directly.

## Install

```bash
npm install -D @coralogix/galeforcecss-config-loader
```

## Usage

```ts
import { loadConfig, findConfigPath } from '@coralogix/galeforcecss-config-loader'

const path = findConfigPath(process.cwd())
const { resolved, pluginNames } = await loadConfig({ path: path ?? undefined })
```

## License

Apache License 2.0. Copyright 2026 Coralogix Ltd.

---

<p align="center">
  Built with 💚 by
  <a href="https://coralogix.com/?utm_source=npm&amp;utm_medium=oss&amp;utm_campaign=galeforcecss">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/coralogix/galeforce-css/master/assets/coralogix-horizontal-white.svg">
      <img src="https://raw.githubusercontent.com/coralogix/galeforce-css/master/assets/coralogix-horizontal-black.svg" alt="Coralogix" height="20" align="middle">
    </picture>
  </a>
</p>
