//! Builds a workbook from nothing: styles, number formats, column widths,
//! merged heading, formulas, a table, a comment, a hyperlink and a locked
//! sheet, then recalculates and writes it out.
//!
//! What a report generator does, end to end and in one file.
//!
//! `cargo run --release --example build -- report.xlsx`

// A developer tool: a failed step should stop it with a readable message.
#![allow(clippy::expect_used, clippy::print_stdout, clippy::too_many_lines)]

use excelerate::formula::eval::recalculate;
use excelerate::model::protection::PasswordHash;
use excelerate::model::table::{Table, TableColumn};
use excelerate::model::{
    ColumnRun, Comment, Hyperlink, LinkTarget, Spreadsheet, TextRun, Worksheet,
};
use excelerate::progress::Options;
use excelerate::style::{Color, Fill, NumberFormat, Pattern, Style};
use excelerate::{CellRef, Range};

/// `A1` or the program has a typo in it.
fn at(a1: &str) -> CellRef {
    CellRef::parse(a1).expect("literal address")
}

fn main() {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "report.xlsx".to_owned());

    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Продажи").expect("valid sheet name");

    // Styles are interned once and referred to by id: a million cells sharing
    // one look cost one `Style`.
    let mut heading = Style::default();
    heading.font.bold = true;
    heading.font.set_size_points(14.0);
    heading.font.color = Color::Argb(0xFFFF_FFFF);
    heading.fill = Fill {
        pattern: Pattern::Solid,
        foreground: Color::Argb(0xFF19_4D33),
        background: Color::Auto,
    };
    let heading = book.styles.intern(heading);

    let money = book.styles.intern(Style {
        number_format: NumberFormat::Custom("#,##0.00 ₽".into()),
        ..Style::default()
    });

    let mut header = Style::default();
    header.font.bold = true;
    let header = book.styles.intern(header);

    // A heading across the table, merged.
    sheet.set(at("A1"), "Отчёт за квартал");
    sheet.entry(at("A1")).style = heading;
    sheet.merges.push(Range::new(at("A1"), at("C1")));

    for (letter, name) in [("A", "Товар"), ("B", "Штук"), ("C", "Сумма")] {
        let cell = at(&format!("{letter}2"));
        sheet.set(cell, name);
        sheet.entry(cell).style = header;
    }

    let rows = [
        ("Чай", 42.0, 180.0),
        ("Кофе", 128.0, 350.0),
        ("Какао", 7.0, 420.0),
    ];
    for (index, (name, count, price)) in rows.iter().enumerate() {
        let row = 3 + u32::try_from(index).expect("three rows");
        sheet.set(at(&format!("A{row}")), *name);
        sheet.set(at(&format!("B{row}")), *count);
        // A formula written without its answer: `recalculate` fills the cache
        // below, so the file opens with numbers in it rather than blanks.
        sheet.set(
            at(&format!("C{row}")),
            excelerate::model::CellValue::Formula {
                formula: format!("B{row}*{price}"),
                cached: None,
            },
        );
        sheet.entry(at(&format!("C{row}"))).style = money;
    }

    let total = 3 + u32::try_from(rows.len()).expect("three rows");
    sheet.set(at(&format!("A{total}")), "Итого");
    sheet.entry(at(&format!("A{total}"))).style = header;
    sheet.set(
        at(&format!("C{total}")),
        excelerate::model::CellValue::Formula {
            formula: format!("SUM(C3:C{})", total - 1),
            cached: None,
        },
    );
    sheet.entry(at(&format!("C{total}"))).style = money;

    // Column widths, in characters of the default font.
    for (letter, width) in [("A", 18.0), ("B", 10.0), ("C", 16.0)] {
        let col = at(&format!("{letter}1")).col;
        let mut run = ColumnRun::new(col, col);
        run.width = Some(width);
        run.custom_width = true;
        sheet.columns.push(run);
    }

    // A table, so that Excel draws banding and structured references work.
    sheet.tables.push(Table {
        id: 1,
        name: "Продажи".to_owned(),
        display_name: "Продажи".to_owned(),
        range: Range::new(at("A2"), at(&format!("C{}", total - 1))),
        header_row_count: None,
        totals_row_count: None,
        auto_filter: Some(Range::new(at("A2"), at(&format!("C{}", total - 1)))),
        columns: ["Товар", "Штук", "Сумма"]
            .into_iter()
            .enumerate()
            .map(|(index, name)| TableColumn {
                id: u32::try_from(index).expect("three columns") + 1,
                name: name.to_owned(),
                ..TableColumn::default()
            })
            .collect(),
        style: None,
    });

    sheet.comments.insert(
        at("C1"),
        Comment {
            author: "excelerate".to_owned(),
            text: vec![TextRun {
                text: "Суммы считает формула, не человек.".to_owned(),
                font: None,
            }],
        },
    );

    sheet.hyperlinks.push(Hyperlink {
        range: Range::new(at("A1"), at("A1")),
        target: LinkTarget::Outside("https://example.org/reports".to_owned()),
        display: None,
        tooltip: Some("Откуда взялись цифры".to_owned()),
    });

    // Locking a sheet is not encryption: it stops fingers, not programs.
    sheet.protection.sheet = Some(true);
    sheet.protection.password = PasswordHash::new("проба");

    book.add_sheet(sheet).expect("the book takes the sheet");

    let computed = recalculate(&mut book, None, &Options::default());
    excelerate::writer::xlsx::write_xlsx(&book, &output).expect("output writes");
    println!("{output}: {computed} formulas computed");
}
