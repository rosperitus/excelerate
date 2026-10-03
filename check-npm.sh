#!/usr/bin/env bash
# Checks a packed package the way a user gets it: `npm pack` of pkg-publish,
# installed into an empty project, then `require`, `import` and the browser
# build under `/web` (started in Node from the wasm bytes).
#
#   ./check-npm.sh [full|reader]       (default: full; run ./pack-npm.sh first)
#
# npm test runs against npm/pkg, the Node build alone, so this is what catches
# a broken `exports` map or a broken web/ before the registry does.
set -euo pipefail

what="${1:-full}"
root="$(cd "$(dirname "$0")" && pwd)"
case "$what" in
  full)   dir="npm";        name="@rosperitus/excelerate" ;;
  reader) dir="npm-reader"; name="@rosperitus/excelerate-reader" ;;
  *) echo "usage: $0 [full|reader]" >&2; exit 2 ;;
esac
[ -d "$root/$dir/pkg-publish" ] || { echo "no $dir/pkg-publish - run ./pack-npm.sh $what first" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"
tarball="$(npm pack --silent "$root/$dir/pkg-publish")"
echo '{ "private": true }' > package.json
npm install --silent --no-audit --no-fund "./$tarball"

fixture="$root/tests/fixtures/sample.xlsx"

cat > check.cjs <<EOF
const assert = require("node:assert");
const pkg = require("$name");
assert.ok(!require.resolve("$name").includes("/web/"), "require got the browser build");
const book = pkg.Book.read(require("node:fs").readFileSync("$fixture"), "sample.xlsx");
assert.deepStrictEqual(book.sheetNames(), ["Data", "Second"]);
if ("$what" === "full") assert.strictEqual(typeof pkg.Workbook, "function");
EOF

cat > check.mjs <<EOF
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { Book } from "$name";
import init, * as web from "$name/web";

const bytes = readFileSync("$fixture");
assert.deepStrictEqual(Book.read(bytes, "sample.xlsx").sheetNames(), ["Data", "Second"]);

const wasm = createRequire(import.meta.url).resolve("$name/package.json").replace(/package\.json$/, "web/excelerate_bg.wasm");
await init({ module_or_path: readFileSync(wasm) });
assert.deepStrictEqual(web.Book.read(bytes, "sample.xlsx").sheetNames(), ["Data", "Second"]);
if ("$what" === "full") {
  assert.strictEqual(typeof web.Workbook, "function");
  const book = new web.Book();
  book.set(0, "A1", "=6*7");
  book.recalculate();
  assert.strictEqual(book.get(0, "A1"), 42);
}
EOF

node check.cjs
node check.mjs
echo "ok: $name - require, import and /web"
