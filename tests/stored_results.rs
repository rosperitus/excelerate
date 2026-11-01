//! Reading stored formula results from a renderer without recalculating dependencies.

use std::cell::Cell;
use std::rc::Rc;

use excelerate::CellRef;
use excelerate::formula::{CustomFunctions, Engine, Value};
use excelerate::model::{CellValue, Spreadsheet};

#[test]
fn stored_results_avoid_recalculating_cached_dependencies() -> Result<(), Box<dyn std::error::Error>>
{
    let mut book = Spreadsheet::new();
    let a = CellRef::parse("A1")?;
    let b = CellRef::parse("B1")?;
    let sheet = book.sheet_mut(0).ok_or("new workbook must have a sheet")?;
    sheet.entry(a).value = CellValue::Formula {
        formula: "EXPENSIVE()".into(),
        cached: Some(Box::new(CellValue::Number(42.0))),
    };
    sheet.entry(b).value = CellValue::Formula {
        formula: "TWICE(A1)".into(),
        cached: None,
    };
    let calls = Rc::new(Cell::new(0));
    let counted = Rc::clone(&calls);
    let mut functions = CustomFunctions::new();
    functions.register("EXPENSIVE", move |_| {
        counted.set(counted.get() + 1);
        Value::Number(-1.0)
    });
    functions.register("TWICE", |args| match args.first() {
        Some(Value::Number(n)) => Value::Number(n * 2.0),
        _ => Value::Blank,
    });
    let mut engine = Engine::with_functions(&book, &functions).on_stored_results();
    assert_eq!(engine.cell(0, a), Value::Number(42.0));
    assert_eq!(engine.cell(0, b), Value::Number(84.0));
    assert_eq!(
        calls.get(),
        0,
        "cached dependencies must not be recalculated"
    );
    Ok(())
}
