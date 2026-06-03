#!/usr/bin/env node
// Stamp a release version into every publishable package.json and rewrite
// each `workspace:*` dependency to a concrete pin. `npm publish` (unlike
// `pnpm publish`) does not resolve the pnpm `workspace:` protocol, so
// shipping the manifests as-is would produce a broken install for end users.
//
// All publishable packages move in lockstep, so every `workspace:*` ref is
// rewritten to the version being published. Private packages (conformance,
// oracle) are skipped — they are never published.
//
// This runs in the release runner's ephemeral workspace; the edits are
// discarded when the job exits. The in-repo versions stay frozen at 0.0.0
// and the git tag is the authoritative record of what was released.
//
// Usage: node scripts/stamp-versions.mjs <version>
//   e.g. node scripts/stamp-versions.mjs 0.2.0

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, '..');

const version = (process.argv[2] ?? '').replace(/^v/, '').trim();
if (!/^\d+\.\d+\.\d+(?:-[\w.-]+)?(?:\+[\w.-]+)?$/.test(version)) {
  console.error(`stamp-versions: invalid version ${JSON.stringify(process.argv[2])}`);
  process.exit(1);
}

const packagesDir = path.join(repoRoot, 'packages');
const pkgPaths = fs
  .readdirSync(packagesDir, { withFileTypes: true })
  .filter((e) => e.isDirectory())
  .map((e) => path.join(packagesDir, e.name, 'package.json'))
  .filter((p) => fs.existsSync(p));

const updated = [];

for (const pkgPath of pkgPaths) {
  const pkg = JSON.parse(fs.readFileSync(pkgPath, 'utf8'));

  // Private packages (conformance, oracle) are never published — leave them.
  if (pkg.private) continue;

  pkg.version = version;

  // Rewrite every `workspace:*` ref to the concrete version. Any other
  // protocol (e.g. a real semver range) is left untouched so this stays
  // safe if someone later pins a dependency explicitly.
  for (const field of ['dependencies', 'optionalDependencies', 'peerDependencies']) {
    const deps = pkg[field];
    if (!deps) continue;
    for (const [name, spec] of Object.entries(deps)) {
      if (typeof spec === 'string' && spec.startsWith('workspace:')) {
        deps[name] = version;
      }
    }
  }

  fs.writeFileSync(pkgPath, `${JSON.stringify(pkg, null, 2)}\n`, 'utf8');
  updated.push(path.relative(repoRoot, pkgPath));
}

console.log(`stamp-versions: set version ${version} in ${updated.length} package.json file(s):`);
for (const p of updated) console.log(`  - ${p}`);
