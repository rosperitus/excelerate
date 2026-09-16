//! Tables of expected answers, checked against the fixtures under
//! `tests/fixtures/`.
//!
//! Each fixture is a list of inputs and the answers Excel gives for them,
//! recorded once and committed, so the suite needs nothing installed to run.
//! Where a case is one Excel itself gets wrong, or one where implementations
//! are known to disagree, it is called out in the test rather than smoothed
//! over.

// A failing assertion is this suite's output; panicking is the report.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::coordinate::{Col, Range, Row};
use excelerate::reader::CsvOptions;
use excelerate::shared::date::{DateTime, Epoch, from_serial, to_serial};

/// Reads a fixture as rows of already-split fields, skipping `#` comments.
fn fixture(name: &str) -> Vec<Vec<String>> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");
    let text = std::fs::read_to_string(format!("{path}{name}"))
        .unwrap_or_else(|e| panic!("fixture {name} is missing ({e})"));
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| l.split('\t').map(str::to_owned).collect())
        .collect()
}

#[test]
fn every_column_label_matches_excel() {
    let rows = fixture("columns.tsv");
    assert_eq!(
        rows.len(),
        16_384,
        "the fixture must cover every column of a sheet"
    );
    for row in rows {
        let (index1, letters) = (row[0].parse::<u64>().unwrap(), row[1].as_str());
        let col = Col::from_one_based(index1).expect("column within the sheet");
        assert_eq!(col.to_letters(), letters, "rendering column {index1}");
        assert_eq!(
            u64::from(Col::from_letters(letters).unwrap().one_based()),
            index1,
            "parsing {letters}"
        );
    }
}

#[test]
fn range_boundaries_match_excel() {
    for row in fixture("ranges.tsv") {
        let range = Range::parse(&row[0]).unwrap_or_else(|e| panic!("parsing {}: {e}", row[0]));
        let expected: Vec<u32> = row[1..].iter().map(|f| f.parse().unwrap()).collect();
        let [start_col, start_row, end_col, end_row, width, height] = expected[..] else {
            panic!("ranges.tsv row has the wrong number of fields: {row:?}");
        };
        assert_eq!(
            (
                range.start.col.one_based(),
                range.start.row.one_based(),
                range.end.col.one_based(),
                range.end.row.one_based(),
                range.width(),
                range.height(),
            ),
            (start_col, start_row, end_col, end_row, width, height),
            "range {}",
            row[0]
        );
    }
}

#[test]
fn serial_dates_match_excel() {
    for row in fixture("dates_1900.tsv") {
        let serial: f64 = row[0].parse().unwrap();
        let expected = &row[1];
        let dt = from_serial(serial, Epoch::Windows1900)
            .unwrap_or_else(|e| panic!("serial {serial}: {e}"));
        assert_eq!(
            format!("{:04}-{:02}-{:02}", dt.year, dt.month, dt.day),
            *expected,
            "serial {serial} to date"
        );

        // Each date is round-tripped back as well; where
        // agrees with itself, we must land on the same serial.
        let back: f64 = row[2].parse().unwrap();
        let ours = to_serial(
            DateTime::date(dt.year, dt.month, dt.day),
            Epoch::Windows1900,
        )
        .unwrap_or_else(|e| panic!("date {expected}: {e}"));
        assert!(
            (ours - back).abs() < f64::EPSILON,
            "date {expected} back to serial: {back}, ours {ours}"
        );
    }
}

#[test]
fn times_of_day_match_excel() {
    for row in fixture("times.tsv") {
        let frac: f64 = row[0].parse().unwrap();
        // Any date will do; the fraction is what is under test.
        let dt = from_serial(45_658.0 + frac, Epoch::Windows1900).unwrap();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "second is in 0.0..60.0"
        )]
        let secs = dt.second as u32;
        assert_eq!(
            format!("{:02}:{:02}:{:02}", dt.hour, dt.minute, secs),
            row[1],
            "fraction {frac}"
        );
    }
}

