# CLI

The `galeforcecss` binary ships with the npm package. One-shot builds, file watcher, class scanner, project scaffolder.

## Installation

```bash
# Recommended — platform binary included
npm install -D @cx/galeforcecss

# Or from source
cargo install --path crates/galeforce-cli
```

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
| `--config` | auto | `tailwind.config.{js,ts}`. |
| `--verbose` | off | Diagnostics + timing to stderr. |
| `--diagnostics <path>` | off | JSON file or `-` for stderr. |

## `watch`

Watch input, config, and content roots; recompile on change. Same flags as `build`. 100 ms debounce.

```bash
galeforcecss watch -i src/index.css -o dist/output.css --content src
```

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
