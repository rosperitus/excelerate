#!/bin/sh
# Builds the starting inputs of the fuzz targets from the committed fixtures,
# so the fuzzer begins from real files rather than from nothing.
set -eu
cd "$(dirname "$0")"
fixtures=../tests/fixtures
mkdir -p seeds/read_bytes seeds/xls seeds/formula
# read_bytes takes a leading byte that picks the file name; 0 means none.
for f in "$fixtures"/*.xlsx "$fixtures"/*.xls "$fixtures"/sample.slk \
         "$fixtures"/sample.xml "$fixtures"/sample.gnumeric; do
    { printf '\000'; cat "$f"; } > "seeds/read_bytes/$(basename "$f")"
done
cp "$fixtures"/*.xls seeds/xls/
grep -v '^#' "$fixtures/formulas.tsv" | cut -f1 | awk '{ printf "%s", $0 > ("seeds/formula/" NR ".txt") }'