/// Deviations from that are deliberate, not accidental.
///
/// Both are cases where an implementation is known to misbehave, so it cannot serve as the
/// oracle.
#[test]
fn documented_deviations_from_other_engines() {
    // An implementation returning the corners unswapped gives width -1 and
    // height -4.
    // A range with a negative size is not a thing Excel has.
    let swapped = Range::parse("D9:B4").unwrap();
    assert_eq!(swapped, Range::parse("B4:D9").unwrap());
    assert_eq!((swapped.width(), swapped.height()), (3, 6));

    // A parser that does not strip the `$` first reads row 0 for both corners.
    let absolute = Range::parse("$B$4:$D$9").unwrap();
    assert_eq!(absolute, Range::parse("B4:D9").unwrap());

    // Mapping serial 60 to 1900-02-28 - the same day as serial 59 - makes
    // its conversion is not reversible there. Excel shows 1900-02-29, and the
    // read -> write -> read invariant needs a bijection, so we follow Excel.
    let phantom = from_serial(60.0, Epoch::Windows1900).unwrap();
    assert_eq!((phantom.year, phantom.month, phantom.day), (1900, 2, 29));
    assert!(
        (to_serial(DateTime::date(1900, 2, 29), Epoch::Windows1900).unwrap() - 60.0).abs()
            < f64::EPSILON,
        "serial 60 must survive a round trip"
    );

    // Row 0 is sometimes allowed for backwards compatibility.
    assert!(excelerate::CellRef::parse("A0").is_err());
    assert!(Row::from_one_based(0).is_err());

    // An implementation that gives CSV a backslash escape - which no
    // spreadsheet has - reads `"a\"b",c` as one field
    // `a\"b`. Excel closes the quote at the quote, giving `a\b"`.
    let book = excelerate::reader::read_csv_str("\"a\\\"b\",c\n", &CsvOptions::with_delimiter(','));
    let sheet = &book.sheets()[0];
    let first = sheet
        .get(excelerate::CellRef::parse("A1").unwrap())
        .map(|c| &c.value)
        .and_then(excelerate::model::CellValue::plain_text);
    assert_eq!(first.as_deref(), Some("a\\b\""));
}

#[test]
fn xlsx_cells_match_excel() {
    use excelerate::model::CellValue;
    use excelerate::reader::read_xlsx;

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample.xlsx");
    let book = read_xlsx(path).expect("sample.xlsx reads");

    for row in fixture("sample_cells.tsv") {
        let (sheet_index, title, coord, kind) = (
            row[0].parse::<usize>().unwrap(),
            row[1].as_str(),
            row[2].as_str(),
            row[3].as_str(),
        );
        // The generator escapes tabs and newlines so a value stays on one line.
        let expected = row[4].replace("\\n", "\n").replace("\\t", "\t");

        let sheet = book
            .sheet(sheet_index)
            .unwrap_or_else(|| panic!("no sheet {sheet_index}"));
        assert_eq!(sheet.title(), title, "sheet {sheet_index} title");

        let at = excelerate::CellRef::parse(coord).unwrap();
        // A styled-but-valueless cell may be reported as type "null"; for us
        // that is an empty value, and it may legitimately be absent.
        let Some(cell) = sheet.get(at) else {
            assert_eq!(kind, "null", "{title}!{coord} is missing but has a value");
            continue;
        };

        let actual = match &cell.value {
            CellValue::Empty => String::new(),
            CellValue::Number(n) => {
                // Integral floats are recorded without a decimal point.
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    format!("{n:.0}")
                } else {
                    n.to_string()
                }
            }
            CellValue::Text(_) | CellValue::RichText(_) => {
                cell.value.plain_text().unwrap_or_default()
            }
            CellValue::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
            CellValue::Error(e) => e.to_string(),
            CellValue::Formula { formula, .. } => format!("={formula}"),
        };
        assert_eq!(actual, expected, "{title}!{coord} (type {kind})");
    }
}

#[test]
fn xlsx_structure_matches_excel() {
    use excelerate::reader::read_xlsx;

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample.xlsx");
    let book = read_xlsx(path).expect("sample.xlsx reads");

    assert_eq!(book.sheets().len(), 2, "both sheets are read, in tab order");
    assert_eq!(book.sheets()[0].title(), "Data");
    assert_eq!(book.sheets()[1].title(), "Second");

    let data = &book.sheets()[0];
    assert_eq!(
        data.merges,
        vec![Range::parse("A5:B5").unwrap()],
        "merged ranges are read"
    );

    // A formula cell keeps both the text and the result Excel last computed,
    // which is what lets the workbook be read without a formula engine.
    let d2 = data.get(excelerate::CellRef::parse("D2").unwrap()).unwrap();
    let excelerate::model::CellValue::Formula { formula, cached } = &d2.value else {
        panic!("D2 should be a formula, got {:?}", d2.value);
    };
    assert_eq!(formula, "B1*2");
    assert_eq!(
        cached.as_deref(),
        Some(&excelerate::model::CellValue::Number(84.0)),
        "the cached result is kept"
    );

    // F1 carries a date format, so it must land on a non-default style.
    let f1 = data.get(excelerate::CellRef::parse("F1").unwrap()).unwrap();
    assert_ne!(
        f1.style,
        excelerate::style::StyleId::default(),
        "F1 is styled"
    );
}

