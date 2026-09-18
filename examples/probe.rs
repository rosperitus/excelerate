//! Builds the workbooks that ask Excel what it does, where this crate had to
//! guess: fraction formats, a locale tag, the newest functions, the objects
//! written from the model, an array formula in three formats, a password.
//!
//! `cargo run --release --example probe -- /tmp/probe`
//!
//! Each question is a formula, so Excel answers it on opening and stores the
//! answer beside it. Open the files, save them (same format, a new name), and
//! `probe_check` compares Excel's answers with this engine's, cell for cell.

use excelerate::coordinate::{CellRef, Col, Range, Row};
use excelerate::formula::eval::{Engine, Origin, recalculate};
use excelerate::model::chart::{
    Anchor, ChartEx, ChartText, Dimension, DimensionRole, Marker, SeriesLayout,
};
use excelerate::model::pivot::{
    CacheField, CacheSource, DataField, PivotAxis, PivotCache, PivotField, PivotTable, Subtotal,
};
use excelerate::model::protection::PasswordHash;
use excelerate::model::shape::Shape;
use excelerate::model::{CellValue, Spreadsheet, Worksheet};
use excelerate::progress::Options;

type Fallible = Result<(), Box<dyn std::error::Error>>;

/// The password the protected probe is locked with.
const PASSWORD: &str = "проба";

fn main() -> Fallible {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "probe".to_owned());
    std::fs::create_dir_all(&dir)?;
    formulas(&dir)?;
    objects(&dir)?;
    isolated(&dir)?;
    edited(&dir)?;
    arrays(&dir)?;
    protected(&dir)?;
    instructions(&dir)?;
    println!("Файлы лежат в {dir}");
    Ok(())
}

fn at(address: &str) -> Result<CellRef, excelerate::Error> {
    CellRef::parse(address)
}

fn cell(col: u32, row: u32) -> CellRef {
    CellRef::new(
        Col::new(col).unwrap_or_default(),
        Row::new(row).unwrap_or_default(),
    )
}

fn formula(text: &str) -> CellValue {
    CellValue::Formula {
        formula: text.to_owned(),
        cached: None,
    }
}

/// What this engine answers for a formula, as text, so a human comparing by
/// eye sees it beside Excel's answer. An array is written row by row.
fn ours(book: &Spreadsheet, sheet: usize, text: &str) -> String {
    let mut engine = Engine::new(book);
    let value = engine.eval(Origin::new(sheet, cell(200, 200)), text);
    show(&value)
}

fn show(value: &excelerate::formula::value::Value) -> String {
    use excelerate::formula::value::Value;
    match value {
        Value::Blank => String::new(),
        Value::Number(n) => format!("{n}"),
        Value::Text(t) => t.clone(),
        Value::Bool(b) => if *b { "ИСТИНА" } else { "ЛОЖЬ" }.to_owned(),
        Value::Error(e) => e.as_str().to_owned(),
        Value::Lambda(_) => "(функция)".to_owned(),
        Value::Array(rows) => rows
            .iter()
            .map(|row| row.iter().map(show).collect::<Vec<_>>().join(" | "))
            .collect::<Vec<_>>()
            .join(" ; "),
    }
}

