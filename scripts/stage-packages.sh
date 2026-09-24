#!/usr/bin/env bash
# Stage build-binaries.yml artifacts into the publishable npm packages.
# Expects `binary-<pkg>/` directories from actions/download-artifact
# (merge-multiple: false) in the repo root. Shared by release.yml and
# pkg-pr-new.yml.
set -euo pipefail

# Copy each binary + napi addon into its npm package directory and restore
# the executable bit (upload-artifact@v4 strips it, so the launcher would
# otherwise hit EACCES spawning the CLI).
for pkg in darwin-arm64 darwin-x64 linux-x64-gnu linux-arm64-gnu; do
  cp binary-galeforcecss-${pkg}/galeforcecss      packages/galeforcecss-${pkg}/
  cp binary-galeforcecss-${pkg}/galeforcecss.node packages/galeforcecss-${pkg}/
  chmod 0755 packages/galeforcecss-${pkg}/galeforcecss
done
cp binary-galeforcecss-win32-x64-msvc/galeforcecss.exe  packages/galeforcecss-win32-x64-msvc/
cp binary-galeforcecss-win32-x64-msvc/galeforcecss.node packages/galeforcecss-win32-x64-msvc/

# Apache 2.0 section 4(a) requires a copy of the license to travel with
# every distributed copy of the work. npm always includes a LICENSE file in
# the tarball regardless of the manifest's `files` list, so staging one next
# to each publishable manifest is enough.
for pkg in galeforcecss-darwin-arm64 galeforcecss-darwin-x64 \
           galeforcecss-linux-x64-gnu galeforcecss-linux-arm64-gnu \
           galeforcecss-win32-x64-msvc galeforcecss-config-loader \
           galeforcecss vite-plugin-galeforcecss; do
  cp LICENSE "packages/$pkg/LICENSE"
done
