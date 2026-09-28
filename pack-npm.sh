#!/usr/bin/env bash
# Prepares a built package for publishing, under the scope it lives in.
#
#   ./pack-npm.sh [full|reader]        (default: full)
#
# The build scripts name the package `excelerate` and `excelerate-reader`,
# because the tests and examples beside them import it by that name. The
# registry has those names taken by somebody else, so what is published is
# `@rosperitus/excelerate` and `@rosperitus/excelerate-reader`. This copies the
# built package, renames it, and rewrites the name in its README, leaving the
# original where `npm test` can still find it.
set -euo pipefail

what="${1:-full}"
root="$(cd "$(dirname "$0")" && pwd)"
case "$what" in
  full)   dir="npm";        name="excelerate" ;;
  reader) dir="npm-reader"; name="excelerate-reader" ;;
  *) echo "usage: $0 [full|reader]" >&2; exit 2 ;;
esac

cd "$root"
src="$dir/pkg"
out="$dir/pkg-publish"
[ -d "$src" ] || { echo "no $src - run ./build-$dir.sh first" >&2; exit 1; }

rm -rf "$out"
cp -r "$src" "$out"

scoped="@rosperitus/$name"
# The name, and the access a scoped package needs to be public rather than
# private, which is what npm assumes for a scope.
python3 - "$out/package.json" "$scoped" <<'PY'
import json, sys
path, scoped = sys.argv[1], sys.argv[2]
with open(path, encoding="utf-8") as f:
    manifest = json.load(f)
manifest["name"] = scoped
manifest["publishConfig"] = {"access": "public"}
with open(path, "w", encoding="utf-8") as f:
    json.dump(manifest, f, indent=2, ensure_ascii=False)
    f.write("\n")
PY

# Every place the README tells the reader what to install or import.
sed -i "s|npm install $name\b|npm install $scoped|g; s|\"$name\"|\"$scoped\"|g" "$out/README.md"

version="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$out/package.json")"
echo
echo "ready: $out   $scoped@$version"
echo "publish with:"
echo "  npm publish $out --otp=<code>"
