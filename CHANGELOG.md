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
