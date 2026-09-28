//! Formatted text inside a cell, in every format that can hold it.
//!
//! Two fixtures come from other programs: `rich.ods` is what ONLYOFFICE
//! writes for a cell of runs, `rich.xls` what `xlwt` writes, and `xlrd` read
//! the latter back with the same runs this crate reads.

// A failing assertion is this suite's output; panicking is the report.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::CellRef;
use excelerate::model::{CellValue, Spreadsheet, TextRun, Worksheet};
use excelerate::style::{Color, DiffFont, Script, Underline};
use std::io::Cursor;

fn at(a: &str) -> CellRef {
    CellRef::parse(a).unwrap()
}

fn runs_of(book: &Spreadsheet, cell: &str) -> Vec<TextRun> {
    match &book.sheets()[0].get(at(cell)).unwrap().value {
        CellValue::RichText(runs) => runs.clone(),
        other => panic!("{cell} is {other:?}, not formatted text"),
    }
}

fn run(text: &str, font: Option<DiffFont>) -> TextRun {
    TextRun {
        text: text.into(),
        font,
    }
}

/// The runs every format below can carry: a name, a size, the four
/// switches, an explicit colour and a raised run.
fn sample() -> Vec<TextRun> {
    vec![
        run("plain ", None),
        run(
            "bold",
            Some(DiffFont {
                name: Some("Arial".into()),
                size: Some(1400),
                bold: Some(true),
                color: Some(Color::Argb(0xFFFF_0000)),
                ..DiffFont::default()
            }),
        ),
        run(
            " under",
            Some(DiffFont {
                italic: Some(true),
                underline: Some(Underline::Single),
                strike: Some(true),
                ..DiffFont::default()
            }),
        ),
        run(
            "2",
            Some(DiffFont {
                script: Some(Script::Superscript),
                ..DiffFont::default()
            }),
        ),
    ]
}

/// What a format with whole font records makes of a run: the binary ones
/// state every part of the font, not only what changed.
fn same_look(a: &[TextRun], b: &[TextRun]) {
    let texts = |runs: &[TextRun]| runs.iter().map(|r| r.text.clone()).collect::<Vec<_>>();
    assert_eq!(texts(a), texts(b));
    for (a, b) in a.iter().zip(b) {
        let (a, b) = (
            a.font.clone().unwrap_or_default(),
            b.font.clone().unwrap_or_default(),
        );
        let on = |f: Option<bool>| f == Some(true);
        assert_eq!(on(a.bold), on(b.bold), "bold of {a:?}");
        assert_eq!(on(a.italic), on(b.italic), "italic of {a:?}");
        assert_eq!(on(a.strike), on(b.strike), "strike of {a:?}");
        let lined = |u: Option<Underline>| u.is_some_and(|u| u != Underline::None);
        assert_eq!(lined(a.underline), lined(b.underline), "underline of {a:?}");
        let raised = |s: Option<Script>| s == Some(Script::Superscript);
        assert_eq!(raised(a.script), raised(b.script), "script of {a:?}");
        if a.color.is_some() {
            assert_eq!(a.color, b.color);
        }
        if a.size.is_some() {
            assert_eq!(a.size, b.size);
        }
        if a.name.is_some() {
            assert_eq!(a.name, b.name);
        }
    }
}

fn book() -> Spreadsheet {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Rich").unwrap();
    sheet.set(at("A1"), CellValue::RichText(sample()));
    book.add_sheet(sheet).unwrap();
    book
}

#[test]
fn runs_survive_xlsx_ods_xls_and_html() {
    use excelerate::reader::{read_bytes, read_html_str};
    use excelerate::writer::{html, ods, xls, xlsx};
    let book = book();
    let mut bytes = Vec::new();
    xlsx::write_xlsx_to(&book, Cursor::new(&mut bytes)).unwrap();
    assert_eq!(runs_of(&read_bytes(&bytes, None).unwrap(), "A1"), sample());

    for (format, bytes) in [
        ("ods", {
            let mut out = Vec::new();
            ods::write_ods_to(&book, Cursor::new(&mut out)).unwrap();
            out
        }),
        ("xls", {
            let mut out = Vec::new();
            xls::write_xls_to(&book, Cursor::new(&mut out)).unwrap();
            out
        }),
    ] {
        let back = read_bytes(&bytes, None).unwrap_or_else(|e| panic!("{format}: {e}"));
        same_look(&sample(), &runs_of(&back, "A1"));
    }

    let mut page = Vec::new();
    html::write_html_to(&book, &mut page, &html::HtmlOptions::default()).unwrap();
    let back = read_html_str(&String::from_utf8(page).unwrap());
    same_look(&sample(), &runs_of(&back, "A1"));
}

#[test]
fn onlyoffice_spans_read_as_runs() {
    let book = excelerate::reader::read("tests/fixtures/rich.ods").unwrap();
    let runs = runs_of(&book, "A2");
    let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(texts, ["Итого: ", "14pt Arial", " x", "2", " struck"]);
    let fonts: Vec<DiffFont> = runs
        .iter()
        .map(|r| r.font.clone().unwrap_or_default())
        .collect();
    assert_eq!(fonts[0].italic, Some(true));
    assert_eq!(fonts[1].name.as_deref(), Some("Arial"));
    assert_eq!(fonts[1].size, Some(1400));
    assert_eq!(fonts[1].underline, Some(Underline::Single));
    assert_eq!(runs[2].font, None);
    assert_eq!(fonts[3].script, Some(Script::Superscript));
    assert_eq!(fonts[4].strike, Some(true));
    // A cell of one paragraph and no spans is plain text.
    assert_eq!(
        book.sheets()[0].get(at("A3")).unwrap().value,
        CellValue::text("just text")
    );
}

#[test]
fn xlwt_runs_read_as_xlrd_reads_them() {
    let book = excelerate::reader::read("tests/fixtures/rich.xls").unwrap();
    let runs = runs_of(&book, "A1");
    let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(texts, ["plain ", "bold", " red"]);
    // Before the first run the text is in the cell's own font.
    assert_eq!(runs[0].font, None);
    let bold = runs[1].font.clone().unwrap();
    assert_eq!(
        (bold.name.as_deref(), bold.bold),
        (Some("Calibri"), Some(true))
    );
    let red = runs[2].font.clone().unwrap();
    assert_eq!(red.color, Some(Color::Argb(0xFFFF_0000)));
    assert_eq!(red.size, Some(1400));
    assert_eq!(red.underline, Some(Underline::Single));
}

#[test]
fn inline_markup_in_a_page_is_runs_and_spaces_between_them_stay() {
    let book = excelerate::reader::read_html_str(
        "<table><tr><td>a <b>bold</b> and <i>italic</i><sup>2</sup></td>\
         <td><b>all bold</b></td></tr></table>",
    );
    let runs = runs_of(&book, "A1");
    let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(texts, ["a ", "bold", " and ", "italic", "2"]);
    assert_eq!(runs[1].font.clone().unwrap().bold, Some(true));
    assert_eq!(
        runs[4].font.clone().unwrap().script,
        Some(Script::Superscript)
    );
    // One font over the whole text is the cell's font, not a run.
    let whole = book.sheets()[0].get(at("B1")).unwrap();
    assert_eq!(whole.value, CellValue::text("all bold"));
    assert!(book.styles.get(whole.style).unwrap().font.bold);
}
