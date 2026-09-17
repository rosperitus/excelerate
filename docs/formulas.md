# Formulas

The engine parses, evaluates and caches - 443 Excel functions across math,
statistics, distributions, regression, text, dates, financial, lookup, logic,
information, database, engineering and web categories.

## Recalculating a workbook

```rust
use excelerate::formula::eval::recalculate;
use excelerate::progress::Options;
# use excelerate::reader;

# let mut book = reader::read("tests/corpus/test1.xlsx")?;
let computed = recalculate(&mut book, None, &Options::default());        // whole workbook
let one_sheet = recalculate(&mut book, Some(0), &Options::default());    // just this sheet
# let _ = (computed, one_sheet);
# Ok::<(), excelerate::Error>(())
```

The third argument carries a progress callback and any functions of your own
the workbook calls; `&Options::default()` when you want neither. See
[`progress`](https://docs.rs/excelerate/latest/excelerate/progress/) for a
callback that reports formula by formula.

Results land in each formula cell's `cached` field, which is exactly where a
file's own saved results live - so writing the book back gives every other
reader the numbers too.

## Recalculating after an edit

Full recalc on a book with 20k formulas costs ~70 ms. When you are editing,
only what depends on the change needs to move:

```rust
use excelerate::CellRef;
use excelerate::formula::eval::recalculate_from;
# use excelerate::reader;

# let mut book = reader::read("tests/corpus/test1.xlsx")?;
let a1 = CellRef::parse("A1")?;
book.sheet_mut(0).unwrap().set(a1, 99.0);
let touched = recalculate_from(&mut book, &[(0, a1)]);
# let _ = touched;
# Ok::<(), excelerate::Error>(())
```

To recompute a single formula instead - the cell itself, not the cells reading
it - use `recalculate_cell`:

```rust
use excelerate::CellRef;
use excelerate::formula::eval::recalculate_cell;
# use excelerate::reader;

# let mut book = reader::read("tests/corpus/test1.xlsx")?;
let b4 = CellRef::parse("B4")?;
let was_a_formula = recalculate_cell(&mut book, 0, b4);
# let _ = was_a_formula;
# Ok::<(), excelerate::Error>(())
```

Whatever that formula reads is computed on the way, but only inside the
engine's own cache: no other cell of the workbook is written.

Dependencies come out of the formulas themselves - every reference they parse -
and the wave runs to a fixed point. An edit anywhere inside a range counts as
reading that range.

Editing in a loop? Build the dependency index once and keep it:

```rust
use excelerate::CellRef;
use excelerate::formula::eval::Dependencies;
# use excelerate::model::CellValue;
# use excelerate::reader;

# let mut book = reader::read("tests/corpus/test1.xlsx")?;
let mut deps = Dependencies::of(&book);

let a1 = CellRef::parse("A1")?;
book.sheet_mut(0).unwrap().set(a1, 1.0);
deps.recalculate_from(&mut book, &[(0, a1)]);

// Changed a *formula*, not a value? The index has to be told.
let b1 = CellRef::parse("B1")?;
book.sheet_mut(0).unwrap().set(b1, CellValue::Formula {
    formula: "A1*100".into(),
    cached: None,
});
deps.note(&book, 0, b1);
# Ok::<(), excelerate::Error>(())
```

Numbers from a real book with 19,811 formulas: full recalc 635 ms, building the
index 16 ms, 50 edits in one batch -> 184 formulas in 13 ms. The same 50 edits
one at a time: 263 ms. Batch your edits; the pass is per call, not per cell.

## Evaluating an expression yourself

No cells involved, no book mutated:

```rust
use excelerate::formula::{Value, eval::{Engine, Origin}};
use excelerate::model::Spreadsheet;

let book = Spreadsheet::new();
let mut engine = Engine::new(&book);

let answer = engine.eval(Origin::new(0, excelerate::CellRef::parse("A1")?), "ROUND(2/3, 4)");
assert_eq!(answer, Value::Number(0.6667));
# Ok::<(), excelerate::Error>(())
```

`Origin` says which cell the formula is speaking from - relative references and
`ROW()`/`COLUMN()` need it. `Engine` caches within its lifetime, so evaluating
a thousand expressions against the same book reuses everything it already
computed.

## Parsing without evaluating

```rust
use excelerate::formula::{Expr, parse};

let expr = parse("SUM(Sheet2!A1:A9)*2")?;
match expr {
    Expr::Binary(..) => {}
    _ => unreachable!(),
}
# Ok::<(), excelerate::Error>(())
```

The parser handles literals, references, ranges, calls, array constants and all
three reference operators - intersection (a space), union (`,`) and the span
operator (`:` between areas, so `A1:A2:B1` is `A1:B2`). Useful for linting
formulas, finding every cell a sheet reads, or rewriting references.

## Values

```rust
pub enum Value {
    Blank,
    Number(f64),
    Text(String),
    Bool(bool),
    Error(CellError),
    Array(Rc<Vec<Vec<Value>>>),
    Lambda(Rc<Lambda>),
}
```

Excel's own coercion rules apply throughout, including the comparison order
that trips people up: numbers < text < `FALSE` < `TRUE`. A formula that returns
an array shows its top-left value in a cell - there is no spill range here, and
one cell is one cell. A reference to that cell reads the value it shows;
`A1#` (`ANCHORARRAY`) reads the array behind it.

An array is shared, not owned: a range is read once and handed to every
formula that asks for it, so `INDEX(Data, ROW(), 1)` down forty thousand rows
does not copy `Data` forty thousand times. Make one with `Value::array(rows)`;
take the rows out with `Rc::unwrap_or_clone`.

## About that cached result

A formula cell carries the result whoever saved the file computed. That cache
is worth exactly as much as the engine that wrote it. A quick tell: if an xlsx
package has no `xl/calcChain.xml`, Excel was not the last thing to save it
(Excel always writes that part for a book with formulas), and the cached values
may be stale or plain wrong.

So: trust it for display, recalculate before you rely on it.

## Named ranges

Defined names resolve during evaluation. A sheet-scoped name beats a
workbook-scoped one of the same name - that is how two sheets can give one name
two meanings - and a name that refers to itself yields `#REF!` instead of
looping. `INDIRECT("Sales")` follows a name that stands for a reference; one
that stands for a constant is `#REF!`, as in Excel.

## Errors

Errors propagate the way they do in Excel: anything touching `#DIV/0!` becomes
`#DIV/0!`. A formula that fails to parse evaluates to `#NAME?`, which is also
what Excel shows for a function it does not know.

There is no panic path here. A formula 200 levels deep, a circular reference, a
range covering a million cells - each has a defined answer (`#REF!` for a
cycle, `#VALUE!` for an oversized range) rather than a stack overflow.
