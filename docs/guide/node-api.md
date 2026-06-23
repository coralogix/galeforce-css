# Node API

TypeScript-typed API for integrating GaleforceCSS into custom tooling.

```bash
npm install -D @coralogix/galeforcecss
```

## `compile()`

One-shot. Spawns the binary, compiles, exits.

```ts
import { compile } from 'galeforcecss'

const { css, diagnostics } = await compile({
  candidates: ['flex', 'hover:bg-blue-500', 'md:px-4'],
  inputCss: '@tailwind utilities;\n',
  config: { corePlugins: { preflight: false } },
})
```

```ts
interface CompileInput {
  candidates?: string[]   // class names to compile
  inputCss?: string       // CSS with @tailwind directives (default: '@tailwind utilities;')
  config?: object         // resolved Tailwind config (default: Tailwind default)
  content?: string[]      // content roots (alternative to candidates)
}

interface CompileOutput {
  css: string
  diagnostics: Diagnostic[]
}
```

## `createCompileStream()`

Keeps the Rust binary alive between calls. Use this for Vite plugins,
language servers, and any tool that compiles repeatedly.

```ts
import { createCompileStream } from 'galeforcecss'

const stream = createCompileStream()

const a = await stream.compile({ candidates: ['flex'] })
const b = await stream.compile({ candidates: ['block', 'flex'] })

const files = await stream.scanPerFile(['src'])
// { 'src/App.tsx': ['flex', 'text-red-500', …], … }

const merged = await stream.scan(['src/App.tsx'])

stream.close()
```

| Method | Returns |
| --- | --- |
| `compile(input)` | `CompileOutput` |
| `scan(paths)` | merged `string[]` of candidates |
| `scanPerFile(roots)` | `Record<string, string[]>` |
| `close()` | shuts down the background process |

## `loadConfig()` / `findConfigPath()`

```ts
import { loadConfig, findConfigPath } from 'galeforcecss'

const configPath = await findConfigPath()
const { resolved } = await loadConfig({ configPath })
```

`resolved` is the JSON-serialisable config object Galeforce consumes.

## Diagnostics

```ts
const { diagnostics } = await compile({ candidates: ['bogus-[100xp]'] })

for (const d of diagnostics) {
  console.log(d.severity, d.code, d.message, d.candidate)
}
```

| Field | Type | Description |
| --- | --- | --- |
| `severity` | `'error' \| 'warning' \| 'info'` | Severity level. |
| `code` | `string` | Machine-readable code (e.g. `unsupported-candidate`). |
| `message` | `string` | Human-readable description. |
| `candidate` | `string \| undefined` | The triggering candidate, if applicable. |

See [Unsupported features](/guide/unsupported) for the full code list.
