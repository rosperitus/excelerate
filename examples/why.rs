//! Why a formula disagrees with the result its file cached: follows the cells
//! it reads that disagree too, down to the ones whose inputs all agree and
//! whose own answer does not - the roots worth reading.
//!
//! ```text
//! cargo run --release --example why -- book.xlsx 'Sheet!A1'
//! ```

#![allow(
    clippy::expect_used,
    reason = "a command-line tool: a bad argument ends it"
)]

use excelerate::CellRef;
use excelerate::formula::eval::Engine;
use excelerate::formula::parser::{Expr, parse};
use excelerate::formula::value::Value;
use excelerate::model::{CellValue, Spreadsheet};
use std::collections::HashSet;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(start)) = (args.next(), args.next()) else {
        eprintln!("usage: why <book> <Sheet!A1>");
        std::process::exit(2);
    };
    let book = excelerate::reader::read(&path).expect("the book reads");
    let (sheet, at) = locate(&book, &start).expect("a cell as Sheet!A1");
    let mut engine = Engine::new(&book);
    let mut seen = HashSet::new();
    let mut roots = Vec::new();
    walk(&book, &mut engine, (sheet, at), &mut seen, &mut roots);
    for (sheet, at) in roots.iter().take(20) {
        let cell = book.sheets()[*sheet].get(*at).expect("a formula cell");
        if let CellValue::Formula { formula, cached } = &cell.value {
            println!("root {}!{at}  {formula}", book.sheets()[*sheet].title());
            println!("     file {cached:?}");
            println!("     ours {:?}", engine.cell(*sheet, *at));
        }
    }
    println!("{} roots", roots.len());
}

fn locate(book: &Spreadsheet, text: &str) -> Option<(usize, CellRef)> {
    let (name, cell) = text.rsplit_once('!')?;
    let name = name.trim_matches('\'');
    let sheet = book.sheets().iter().position(|s| s.title() == name)?;
    Some((sheet, CellRef::parse(cell).ok()?))
}

/// Whether a formula cell's computed value differs from its cache.
fn disagrees(book: &Spreadsheet, engine: &mut Engine<'_>, (sheet, at): (usize, CellRef)) -> bool {
    let Some(CellValue::Formula {
        cached: Some(cached),
        ..
    }) = book.sheets()[sheet].get(at).map(|c| &c.value)
    else {
        return false;
    };
    let got = engine.cell(sheet, at);
    !match (&got, cached.as_ref()) {
        (Value::Number(a), CellValue::Number(b)) => (a - b).abs() <= b.abs() * 1e-9 + 1e-9,
        (Value::Text(a), CellValue::Text(b)) => a == b,
        (Value::Bool(a), CellValue::Bool(b)) => a == b,
        (Value::Error(a), CellValue::Error(b)) => a == b,
        (Value::Number(a), CellValue::Text(b)) => *a == 0.0 && b.is_empty(),
        _ => false,
    }
}

fn walk(
    book: &Spreadsheet,
    engine: &mut Engine<'_>,
    here: (usize, CellRef),
    seen: &mut HashSet<(usize, CellRef)>,
    roots: &mut Vec<(usize, CellRef)>,
) {
    if !seen.insert(here) || !disagrees(book, engine, here) {
        return;
    }
    let Some(CellValue::Formula { formula, .. }) =
        book.sheets()[here.0].get(here.1).map(|c| &c.value)
    else {
        return;
    };
    let mut reads = Vec::new();
    if let Ok(expr) = parse(formula) {
        cells_read(book, here.0, &expr, &mut reads);
    }
    let wrong: Vec<_> = reads
        .into_iter()
        .filter(|&c| c != here && disagrees(book, engine, c))
        .collect();
    if wrong.is_empty() {
        roots.push(here);
    }
    for next in wrong {
        walk(book, engine, next, seen, roots);
    }
}

/// The formula cells an expression reads directly, from its references.
fn cells_read(book: &Spreadsheet, own: usize, expr: &Expr, out: &mut Vec<(usize, CellRef)>) {
    match expr {
        Expr::Range { sheet, range, .. } => {
            let index = match sheet {
                None => Some(own),
                Some(name) => book.sheets().iter().position(|s| s.title() == name),
            };
            let Some(index) = index else { return };
            let ws = &book.sheets()[index];
            out.extend(
                ws.iter()
                    .filter(|(at, c)| {
                        range.contains(*at) && matches!(c.value, CellValue::Formula { .. })
                    })
                    .map(|(at, _)| (index, at)),
            );
        }
        Expr::Unary(_, x) => cells_read(book, own, x, out),
        Expr::Binary(_, a, b) => {
            cells_read(book, own, a, out);
            cells_read(book, own, b, out);
        }
        Expr::Call { args, .. } | Expr::Apply { args, .. } => {
            for a in args {
                cells_read(book, own, a, out);
            }
        }
        Expr::Array(rows) => {
            for x in rows.iter().flatten() {
                cells_read(book, own, x, out);
            }
        }
        _ => {}
    }
}
