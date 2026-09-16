// How fast our wasm build opens a document: parsing a buffer into memory and
// nothing else - no recalculation, no writing.
//
//   node bench.js [runs]
const fs = require("node:fs");
const path = require("node:path");
const { Book } = require("excelerate");

const RUNS = Number(process.argv[2] ?? 15);
/// A large file gets fewer runs: it already takes seconds, and every run holds
/// gigabytes in memory.
const runsFor = (size) => (size > 64e6 ? 1 : size > 8e6 ? 3 : RUNS);
/// The unpacking ceiling our readers take: 512 MB by default, a guard against a
/// zip bomb, but these files are known, and a 100 MB package outgrows it.
const MAX_EXPANDED = 8 * 1024 ** 3;
const DIR = path.join(__dirname, "files");

/// The median and the minimum of `runs` runs, in milliseconds. Every run's
/// result is freed: a workbook from wasm lives in its linear memory and does not
/// go away on its own, and the next run needs that memory.
function measure(fn, runs) {
  // Warm-up: the JIT and wasm's own first run. On a large file that alone costs
  // seconds, so once is enough.
  for (let i = 0; i < (runs > 1 ? 3 : 1); i++) fn()?.free?.();
  const times = [];
  for (let i = 0; i < runs; i++) {
    const started = process.hrtime.bigint();
    const made = fn();
    times.push(Number(process.hrtime.bigint() - started) / 1e6);
    made?.free?.();
  }
  times.sort((a, b) => a - b);
  return { median: times[times.length >> 1], min: times[0] };
}

const rows = [];
for (const name of fs.readdirSync(DIR).sort()) {
  const bytes = fs.readFileSync(path.join(DIR, name));
  try {
    const book = Book.read(bytes, name, MAX_EXPANDED);
    const cells = book.cellCount();
    book.free();
    const m = measure(
      () => Book.read(bytes, name, MAX_EXPANDED),
      runsFor(bytes.length),
    );
    rows.push({ name, size: bytes.length, cells, ...m });
  } catch (e) {
    const error = String(e.message ?? e);
    console.error(`  ${name}: ${error}`);
    rows.push({ name, size: bytes.length, error });
  }
}

console.log(`median of ${RUNS} runs (fewer for large files), parsing only\n`);
console.table(
  rows.map((r) => ({
    file: r.name.length > 40 ? `${r.name.slice(0, 37)}...` : r.name,
    KB: (r.size / 1024).toFixed(0),
    cells: r.error ? "-" : r.cells,
    median: r.error ? "error" : `${r.median.toFixed(1)} ms`,
    min: r.error ? "-" : `${r.min.toFixed(1)} ms`,
  })),
);

const ok = rows.filter((r) => !r.error);
console.log(
  `${ok.length} files in all: ${ok.reduce((t, r) => t + r.median, 0).toFixed(1)} ms, ` +
    `${ok.reduce((t, r) => t + r.cells, 0)} cells`,
);
