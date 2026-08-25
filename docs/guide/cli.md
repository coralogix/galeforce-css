# CLI

The `galeforcecss` binary ships with the npm package. One-shot builds, file watcher, class scanner, project scaffolder.

## Installation

```bash
# Recommended - platform binary included
npm install -D @coralogix/galeforcecss

# Or from source
cargo install --path crates/galeforce-cli
```

Installing the package puts `galeforcecss` on your project's `PATH` for npm
scripts; outside a script, run it with `npx galeforcecss <command>`.

## `build`

Compile once and write to a file.

```bash
galeforcecss build \
  --input  src/index.css \
  --output dist/output.css \
  --content src
```

| Flag | Default | Description |
| --- | --- | --- |
| `--input` / `-i` | `src/index.css` | CSS entry. |
| `--output` / `-o` | stdout | Output path. |
| `--content` | `.` | Content root (repeatable). |
| `--config` | none | Resolved-config **JSON** file. See the note below. |
| `--verbose` | off | Diagnostics + timing to stderr. |
| `--diagnostics <path>` | off | JSON file or `-` for stderr. |
| `--minify` | off | Minify the output CSS. |

::: warning `--config` does not take a `tailwind.config.js`
The binary reads a *resolved-config JSON* file, and does not discover or
evaluate a JS config: evaluating one means running Tailwind's `resolveConfig`
and your plugins in a JS runtime, which lives in the Node packages rather than
in the Rust binary. Point `--config` at a `tailwind.config.js` and it fails with
`parsing config …: expected value at line 1 column 1`. Omit it and the build
uses the **default Tailwind theme**, silently ignoring your project's colours,
spacing, fonts and plugins.

Until this is wired up, produce the JSON yourself:

```js
// resolve-config.mjs
import { writeFileSync } from 'node:fs'
import { loadConfig } from '@coralogix/galeforcecss'

const { resolved } = await loadConfig()
writeFileSync('galeforce.config.json', JSON.stringify(resolved))
```

```bash
node resolve-config.mjs
galeforcecss build -i src/index.css -o dist/output.css --content src \
  --config galeforce.config.json
```

The [Vite plugin](./vite) and the [Node API](./node-api) resolve
`tailwind.config.{js,cjs,mjs,ts}` for you; only the standalone CLI has this
limitation.
:::

## `watch`

Watch input, config, and content roots; recompile on change. Same flags as `build`. 200 ms debounce.

```bash
galeforcecss watch -i src/index.css -o dist/output.css --content src
```

::: warning Keep `--output` outside every `--content` root
Each rebuild writes `--output`, and a write inside a watched root counts as a
change, so the watcher rebuilds continuously. The example above is safe because
`dist/` is not under `src/`. The default content root is `.`, which *does*
contain the output, so `galeforcecss watch -i in.css -o out.css` loops. Pass
`--content` explicitly.
:::

## `scan`

Walk content roots and print candidates.

```bash
galeforcecss scan --content src           # human-readable
galeforcecss scan --content src --json    # JSON array
```

## `init`

Scaffold `tailwind.config.js` + `src/index.css` in a new project.

```bash
galeforcecss init
galeforcecss init --force                       # overwrite existing
galeforcecss init --input styles/global.css     # custom CSS output path
```

## `list-classes` / `list-variants`

Print every known utility class or variant. Useful for editor integrations.

```bash
galeforcecss list-classes
galeforcecss list-variants
```

## `compile-stream` (advanced)

Long-running mode used internally by the Vite plugin and Node API.
JSONL on stdin/stdout. Documented in `packages/galeforcecss/src/index.ts`
for tooling authors.
