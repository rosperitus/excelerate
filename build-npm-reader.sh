#!/usr/bin/env bash
# Builds the reader-only npm package: reading files and their styles, with no
# writers and no formula engine compiled in.
#
#   ./build-npm-reader.sh [nodejs|web|bundler]   (default: nodejs)
#
# The result lands in npm-reader/pkg and is ready for `npm publish`.
set -euo pipefail

target="${1:-nodejs}"
root="$(cd "$(dirname "$0")" && pwd)"
case "$target" in
  nodejs) out="npm-reader/pkg" ;;
  *) out="npm-reader/pkg-$target" ;;
esac

cd "$root"
rm -rf "$out"
# opt-lto-release: whole-program LTO in one unit takes 7 % off the wasm
# (3.94 -> 3.65 MB, 6 % gzipped). Set here and not in Cargo.toml: a
# library's release profile never reaches the crates that depend on it,
# and the package is the one artefact of this repository people download.
# The remap keeps the home directory out of the panic messages compiled into
# the wasm: without it every dependency path starts with /home/<user>/.
RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=~" \
  CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
  wasm-pack build --release --target "$target" --out-dir "$out" \
  -- --no-default-features

# wasm-pack takes the name from Cargo.toml, and both packages come from the
# same crate; this one is published under its own name.
node -e '
  const fs = require("fs");
  const path = "'"$out"'/package.json";
  const pkg = JSON.parse(fs.readFileSync(path, "utf8"));
  pkg.name = "excelerate-reader";
  pkg.description =
    "Read spreadsheets in the browser or Node: xlsx, xls, ods, csv, html, " +
    "SYLK, Gnumeric and SpreadsheetML, with cell styles. Reading only.";
  fs.writeFileSync(path, JSON.stringify(pkg, null, 2) + "\n");
'

cp npm-reader/README.md "$out/README.md"

echo
echo "package: $out"
ls -la "$out"
