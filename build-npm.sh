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

# The object layer (npm/classes): one source with no imports or exports, wrapped
# here as CommonJS for Node and as an ES module for the browser and bundlers.
# The package then points at index.js, which is the generated module plus
# Workbook, Sheet, Table and SheetRange.
python3 - "$out" "$target" <<'PY'
import json, pathlib, sys
out, target = pathlib.Path(sys.argv[1]), sys.argv[2]
body = pathlib.Path("npm/classes/classes.js").read_text(encoding="utf-8")
types = pathlib.Path("npm/classes/classes.d.ts").read_text(encoding="utf-8")
names = "Workbook, Sheet, Table, SheetRange"
default = 'export { default } from "./excelerate.js";\n' if target == "web" else ""
if target == "nodejs":
    # Spelled so Node's static scan of CommonJS finds every name: an ES
    # module importing the package sees the re-export and each assignment.
    js = ('"use strict";\nmodule.exports = require("./excelerate.js");\n\n' + body
          + "\nconst classes = build(module.exports.Book);\n"
          + "".join(f"module.exports.{n} = classes.{n};\n" for n in names.split(", ")))
else:
    js = ('import * as core from "./excelerate.js";\nexport * from "./excelerate.js";\n' + default
          + "\n" + body + f"\nexport const {{ {names} }} = build(core.Book);\n")
dts = ('export * from "./excelerate";\n' + default.replace(".js", "")
       + 'import type { Book, CellValue, CellGrid, CellStylePatch, SheetTable, SortKeys, TableFilter, CsvOptions }'
       + ' from "./excelerate";\n\n' + types)
(out / "index.js").write_text(js, encoding="utf-8")
(out / "index.d.ts").write_text(dts, encoding="utf-8")
manifest_path = out / "package.json"
manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
for key, value in (("main", "index.js"), ("module", "index.js"), ("types", "index.d.ts")):
    if key in manifest:
        manifest[key] = value
manifest["files"] = sorted(set(manifest.get("files", [])) | {"index.js", "index.d.ts"})
manifest_path.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
PY

echo
echo "package: $out"
ls -la "$out"