/// The questions that answer with one value: number formats, the locale tag,
/// the regular expressions.
fn scalar_questions() -> Vec<(&'static str, &'static str)> {
    // A format string is typed by a person, so Excel reads it in the language
    // it runs in: a Russian Excel wants Д, М, Г where an English one wants
    // d, m, y, and it took the space in `# ?/?` for a group separator. Each
    // question is therefore asked in several spellings; the one Excel
    // understands is the answer, and the rest come back `#ЗНАЧ!`.
    vec![
        ("дробь: 2 -> # ?/?", r##"TEXT(2,"# ?/?")"##),
        (
            "дробь: 2 -> # ?/? (пробел экранирован)",
            r##"TEXT(2,"#\ ?/?")"##,
        ),
        ("дробь: 2 -> 0 ?/?", r#"TEXT(2,"0 ?/?")"#),
        ("дробь: 2 -> ?/? (без целой части)", r#"TEXT(2,"?/?")"#),
        ("дробь: 0,5 -> # ?/?", r##"TEXT(0.5,"# ?/?")"##),
        ("дробь: 0,5 -> ?/?", r#"TEXT(0.5,"?/?")"#),
        ("дробь: 0,99 -> ?/?", r#"TEXT(0.99,"?/?")"#),
        ("дробь: 1,25 -> ?/?", r#"TEXT(1.25,"?/?")"#),
        ("дробь: 1,25 -> # ?/?", r##"TEXT(1.25,"# ?/?")"##),
        ("дробь: 5,25 -> ???/???", r#"TEXT(5.25,"???/???")"#),
        ("дробь: 5,25 -> # ???/???", r##"TEXT(5.25,"# ???/???")"##),
        (
            "дробь: пи -> ?/??? (это уже совпало)",
            r#"TEXT(PI(),"?/???")"#,
        ),
        (
            "дробь: 1,3 -> ?/8 (знаменатель задан)",
            r#"TEXT(1.3,"?/8")"#,
        ),
        ("дробь: -1,75 -> ?/?", r#"TEXT(-1.75,"?/?")"#),
        ("число: 1234,5 -> # ##0,00", r##"TEXT(1234.5,"# ##0,00")"##),
        ("число: 1234,5 -> #,##0.00", r##"TEXT(1234.5,"#,##0.00")"##),
        (
            "валюта: 1234,5 -> # ##0,00 [$₽-419]",
            r##"TEXT(1234.5,"# ##0,00 [$₽-419]")"##,
        ),
        (
            "дата: ДД.ММ.ГГГГ (русские коды)",
            r#"TEXT(DATE(2025,1,15),"ДД.ММ.ГГГГ")"#,
        ),
        (
            "дата: полный месяц, русские коды",
            r#"TEXT(DATE(2025,1,15),"Д ММММ ГГГГ")"#,
        ),
        (
            "дата: полный месяц с тегом локали",
            r#"TEXT(DATE(2025,1,15),"[$-419]Д ММММ ГГГГ")"#,
        ),
        (
            "дата: полный месяц, английские коды",
            r#"TEXT(DATE(2025,1,15),"[$-419]d mmmm yyyy")"#,
        ),
        (
            "дата: сокращённый месяц (русские коды)",
            r#"TEXT(DATE(2025,1,15),"МММ")"#,
        ),
        (
            "дата: день недели (русские коды)",
            r#"TEXT(DATE(2025,1,15),"ДДДД")"#,
        ),
        (
            "дата: день недели сокращённо",
            r#"TEXT(DATE(2025,1,15),"ДДД")"#,
        ),
        (
            "дата: одна буква месяца",
            r#"TEXT(DATE(2025,1,15),"МММММ")"#,
        ),
        ("месяц как текст без формата", "MONTH(DATE(2025,1,15))"),
        ("время: Ч:ММ:СС (русские коды)", r#"TEXT(0.5,"Ч:ММ:СС")"#),
        ("время: [Ч]:ММ (прошло часов)", r#"TEXT(1.5,"[Ч]:ММ")"#),
        ("число: 1234,5 -> 0,00", r#"TEXT(1234.5,"0,00")"#),
        ("число: 1234,5 -> 0.00", r#"TEXT(1234.5,"0.00")"#),
        ("число: 1234,5 -> # ##0", "TEXT(1234.5,\"# ##0\")"),
        ("процент: 0,256 -> 0,0%", r#"TEXT(0.256,"0,0%")"#),
    ]
}

/// The questions that answer with a rectangle, which needs room around it.
fn array_questions() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "регэксп: все совпадения (куда разливается)",
            r#"_xlfn.REGEXEXTRACT("a1 b22","\d+",1)"#,
        ),
        (
            "регэксп: группы первого совпадения",
            r#"_xlfn.REGEXEXTRACT("Соня Рис","(\w+) (\w+)",2)"#,
        ),
        (
            "сводка: сумма по группам",
            r"_xlfn.GROUPBY($A$4:$A$9,$C$4:$C$9,_xleta.SUM)",
        ),
        (
            "сводка: заголовки показаны",
            r"_xlfn.GROUPBY($A$3:$A$9,$C$3:$C$9,_xleta.SUM,3)",
        ),
        (
            "сводка: без итога, по убыванию",
            r"_xlfn.GROUPBY($A$4:$A$9,$C$4:$C$9,_xleta.SUM,,0,-2)",
        ),
        (
            "сводка: два поля, промежуточные итоги",
            r"_xlfn.GROUPBY($A$4:$B$9,$C$4:$C$9,_xleta.SUM,,2)",
        ),
        (
            "сводка: строки и столбцы",
            r"_xlfn.PIVOTBY($A$4:$A$9,$B$4:$B$9,$C$4:$C$9,_xleta.SUM)",
        ),
        ("обрезка пустых краёв", "_xlfn.TRIMRANGE($E$3:$H$12)"),
    ]
}

/// `probe-formulas.xlsx`: a question per row, Excel's answer beside ours.
fn formulas(dir: &str) -> Fallible {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Скаляры")?;
    sheet.set(at("A1")?, "Что спрашиваем");
    sheet.set(at("B1")?, "Формула (текстом)");
    sheet.set(at("C1")?, "Ответ Excel");
    sheet.set(at("D1")?, "Наш ответ");
    for (row, (what, text)) in scalar_questions().into_iter().enumerate() {
        let row = u32::try_from(row)? + 1;
        sheet.set(cell(0, row), what);
        sheet.set(cell(1, row), text);
        sheet.set(cell(2, row), formula(text));
    }
    book.add_sheet(sheet)?;

    let mut wide = Worksheet::new("Массивы")?;
    // A small table the summaries are asked about, and a rectangle with empty
    // edges for `TRIMRANGE`.
    wide.set(at("A3")?, "Регион");
    wide.set(at("B3")?, "Товар");
    wide.set(at("C3")?, "Продажи");
    for (row, (region, product, sales)) in [
        ("Север", "Хлеб", 10.0),
        ("Север", "Молоко", 20.0),
        ("Юг", "Хлеб", 30.0),
        ("Юг", "Молоко", 40.0),
        ("Север", "Хлеб", 50.0),
        ("Юг", "Хлеб", 60.0),
    ]
    .into_iter()
    .enumerate()
    {
        let row = u32::try_from(row)? + 3;
        wide.set(cell(0, row), region);
        wide.set(cell(1, row), product);
        wide.set(cell(2, row), sales);
    }
    wide.set(at("F5")?, 1.0);
    wide.set(at("G5")?, 2.0);
    wide.set(at("F6")?, 3.0);
    wide.set(at("G6")?, 4.0);
    // Each array question gets its own block of rows, with the text of the
    // question and our answer far enough to the right to stay clear of the
    // spill.
    for (index, (what, text)) in array_questions().into_iter().enumerate() {
        let row = u32::try_from(index)? * 12 + 14;
        wide.set(cell(0, row), what);
        wide.set(cell(0, row + 1), text);
        wide.set(cell(1, row + 2), formula(text));
        wide.set(cell(11, row + 2), "наш ответ:");
    }
    book.add_sheet(wide)?;
    // No cached answers in the question cells: a cache Excel decides to trust
    // would be ours, and the probe would be asking itself.
    recalculate_on_load(&mut book);
    // Ours goes in beside each question, as text.
    let answers: Vec<(usize, CellRef, String)> = {
        let mut out = Vec::new();
        for (row, (_, text)) in scalar_questions().into_iter().enumerate() {
            out.push((0, cell(3, u32::try_from(row)? + 1), ours(&book, 0, text)));
        }
        for (index, (_, text)) in array_questions().into_iter().enumerate() {
            let row = u32::try_from(index)? * 12 + 16;
            out.push((1, cell(12, row), ours(&book, 1, text)));
        }
        out
    };
    for (sheet, at, text) in answers {
        if let Some(sheet) = book.sheet_mut(sheet) {
            sheet.set(at, text);
        }
    }
    excelerate::writer::xlsx::write_xlsx(&book, format!("{dir}/probe-formulas.xlsx"))?;
    Ok(())
}

/// `probe-objects.xlsx`: everything this crate writes from the model and no
/// other engine could check - three 2016 charts, two shapes, a pivot report.
fn objects(dir: &str) -> Fallible {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Данные")?;
    sheet.set(at("A1")?, "Этап");
    sheet.set(at("B1")?, "Сумма");
    for (row, (stage, amount)) in [
        ("Лиды", 1000.0),
        ("Звонки", 700.0),
        ("Встречи", 400.0),
        ("Счета", 250.0),
        ("Оплаты", 120.0),
    ]
    .into_iter()
    .enumerate()
    {
        let row = u32::try_from(row)? + 1;
        sheet.set(cell(0, row), stage);
        sheet.set(cell(1, row), amount);
    }
    // A pivot needs a table of its own, with repeats to group.
    sheet.set(at("D1")?, "Регион");
    sheet.set(at("E1")?, "Товар");
    sheet.set(at("F1")?, "Продажи");
    for (row, (region, product, sales)) in [
        ("Север", "Хлеб", 10.0),
        ("Север", "Молоко", 20.0),
        ("Юг", "Хлеб", 30.0),
        ("Юг", "Молоко", 40.0),
        ("Север", "Хлеб", 50.0),
        ("Юг", "Хлеб", 60.0),
    ]
    .into_iter()
    .enumerate()
    {
        let row = u32::try_from(row)? + 1;
        sheet.set(cell(3, row), region);
        sheet.set(cell(4, row), product);
        sheet.set(cell(5, row), sales);
    }

    drawings(&mut sheet)?;

    let default_field = PivotField {
        default_subtotal: true,
        ..PivotField::default()
    };
    sheet.pivot_tables.push(PivotTable {
        name: "ПробнаяСводная".into(),
        cache_id: 1,
        location: Some(Range::new(at("P1")?, at("T12")?)),
        first_data_row: 1,
        first_data_col: 1,
        fields: vec![
            PivotField {
                axis: PivotAxis::Row,
                ..default_field.clone()
            },
            PivotField {
                axis: PivotAxis::Column,
                ..default_field.clone()
            },
            PivotField {
                data_field: true,
                ..default_field
            },
        ],
        row_fields: vec![0],
        column_fields: vec![1],
        data_fields: vec![DataField {
            name: Some("Сумма продаж".into()),
            field: 2,
            subtotal: Subtotal::Sum,
            number_format: None,
        }],
        row_grand_totals: true,
        column_grand_totals: true,
        ..PivotTable::default()
    });
    // The question Excel answers about the report it lays out itself.
    sheet.set(at("P16")?, "GETPIVOTDATA по разложенному отчёту:");
    sheet.set(
        at("P17")?,
        formula(r#"GETPIVOTDATA("Продажи",$P$1,"Регион","Север")"#),
    );
    sheet.set(at("P18")?, "ожидаем 80 (10+20+50)");
    book.add_sheet(sheet)?;
    recalculate_on_load(&mut book);
    book.pivot_caches.push(PivotCache {
        id: 1,
        source: CacheSource {
            sheet: Some("Данные".into()),
            range: Some(Range::new(at("D1")?, at("F7")?)),
            name: None,
        },
        fields: ["Регион", "Товар", "Продажи"]
            .into_iter()
            .map(|name| CacheField {
                name: name.to_owned(),
                ..CacheField::default()
            })
            .collect(),
        ..PivotCache::default()
    });
    excelerate::writer::xlsx::write_xlsx(&book, format!("{dir}/probe-objects.xlsx"))?;
    Ok(())
}

/// The 2016 charts and the shapes of the objects probe.
fn drawings(sheet: &mut Worksheet) -> Fallible {
    let dimensions = |numeric_at: &str, labels_at: &str| {
        vec![
            Dimension {
                role: DimensionRole::Categories,
                numeric: false,
                formula: Some(labels_at.to_owned()),
                levels: Vec::new(),
            },
            Dimension {
                role: DimensionRole::Values,
                numeric: true,
                formula: Some(numeric_at.to_owned()),
                levels: Vec::new(),
            },
        ]
    };
    let anchor = |col: u32, row: u32| Anchor::TwoCell {
        from: Marker {
            col: Col::new(col).unwrap_or_default(),
            row: Row::new(row).unwrap_or_default(),
            ..Marker::default()
        },
        to: Marker {
            col: Col::new(col + 6).unwrap_or_default(),
            row: Row::new(row + 14).unwrap_or_default(),
            ..Marker::default()
        },
        edit_as: None,
    };
    for (index, (layout, name)) in [
        (SeriesLayout::Waterfall, "Каскад"),
        (SeriesLayout::Funnel, "Воронка"),
        (SeriesLayout::Treemap, "Дерево"),
    ]
    .into_iter()
    .enumerate()
    {
        let index = u32::try_from(index)?;
        let mut chart = ChartEx::new(
            layout,
            dimensions("Данные!$B$2:$B$6", "Данные!$A$2:$A$6"),
            anchor(8, index * 16 + 1),
        );
        name.clone_into(&mut chart.name);
        chart.title = Some(ChartText::text(name));
        sheet.extended_charts.push(chart);
    }

    let mut box_shape = Shape::new("rect", anchor(1, 9));
    box_shape.name = "Прямоугольник пробы".into();
    box_shape.text = "Первая строка\nвторая строка".into();
    sheet.shapes.push(box_shape);
    let mut arrow = Shape::new("rightArrow", anchor(1, 20));
    arrow.name = "Стрелка пробы".into();
    arrow.description = "альтернативный текст".into();
    sheet.shapes.push(arrow);

    Ok(())
}

/// The little table the isolated probes draw from.
fn data(sheet: &mut Worksheet) -> Fallible {
    sheet.set(at("A1")?, "Этап");
    sheet.set(at("B1")?, "Сумма");
    for (row, (stage, amount)) in [
        ("Лиды", 1000.0),
        ("Звонки", 700.0),
        ("Встречи", 400.0),
        ("Счета", 250.0),
        ("Оплаты", 120.0),
    ]
    .into_iter()
    .enumerate()
    {
        let row = u32::try_from(row)? + 1;
        sheet.set(cell(0, row), stage);
        sheet.set(cell(1, row), amount);
    }
    Ok(())
}

/// One question per workbook, so a rejected part cannot take an innocent one
/// with it: Excel dropped the whole drawing of `probe-objects`, and the
/// shapes in it were never the question.
fn isolated(dir: &str) -> Fallible {
    let frame = Anchor::TwoCell {
        from: Marker {
            col: Col::new(3).unwrap_or_default(),
            row: Row::new(1).unwrap_or_default(),
            ..Marker::default()
        },
        to: Marker {
            col: Col::new(11).unwrap_or_default(),
            row: Row::new(16).unwrap_or_default(),
            ..Marker::default()
        },
        edit_as: None,
    };
    let dimensions = |labels: &str, values: &str| {
        vec![
            Dimension {
                role: DimensionRole::Categories,
                numeric: false,
                formula: Some(labels.to_owned()),
                levels: Vec::new(),
            },
            Dimension {
                role: DimensionRole::Values,
                numeric: true,
                formula: Some(values.to_owned()),
                levels: Vec::new(),
            },
        ]
    };

    // 1. A waterfall reading the cells directly, with no title at all.
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Каскад")?;
    data(&mut sheet)?;
    sheet.extended_charts.push(ChartEx::new(
        SeriesLayout::Waterfall,
        dimensions("Каскад!$A$2:$A$6", "Каскад!$B$2:$B$6"),
        frame,
    ));
    book.add_sheet(sheet)?;
    excelerate::writer::xlsx::write_xlsx(&book, format!("{dir}/probe-cx-1-простой.xlsx"))?;

    // 2. The same, reading through the hidden names Excel itself writes.
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Каскад")?;
    data(&mut sheet)?;
    sheet.extended_charts.push(ChartEx::new(
        SeriesLayout::Waterfall,
        dimensions("_xlchart.v1.0", "_xlchart.v1.1"),
        frame,
    ));
    book.add_sheet(sheet)?;
    for (name, formula) in [
        ("_xlchart.v1.0", "Каскад!$A$2:$A$6"),
        ("_xlchart.v1.1", "Каскад!$B$2:$B$6"),
    ] {
        book.defined_names.push(excelerate::model::DefinedName {
            name: name.to_owned(),
            sheet: None,
            formula: formula.to_owned(),
            hidden: true,
        });
    }
    excelerate::writer::xlsx::write_xlsx(&book, format!("{dir}/probe-cx-2-имена.xlsx"))?;

    // 3. The same as the first, with the fallback taken out again: if this one
    // opens too, the namespace was the whole story.
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Каскад")?;
    data(&mut sheet)?;
    sheet.extended_charts.push(ChartEx::new(
        SeriesLayout::Waterfall,
        dimensions("Каскад!$A$2:$A$6", "Каскад!$B$2:$B$6"),
        frame,
    ));
    book.add_sheet(sheet)?;
    let mut bytes = Vec::new();
    excelerate::writer::xlsx::write_xlsx_to(&book, std::io::Cursor::new(&mut bytes))?;
    let mut book = excelerate::reader::xlsx::read_xlsx_from(std::io::Cursor::new(bytes))?;
    for part in &mut book.parts {
        if part.path.starts_with("xl/drawings/drawing")
            && let Ok(text) = core::str::from_utf8(&part.data)
            && let Some(start) = text.find("<mc:Fallback>")
            && let Some(end) = text.find("</mc:Fallback>")
        {
            let without = format!(
                "{}{}",
                &text[..start],
                &text[end + "</mc:Fallback>".len()..]
            );
            part.data = without.into_bytes();
        }
    }
    excelerate::writer::xlsx::write_xlsx(&book, format!("{dir}/probe-cx-5-без-запасной.xlsx"))?;

    // 4. The same with a title, which is now written as typed text.
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Каскад")?;
    data(&mut sheet)?;
    let mut chart = ChartEx::new(
        SeriesLayout::Waterfall,
        dimensions("Каскад!$A$2:$A$6", "Каскад!$B$2:$B$6"),
        frame,
    );
    chart.title = Some(ChartText::text("Заголовок"));
    sheet.extended_charts.push(chart);
    book.add_sheet(sheet)?;
    excelerate::writer::xlsx::write_xlsx(&book, format!("{dir}/probe-cx-3-заголовок.xlsx"))?;

    shape_only(dir, frame)
}

/// The fourth of the isolated probes: a shape and nothing else.
fn shape_only(dir: &str, frame: Anchor) -> Fallible {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Фигуры")?;
    data(&mut sheet)?;
    let mut box_shape = Shape::new("rect", frame);
    box_shape.name = "Прямоугольник".into();
    box_shape.text = "Первая строка\nвторая строка".into();
    sheet.shapes.push(box_shape);
    book.add_sheet(sheet)?;
    excelerate::writer::xlsx::write_xlsx(&book, format!("{dir}/probe-cx-4-фигура.xlsx"))?;
    Ok(())
}

/// `probe-edited.xlsx`: a workbook Excel itself wrote, with one 2016 chart and
/// one shape changed through the model, to see what Excel makes of an edit
/// spliced into its own parts.
fn edited(dir: &str) -> Fallible {
    let source = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/chart1.xlsx");
    let mut book = excelerate::reader::xlsx::read_xlsx(source)?;
    let Some(sheet) = book.sheet_mut(0) else {
        return Ok(());
    };
    if let Some(chart) = sheet.extended_charts.first_mut() {
        chart.name = "Каскад после правки".into();
        chart.title = Some(ChartText::text("Заголовок, написанный нами"));
        if let Some(series) = chart.series.first_mut() {
            series.name = Some(ChartText::text("Ряд, названный нами"));
        }
    }
    if let Some(shape) = sheet.shapes.first_mut() {
        shape.name = "Фигура после правки".into();
        shape.text = "Текст, написанный нами".into();
    }
    excelerate::writer::xlsx::write_xlsx(&book, format!("{dir}/probe-edited.xlsx"))?;
    Ok(())
}

/// The same array formula in the three formats that can say how far it
/// reaches, and a second, simpler one: Excel 2019 fell over on the first xls,
/// and a formula with no function in it says whether the function or the
/// record is what it choked on.
fn arrays(dir: &str) -> Fallible {
    let write = |book: &Spreadsheet, stem: &str| -> Fallible {
        excelerate::writer::xlsx::write_xlsx(book, format!("{dir}/{stem}.xlsx"))?;
        excelerate::writer::xls::write_xls(book, format!("{dir}/{stem}.xls"))?;
        excelerate::writer::ods::write_ods(book, format!("{dir}/{stem}.ods"))?;
        Ok(())
    };
    for (stem, text, what) in [
        (
            "probe-array",
            "TRANSPOSE(A2:A4)",
            "В B2:B4 одна формула массива TRANSPOSE(A2:A4)",
        ),
        (
            "probe-array-простой",
            "A2:A4*2",
            "В B2:B4 одна формула массива A2:A4*2, без функций",
        ),
    ] {
        let mut book = Spreadsheet::empty();
        let mut sheet = Worksheet::new("Массив")?;
        sheet.set(at("A1")?, what);
        for row in 1..=3u32 {
            sheet.set(cell(0, row), f64::from(row) * 10.0);
            sheet.set(cell(1, row), formula(text));
        }
        sheet.array_formulas.push(Range::new(at("B2")?, at("B4")?));
        book.add_sheet(sheet)?;
        recalculate(&mut book, None, &Options::default());
        recalculate_on_load(&mut book);
        write(&book, stem)?;
    }
    Ok(())
}

/// A sheet locked with a password hashed the way Excel hashes one.
fn protected(dir: &str) -> Fallible {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Защита")?;
    sheet.set(at("A1")?, format!("Лист защищён паролем «{PASSWORD}»"));
    sheet.set(
        at("A2")?,
        "Снимите защиту этим паролем: примет ли Excel наш хеш?",
    );
    sheet.protection.sheet = Some(true);
    sheet.protection.password = PasswordHash::new(PASSWORD);
    book.add_sheet(sheet)?;
    excelerate::writer::xlsx::write_xlsx(&book, format!("{dir}/probe-protected.xlsx"))?;
    Ok(())
}

/// Asks Excel to compute everything when it opens the file, rather than
/// trusting the values stored beside the formulas - here, none.
fn recalculate_on_load(book: &mut Spreadsheet) {
    book.calculation_properties
        .push(("fullCalcOnLoad".to_owned(), "1".to_owned()));
}

/// What to do with the files, beside them.
fn instructions(dir: &str) -> Fallible {
    let text = ("\u{feff}Третий заход\r\n\
        ============\r\n\r\n\
        Спасибо, второй заход дал три ответа:\r\n\
        \x20 - фигура, написанная нами с нуля, открылась молча: рисунок и якоря в\r\n\
        \x20   порядке, виновата была только рамка диаграммы 2016 года;\r\n\
        \x20 - все три варианта диаграммы Excel отверг одинаково, значит дело не в\r\n\
        \x20   заголовке и не в скрытых именах. Теперь рамка повторяет то, что пишет\r\n\
        \x20   сам Excel: xmlns:r объявлен прямо на <cx:chart> и добавлен mc:Fallback;\r\n\
        \x20 - xls ронял Excel и с функцией, и без неё: ячейки внутри области массива\r\n\
        \x20   мы писали значениями, а BIFF требует в каждой запись FORMULA с\r\n\
        \x20   указателем на верхний левый угол. Исправлено.\r\n\
        \x20 И ещё: по вашим ответам про форматы теперь понимаются русские коды дат\r\n\
        \x20 (ДД.ММ.ГГГГ), а месяцы приведены к тому, что показывает Excel.\r\n\r\n\
        Вопросы этого захода.\r\n\r\n\
        probe-cx-1-простой.xlsx - каскадная диаграмма с исправленной рамкой.\r\n\
        probe-cx-5-без-запасной.xlsx - та же рамка, но без mc:Fallback. Если первый\r\n\
        \x20  откроется, а этот нет - дело было в запасной ветке; если оба откроются -\r\n\
        \x20  хватало объявления пространства имён.\r\n\
        probe-cx-2-имена.xlsx, probe-cx-3-заголовок.xlsx - те же две проверки поверх\r\n\
        \x20  исправленной рамки: скрытые имена и заголовок.\r\n\
        probe-cx-4-фигура.xlsx - фигура, для порядка (в прошлый раз открылась молча).\r\n\r\n\
        probe-array.xls и probe-array-простой.xls - ТОТ САМЫЙ xls, из-за которого\r\n\
        \x20  Excel падал. Если и теперь уронит - больше не открывайте, я уберу\r\n\
        \x20  запись ARRAY из писателя совсем.\r\n\
        \x20  Их же .xlsx и .ods - для порядка.\r\n\r\n\
        probe-formulas.xlsx - форматы в третий раз: русские числовые форматы\r\n\
        \x20  (# ##0,00) мы пока не понимаем, хочу увидеть, что Excel отвечает на них\r\n\
        \x20  и на время (Ч:ММ:СС), чтобы дописать таблицу.\r\n\r\n\
        probe-objects.xlsx, probe-edited.xlsx - с починенными стилями и рамкой:\r\n\
        \x20  интересно, вернутся ли срезы в probe-edited и переживут ли открытие\r\n\
        \x20  три диаграммы 2016 года в probe-objects.\r\n\r\n\
        Как обычно: молча или с журналом, и сохранить с припиской -excel.\r\n\
        Пароль больше не проверяем - он принят.\r\n")
        .to_owned();
    std::fs::write(format!("{dir}/ЧИТАТЬ.txt"), text)?;
    Ok(())
}