#[test]
fn xlsx_styles_match_excel() {
    use excelerate::reader::read_xlsx;
    use excelerate::style::{Color, Pattern, ProtectionState};

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/styles.xlsx");
    let book = read_xlsx(path).expect("styles.xlsx reads");
    let sheet = &book.sheets()[0];

    for row in fixture("styles_cells.tsv") {
        let coord = row[0].as_str();
        let cell = sheet
            .get(excelerate::CellRef::parse(coord).unwrap())
            .unwrap_or_else(|| panic!("no cell at {coord}"));
        let style = book
            .styles
            .get(cell.style)
            .unwrap_or_else(|| panic!("no style for {coord}"));

        let f = &style.font;
        assert_eq!(f.name, row[1], "{coord} font name");
        assert!(
            (f.size_points() - row[2].parse::<f64>().unwrap()).abs() < 1e-9,
            "{coord} font size"
        );
        assert_eq!(f.bold, row[3] == "1", "{coord} bold");
        assert_eq!(f.italic, row[4] == "1", "{coord} italic");
        assert_eq!(f.underline.as_str(), row[5], "{coord} underline");
        assert_eq!(f.strike, row[6] == "1", "{coord} strike");

        // A reader may resolve a theme colour to its rgb; we keep
        // the reference, so only one of the two is ever set.
        let theme: i32 = row[8].parse().unwrap();
        if theme >= 0 {
            assert!(
                matches!(f.color, Color::Theme { id, .. } if id == theme.unsigned_abs()),
                "{coord} theme colour, got {:?}",
                f.color
            );
        } else if !row[7].is_empty() {
            assert_eq!(
                f.color.to_argb_str().as_deref(),
                Some(row[7].as_str()),
                "{coord} font rgb"
            );
        }

        assert_eq!(style.fill.pattern.as_str(), row[9], "{coord} fill pattern");
        if style.fill.pattern != Pattern::None {
            assert_eq!(
                style.fill.foreground.to_argb_str().as_deref(),
                Some(row[10].as_str()),
                "{coord} fill colour"
            );
        }

        let b = &style.borders;
        assert_eq!(b.left.style.as_str(), row[11], "{coord} left border");
        assert_eq!(b.right.style.as_str(), row[12], "{coord} right border");
        assert_eq!(b.top.style.as_str(), row[13], "{coord} top border");
        assert_eq!(b.bottom.style.as_str(), row[14], "{coord} bottom border");
        if b.bottom.style != excelerate::style::BorderStyle::None && !row[15].is_empty() {
            assert_eq!(
                b.bottom.color.to_argb_str().as_deref(),
                Some(row[15].as_str()),
                "{coord} bottom border colour"
            );
        }

        let a = &style.alignment;
        assert_eq!(
            a.horizontal.as_str().unwrap_or("general"),
            row[16],
            "{coord} horizontal"
        );
        assert_eq!(
            a.vertical.as_str().unwrap_or("bottom"),
            row[17],
            "{coord} vertical"
        );
        assert_eq!(a.wrap_text, row[18] == "1", "{coord} wrap");
        assert_eq!(a.indent, row[19].parse::<u32>().unwrap(), "{coord} indent");
        assert_eq!(
            a.text_rotation,
            row[20].parse::<u32>().unwrap(),
            "{coord} rotation"
        );

        // Protection is recorded as "", "protected" or "unprotected".
        let expect_state = |s: &str| match s {
            "protected" => ProtectionState::On,
            "unprotected" => ProtectionState::Off,
            _ => ProtectionState::Inherit,
        };
        assert_eq!(
            style.protection.locked,
            expect_state(&row[21]),
            "{coord} locked"
        );
        assert_eq!(
            style.protection.hidden,
            expect_state(&row[22]),
            "{coord} hidden"
        );
    }
}

