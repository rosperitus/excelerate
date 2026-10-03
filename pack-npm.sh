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
#
# The package carries two builds: the Node one at its root and the browser one
# (`./build-$dir.sh web`) under web/. `exports` picks the first under the
# `node` condition and the second everywhere else - browsers and bundlers -
# so `import { Book } from "@rosperitus/excelerate"` works in both; the
# browser only has to `await init()` first.
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
web="$dir/pkg-web"
out="$dir/pkg-publish"
[ -d "$src" ] || { echo "no $src - run ./build-$dir.sh first" >&2; exit 1; }
[ -d "$web" ] || { echo "no $web - run ./build-$dir.sh web first" >&2; exit 1; }
for build in "$src" "$web"; do
  v="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$build/package.json")"
  versions="${versions:-} $v"
done
[ "$(echo $versions | tr ' ' '\n' | sort -u | wc -l)" = 1 ] \
  || { echo "the builds disagree on the version:$versions - rebuild both" >&2; exit 1; }

rm -rf "$out"
cp -r "$src" "$out"
mkdir "$out/web"
cp "$web"/*.js "$web"/*.d.ts "$web"/*.wasm "$out/web/"

scoped="@rosperitus/$name"
# The name, and the access a scoped package needs to be public rather than
# private, which is what npm assumes for a scope.
python3 - "$out/package.json" "$scoped" "$web/package.json" "$out/web/package.json" <<'PY'
import json, sys
path, scoped, web_path, web_out = sys.argv[1:]
with open(path, encoding="utf-8") as f:
    manifest = json.load(f)
with open(web_path, encoding="utf-8") as f:
    web = json.load(f)
manifest["name"] = scoped
manifest["publishConfig"] = {"access": "public"}
manifest["files"] = sorted(set(manifest["files"]) | {"web"})
node = {"types": "./" + manifest["types"], "default": "./" + manifest["main"]}
browser = {"types": "./web/" + web["types"], "default": "./web/" + web["main"]}
manifest["exports"] = {
    ".": {"node": node, "default": browser},
    "./web": browser,
    "./package.json": "./package.json",
}
# The browser build is an ES module; this tells Node and bundlers so for web/.
with open(web_out, "w", encoding="utf-8") as f:
    json.dump({"type": "module", "sideEffects": False}, f, indent=2)
    f.write("\n")
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
