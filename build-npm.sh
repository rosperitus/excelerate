#!/usr/bin/env bash
# Builds the npm package: wasm plus the TypeScript declarations wasm-bindgen
# derives from the Rust signatures.
#
#   ./build-npm.sh [nodejs|web|bundler]   (default: nodejs)
#
# The result lands in npm/pkg and is ready for `npm publish`.
set -euo pipefail

target="${1:-nodejs}"
root="$(cd "$(dirname "$0")" && pwd)"
# One directory per target: the three builds are not interchangeable, and a
# browser build landing on top of the Node one would break `npm test`.
case "$target" in
  nodejs) out="npm/pkg" ;;
  *) out="npm/pkg-$target" ;;
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
  wasm-pack build --release --target "$target" --out-dir "$out" .

# wasm-pack copies the crate README, which talks about Cargo. npm readers need
# the npm one.
cp npm/README.md "$out/README.md"

echo
echo "package: $out"
ls -la "$out"