#[test]
fn number_formats_match_excel() {
    use excelerate::shared::date::Epoch;
    use excelerate::style::format::{Value, format};

    let rows = fixture("number_formats.tsv");
    assert!(
        rows.len() > 400,
        "the fixture should cover a few hundred pairs"
    );
    for row in rows {
        let (code, raw, expected) = (row[0].as_str(), row[1].as_str(), row[2].as_str());
        let value = match raw.parse::<f64>() {
            Ok(n) => Value::Number(n),
            Err(_) => Value::Text(if raw == "''" { "" } else { raw }),
        };
        assert_eq!(
            format(value, code, Epoch::Windows1900),
            expected,
            "format {code:?} applied to {raw}"
        );
    }
}

/// Where Excel and disagree about formatting, we follow Excel.
///
/// Each case is excluded from `number_formats.tsv`,
/// with the reason recorded there.
#[test]
fn formatting_deviations_from_other_engines() {
    use excelerate::shared::date::Epoch;
    use excelerate::style::format::{Value, format};

    let render = |v: f64, code: &str| format(Value::Number(v), code, Epoch::Windows1900);

    // An implementation printing one exponent digit gives "1.00E+0", which
    // does not match Excel.
    assert_eq!(render(1.0, "0.00E+00"), "1.00E+00");
    assert_eq!(render(1234.5678, "0.00E+00"), "1.23E+03");
    assert_eq!(render(0.5, "0.00E+00"), "5.00E-01");
    // Engineering notation moves the exponent in steps of three.
    assert_eq!(render(1234.5678, "##0.0E+0"), "1.2E+3");

    // General shows eleven significant digits, not the binary expansion of the
    // double, which is what emits.
    assert_eq!(render(1234.5678, "General"), "1234.5678");
    assert_eq!(render(123_456_789.987, "General"), "123456789.99");
    assert_eq!(render(1e-7, "General"), "1E-07", "Excel writes 1.0E-7");

    // 'mmmmm' is the single-letter month; an implementation mapping it to a
    // and writes the three-letter form instead.
    assert_eq!(render(45_658.0, "mmmmm"), "J");
    assert_eq!(render(45_689.0, "mmmmm"), "F");

    // The fourth section applies to text; ignores it.
    assert_eq!(
        format(
            Value::Text("hi"),
            r#"0;-0;"zero";"text: "@"#,
            Epoch::Windows1900
        ),
        "text: hi"
    );

    // Weekdays before 1 March 1900 follow Excel's phantom-day calendar, so
    // serial 1 is a Sunday. The real calendar makes it a Monday.
    assert_eq!(render(1.0, "ddd"), "Sun");
    // Serial 60 is the phantom day itself.
    assert_eq!(render(60.0, "yyyy-mm-dd"), "1900-02-29");
    // Past Excel's last date there is nothing to render, so the number shows.
    assert_eq!(
        render(3_000_000.0, "yyyy-mm-dd"),
        format(Value::Number(3_000_000.0), "General", Epoch::Windows1900)
    );
}

