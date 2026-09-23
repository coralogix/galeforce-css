# Changelog

## 0.1.1

### Fixed

- `@coralogix/vite-plugin-galeforcecss`: absolute `content` globs no longer
  depend on the process working directory. When cwd sat inside a glob's
  base, every file under cwd was silently dropped from the scan. The visible
  symptom: in Vitest browser mode (cwd = the library under test), classes
  used only by that library were never generated and tests rendered with
  part of their styling missing. Callers whose cwd is above every pattern,
  such as a dev server run from the repo root, get the same output as
  before.
- The plugin now warns when a content glob covers the Vite root but
  matches no files under it.
- `@coralogix/vite-plugin-galeforcecss`: dev-server HMR now picks up edits
  to files matched only by a `content` glob. Previously a file counted as
  content only if its path started with a content entry, which no glob
  pattern satisfies, so such edits never updated the generated classes.
- `@coralogix/vite-plugin-galeforcecss`: editing the Tailwind config in dev
  now re-scans content, so files newly covered by `content` contribute
  classes and files no longer covered stop contributing, without a server
  restart.
- `@coralogix/galeforcecss-config-loader`: reloading an edited `.js`,
  `.mjs` or `.cjs` config in the same process returned the first version,
  because jiti handed it to Node's native `import()`, which caches for the
  life of the process. The config file is now re-read on every load.
  Modules the config itself requires can still be served from Node's
  cache.
