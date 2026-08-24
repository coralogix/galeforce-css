# @coralogix/galeforcecss

Node API for [GaleforceCSS](https://github.com/coralogix/galeforce-css), a
Rust-powered Tailwind CSS v3-compatible compiler.

Most projects want
[`@coralogix/vite-plugin-galeforcecss`](https://www.npmjs.com/package/@coralogix/vite-plugin-galeforcecss)
instead; this package is the programmatic surface underneath it.

> Not an official Tailwind Labs project. GaleforceCSS is an independent port of
> Tailwind CSS v3, pinned to `tailwindcss@3.4.19` as its conformance oracle.

## Install

```bash
npm install -D @coralogix/galeforcecss
```

The platform-specific native binary is pulled in automatically as an optional
dependency. No build step.

## Usage

```ts
import { compile } from '@coralogix/galeforcecss'

const result = await compile({
  candidates: ['flex', 'hover:bg-blue-500', 'md:px-4'],
  inputCss: '@tailwind utilities;\n',
  config: { corePlugins: { preflight: false } },
})

console.log(result.css)
```

For long-running tooling, `createCompileStream()` keeps one compiler process
alive so per-call startup is amortised:

```ts
import { createCompileStream } from '@coralogix/galeforcecss'

const stream = createCompileStream()
const a = await stream.compile({ candidates: ['flex'] })
const b = await stream.compile({ candidates: ['block'] })
stream.close()
```

## Documentation

- [Node API reference](https://coralogix.github.io/galeforce-css/guide/node-api)
- [Configuration](https://coralogix.github.io/galeforce-css/guide/configuration)

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