/// Formula results, against the engine evaluates them with.
///
/// The fixture is built on the same little sheet `tests/formula.rs` uses, so a
/// formula means the same thing on both sides. Cases where disagrees
/// with Excel are excluded by the generator with the reason written beside
/// them, and pinned by our own tests instead.
#[test]
fn formula_results_match_excel() {
    use excelerate::error::CellError;
    use excelerate::formula::eval::{Engine, Origin};
    use excelerate::formula::value::Value;
    use excelerate::model::{CellValue, Spreadsheet, Worksheet};

    let cell = |a: &str| excelerate::coordinate::CellRef::parse(a).expect("reference is valid");
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Data").unwrap();
    for row in 1..=3 {
        sheet.set(cell(&format!("A{row}")), f64::from(row));
        sheet.set(cell(&format!("D{row}")), f64::from(row) * 10.0);
    }
    sheet.set(cell("B1"), "x");
    sheet.set(cell("B2"), true);
    sheet.entry(cell("C1")).value = CellValue::Error(CellError::Div0);
    book.add_sheet(sheet).unwrap();
    let mut other = Worksheet::new("Другой лист").unwrap();
    other.set(cell("A1"), 42.0);
    book.add_sheet(other).unwrap();

    let mut engine = Engine::new(&book);
    let origin = Origin::new(0, cell("Z100"));
    let mut checked = 0;
    let mut differ: Vec<String> = Vec::new();
    for row in fixture("formulas.tsv") {
        let [formula, kind, expected] = row.as_slice() else {
            panic!("malformed row: {row:?}");
        };
        let (formula, kind, expected) = (formula.as_str(), kind.as_str(), expected.as_str());
        let got = engine.eval(origin, formula);
        let agrees = match kind {
            "n" => match (got.clone(), expected.parse::<f64>()) {
                (Value::Number(n), Ok(want)) => (n - want).abs() <= want.abs() * 1e-12 + 1e-12,
                _ => false,
            },
            "b" => got == Value::Bool(expected == "TRUE"),
            "e" => CellError::parse(expected).is_some_and(|e| got == Value::Error(e)),
            "z" => got == Value::Number(0.0),
            _ => got == Value::Text(expected.replace("\\t", "\t").replace("\\n", "\n")),
        };
        if !agrees {
            differ.push(format!("{formula}: {kind} {expected:?}, we {got:?}"));
        }
        checked += 1;
    }
    assert!(checked > 100, "only {checked} formulas in the fixture");
    assert!(
        differ.is_empty(),
        "{} formulas differ:\n{}",
        differ.len(),
        differ.join("\n")
    );
}

/// Undoes the escaping the generator applies so a value survives a TSV line.
fn unescape(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[test]
fn csv_records_split_the_way_excel_splits_them() {
    for row in fixture("csv_fields.tsv") {
        let record = unescape(&row[0]);
        let expected: Vec<String> = row[1..].iter().map(|f| unescape(f)).collect();
        // The generator feeds each record to fgetcsv with a line break after
        // it, so the reader has to see the same text.
        let text = format!("{record}\n");
        let book = excelerate::reader::read_csv_str(&text, &CsvOptions::with_delimiter(','));
        let sheet = &book.sheets()[0];
        let got: Vec<String> = (1..=expected.len())
            .map(|col| {
                let at = excelerate::CellRef::new(
                    Col::from_one_based(col as u64).unwrap(),
                    Row::from_one_based(1).unwrap(),
                );
                sheet
                    .get(at)
                    .map(|c| &c.value)
                    .and_then(excelerate::model::CellValue::plain_text)
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(got, expected, "splitting {record:?}");
    }
}

#[test]
fn the_delimiter_is_inferred_the_way_excel_infers_it() {
    for row in fixture("csv_delimiters.tsv") {
        let text = unescape(&row[0]);
        let expected = unescape(row.get(1).map_or("", String::as_str));
        // Inference is not public, so it is observed through what it produces:
        // splitting on the right delimiter puts each line in one row of cells.
        let split = |delimiter: Option<char>| {
            let options = CsvOptions {
                delimiter,
                ..CsvOptions::default()
            };
            let book = excelerate::reader::read_csv_str(&text, &options);
            book.sheets()[0]
                .iter()
                .map(|(at, cell)| (at, cell.value.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            split(None),
            split(expected.chars().next()),
            "inferring the delimiter of {text:?}, says {expected:?}"
        );
    }
}

/// The xls the writer produced reads back as the workbook that went
/// into it. The reader is not the oracle here - the writer is: the
/// fixture is written by and the values are what the script that
/// built it put in.
#[test]
fn xls_cells_match_what_excel_wrote() {
    use excelerate::model::CellValue;
    use excelerate::reader::read_xls;
    use excelerate::style::NumberFormat;

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample.xls");
    let book = read_xls(path).expect("sample.xls reads");

    assert_eq!(book.sheets().len(), 2, "both sheets");
    assert_eq!(book.sheets()[0].title(), "Values");
    assert_eq!(book.sheets()[1].title(), "Second");

    let sheet = &book.sheets()[0];
    let value = |a: &str| {
        sheet
            .get(excelerate::CellRef::parse(a).unwrap())
            .map(|c| c.value.clone())
    };
    assert_eq!(value("A1"), Some(CellValue::text("plain")));
    assert_eq!(value("B1"), Some(CellValue::text("привет")));
    assert_eq!(value("C1"), Some(CellValue::text("x".repeat(300))));
    assert_eq!(value("A2"), Some(CellValue::Number(1234.5)));
    assert_eq!(value("A3"), Some(CellValue::Number(42.0)));
    assert_eq!(value("B2"), Some(CellValue::Number(-0.25)));
    assert_eq!(value("C2"), Some(CellValue::Bool(true)));
    assert_eq!(
        value("D2"),
        Some(CellValue::Error(excelerate::error::CellError::Div0))
    );
    // A formula comes back as the result the file cached for it.
    assert_eq!(value("A4"), Some(CellValue::Number(2469.0)));
    assert_eq!(value("B4"), Some(CellValue::text("привет!")));
    assert_eq!(value("A5"), Some(CellValue::Number(0.125)));

    let format = |a: &str| {
        let cell = sheet.get(excelerate::CellRef::parse(a).unwrap())?;
        Some(book.styles.get(cell.style)?.number_format.clone())
    };
    // `0.00%` is Excel's built-in format 10, and that is how wrote it.
    assert_eq!(format("A5"), Some(NumberFormat::Builtin(10)));
    assert_eq!(
        format("B5"),
        Some(NumberFormat::Custom("yyyy-mm-dd".to_owned()))
    );

    // The indent lives in the low nibble of the XF alignment byte; the
    // reader agrees cell for cell on the workbooks this was checked against.
    let indent = |a: &str| {
        let cell = sheet.get(excelerate::CellRef::parse(a).unwrap())?;
        Some(book.styles.get(cell.style)?.alignment.indent)
    };
    assert_eq!(indent("A10"), Some(0));
    assert_eq!(indent("A11"), Some(1));
    assert_eq!(indent("A12"), Some(2));
    assert_eq!(indent("A13"), Some(15), "the nibble holds fifteen at most");

    assert_eq!(sheet.merges, vec![Range::parse("A7:C8").unwrap()]);
    assert_eq!(
        book.sheets()[1]
            .get(excelerate::CellRef::parse("A2").unwrap())
            .map(|c| c.value.clone()),
        Some(CellValue::Number(7.0))
    );
}

/// The SYLK fixture reads back with its values intact: the values
/// beside each assertion are what another reader printed for the same file.
#[test]
fn slk_cells_match_excel() {
    use excelerate::model::CellValue;
    use excelerate::reader::read_slk;
    use excelerate::style::NumberFormat;

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample.slk");
    let book = read_slk(path).expect("sample.slk reads");
    let sheet = book.sheet(0).expect("one sheet");
    assert_eq!(sheet.title(), "sample", "the file names the sheet");

    let value = |a: &str| {
        sheet
            .get(excelerate::CellRef::parse(a).unwrap())
            .map(|c| c.value.clone())
    };
    assert_eq!(value("A1"), Some(CellValue::text("plain")));
    // `;;` in the file is one literal semicolon.
    assert_eq!(value("B1"), Some(CellValue::text("a;b")));
    assert_eq!(value("A2"), Some(CellValue::Number(1234.5)));
    assert_eq!(value("B2"), Some(CellValue::Number(-0.25)));
    assert_eq!(
        value("A3"),
        Some(CellValue::Error(excelerate::error::CellError::Div0))
    );
    // R1C1 becomes A1, and the shared formula is offset from its master.
    let formula = |a: &str| match value(a) {
        Some(CellValue::Formula { formula, .. }) => formula,
        other => panic!("{a} is not a formula: {other:?}"),
    };
    assert_eq!(formula("A4"), "A2*2");
    assert_eq!(formula("B4"), "B2*2");

    let style = sheet
        .get(excelerate::CellRef::parse("A2").unwrap())
        .map(|c| c.style)
        .and_then(|id| book.styles.get(id))
        .expect("A2 has a style");
    assert_eq!(style.number_format, NumberFormat::Custom("0.00".to_owned()));
    assert!(style.font.bold);
    assert_eq!(
        sheet.column_width(excelerate::Col::from_one_based(1).unwrap()),
        Some(12.0)
    );
}

/// The Gnumeric fixture, likewise read back with what it holds intact.
#[test]
fn gnumeric_cells_match_excel() {
    use excelerate::model::CellValue;
    use excelerate::reader::read_gnumeric;
    use excelerate::style::NumberFormat;

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/sample.gnumeric"
    );
    let book = read_gnumeric(path).expect("sample.gnumeric reads");
    let sheet = book.sheet(0).expect("one sheet");
    assert_eq!(sheet.title(), "Data");

    let value = |a: &str| {
        sheet
            .get(excelerate::CellRef::parse(a).unwrap())
            .map(|c| c.value.clone())
    };
    assert_eq!(value("A1"), Some(CellValue::text("plain")));
    assert_eq!(value("B1"), Some(CellValue::text("привет & <xml>")));
    assert_eq!(value("A2"), Some(CellValue::Number(1234.5)));
    assert_eq!(value("B2"), Some(CellValue::Bool(true)));
    assert_eq!(
        value("A3"),
        Some(CellValue::Error(excelerate::error::CellError::Div0))
    );
    let formula = |a: &str| match value(a) {
        Some(CellValue::Formula { formula, .. }) => formula,
        other => panic!("{a} is not a formula: {other:?}"),
    };
    assert_eq!(formula("A4"), "A2*2");
    // The second cell of the run carries the id alone and offsets the master.
    assert_eq!(formula("B4"), "B2*2");

    let style = sheet
        .get(excelerate::CellRef::parse("A2").unwrap())
        .map(|c| c.style)
        .and_then(|id| book.styles.get(id))
        .expect("A2 has a style");
    assert_eq!(style.number_format, NumberFormat::Custom("0.00".to_owned()));
    assert_eq!(sheet.merges, vec![Range::parse("A5:B6").unwrap()]);
    let width = sheet
        .column_width(excelerate::Col::from_one_based(1).unwrap())
        .expect("column A has a width");
    assert!((width - 17.777_777_777_777_775).abs() < 1e-9, "{width}");
    assert_eq!(
        sheet.row_height(excelerate::Row::from_one_based(1).unwrap()),
        Some(20.0)
    );
}

/// The `SpreadsheetML` 2003 fixture, read the same way.
#[test]
fn xml2003_cells_match_excel() {
    use excelerate::model::CellValue;
    use excelerate::reader::read_xml2003;
    use excelerate::style::{HorizontalAlign, NumberFormat};

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample.xml");
    let book = read_xml2003(path).expect("sample.xml reads");
    assert_eq!(book.sheets().len(), 2);
    assert_eq!(book.sheets()[0].title(), "Data");
    assert_eq!(book.sheets()[1].title(), "Second");
    let sheet = &book.sheets()[0];

    let value = |a: &str| {
        sheet
            .get(excelerate::CellRef::parse(a).unwrap())
            .map(|c| c.value.clone())
    };
    assert_eq!(value("A1"), Some(CellValue::text("plain")));
    assert_eq!(value("B1"), Some(CellValue::text("привет & <xml>")));
    assert_eq!(value("A2"), Some(CellValue::Number(1234.5)));
    assert_eq!(value("B2"), Some(CellValue::Bool(true)));
    assert_eq!(
        value("C2"),
        Some(CellValue::Error(excelerate::error::CellError::Div0))
    );
    // `ss:Index` puts the formula in B3, and R1C1 makes it A2.
    match value("B3") {
        Some(CellValue::Formula { formula, cached }) => {
            assert_eq!(formula, "A2*2");
            assert_eq!(cached.as_deref(), Some(&CellValue::Number(2469.0)));
        }
        other => panic!("B3 is not a formula: {other:?}"),
    }
    assert_eq!(value("A4"), Some(CellValue::text("merged")));
    assert_eq!(sheet.merges, vec![Range::parse("A4:B5").unwrap()]);

    let style = sheet
        .get(excelerate::CellRef::parse("A2").unwrap())
        .map(|c| c.style)
        .and_then(|id| book.styles.get(id))
        .expect("A2 has a style");
    assert_eq!(style.number_format, NumberFormat::Custom("0.00".to_owned()));
    assert!(style.font.bold);
    assert_eq!(
        style.font.color,
        excelerate::style::Color::Argb(0xFFFF_0000)
    );
    assert_eq!(style.alignment.horizontal, HorizontalAlign::Center);
    assert!(style.alignment.wrap_text);
    // The default style names the font, and a named style starts from it.
    assert_eq!(style.font.name, "Calibri");

    let width = sheet
        .column_width(excelerate::Col::from_one_based(2).unwrap())
        .expect("the span covers column B");
    assert!((width - 17.777_777_777_777_775).abs() < 1e-9, "{width}");
    assert_eq!(
        sheet.row_height(excelerate::Row::from_one_based(1).unwrap()),
        Some(20.0)
    );
}
