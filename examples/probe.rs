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
    vec![
        ("дробь: целое число гасит дробь?", r##"TEXT(2,"# ?/?")"##),
        ("дробь: 0,5 - печатает ли # ноль", r##"TEXT(0.5,"# ?/?")"##),
        (
            "дробь: 0,99 - округляется ли до 1",
            r##"TEXT(0.99,"# ?/?")"##,
        ),
        ("дробь: 1,25", r##"TEXT(1.25,"# ?/?")"##),
        ("дробь: 5,25 в трёх знаках", r##"TEXT(5.25,"# ???/???")"##),
        (
            "дробь: ближайшая со знаменателем до 999",
            r#"TEXT(PI(),"?/???")"#,
        ),
        ("дробь: фиксированный знаменатель", r##"TEXT(1.3,"# ?/8")"##),
        ("дробь: отрицательное", r##"TEXT(-1.75,"# ?/?")"##),
        (
            "локаль: символ валюты",
            r##"TEXT(1234.5,"#,##0.00 [$₽-419]")"##,
        ),
        (
            "локаль: полный месяц по-русски",
            r#"TEXT(DATE(2025,1,15),"[$-419]d mmmm yyyy")"#,
        ),
        (
            "локаль: сокращённый месяц",
            r#"TEXT(DATE(2025,1,15),"[$-419]mmm")"#,
        ),
        (
            "локаль: день недели",
            r#"TEXT(DATE(2025,1,15),"[$-419]dddd")"#,
        ),
        (
            "локаль: день недели сокращённо",
            r#"TEXT(DATE(2025,1,15),"[$-419]ddd")"#,
        ),
        (
            "локаль: одна буква месяца",
            r#"TEXT(DATE(2025,1,15),"[$-419]mmmmm")"#,
        ),
        (
            "регэксп: есть ли совпадение без учёта регистра",
            r#"_xlfn.REGEXTEST("Привет, мир","МИР",1)"#,
        ),
        (
            "регэксп: первое совпадение",
            r#"_xlfn.REGEXEXTRACT("тел 555-1234 и 555-9876","\d{3}-\d{4}")"#,
        ),
        (
            "регэксп: замена всех",
            r##"_xlfn.REGEXREPLACE("a1b2","\d","#")"##,
        ),
        (
            "регэксп: замена последнего",
            r#"_xlfn.REGEXREPLACE("a-b-c","-","+",-1)"#,
        ),
        (
            "регэксп: ссылка на группу",
            r#"_xlfn.REGEXREPLACE("Иванов, Иван","(\w+), (\w+)","$2 $1")"#,
        ),
        (
            "регэксп: кириллица в \\w",
            r#"_xlfn.REGEXTEST("Ёж","^\w+$")"#,
        ),
        (
            "регэксп: просмотр вперёд (у нас #ЗНАЧ!)",
            r#"_xlfn.REGEXTEST("x1","x(?=\d)")"#,
        ),
        ("доля от суммы", "_xlfn.PERCENTOF({1;3},{1;3;4})"),
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
/// reaches.
fn arrays(dir: &str) -> Fallible {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Массив")?;
    sheet.set(at("A1")?, "В B1:B3 одна формула массива TRANSPOSE(A1:A3)");
    for row in 1..=3u32 {
        sheet.set(cell(0, row), f64::from(row) * 10.0);
        sheet.set(cell(1, row), formula("TRANSPOSE(A2:A4)"));
    }
    sheet.array_formulas.push(Range::new(at("B2")?, at("B4")?));
    book.add_sheet(sheet)?;
    recalculate(&mut book, None, &Options::default());
    recalculate_on_load(&mut book);
    excelerate::writer::xlsx::write_xlsx(&book, format!("{dir}/probe-array.xlsx"))?;
    excelerate::writer::xls::write_xls(&book, format!("{dir}/probe-array.xls"))?;
    excelerate::writer::ods::write_ods(&book, format!("{dir}/probe-array.ods"))?;
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
    let text = format!(
        "\u{feff}Что с этим делать\r\n\
        =================\r\n\r\n\
        По каждому файлу нужно одно: открылся молча или Excel просил восстановить\r\n\
        содержимое (и что он написал в журнале восстановления). Потом сохраните файл\r\n\
        в том же формате под новым именем - с припиской -excel - и скажите, куда.\r\n\r\n\
        probe-formulas.xlsx - вопросы формулами. Excel посчитает их сам при открытии.\r\n\
        \x20  Лист «Скаляры»: столбец C - ответ Excel, D - наш. Лист «Массивы»:\r\n\
        \x20  формула в столбце B каждого блока, наш ответ - в столбце M.\r\n\
        \x20  Если какие-то функции ваш Excel не знает, он покажет #ИМЯ? - это тоже ответ.\r\n\r\n\
        probe-objects.xlsx - то, что мы пишем из модели: три диаграммы 2016 года\r\n\
        \x20  (каскад, воронка, дерево), две фигуры и сводная таблица. Вопросы:\r\n\
        \x20  1) открылся ли молча; 2) нарисовались ли диаграммы (скриншот, если не лень);\r\n\
        \x20  3) разложила ли Excel сводную в P1:T12; 4) что в P17 - там GETPIVOTDATA\r\n\
        \x20  по этой сводной, ждём 80.\r\n\r\n\
        probe-edited.xlsx - книга, написанная самим Excel, где мы поменяли заголовок\r\n\
        \x20  каскадной диаграммы и текст фигуры на первом листе. Вопрос тот же:\r\n\
        \x20  молча ли открылась и на месте ли остальные 130 диаграмм.\r\n\r\n\
        probe-array.xlsx / .xls / .ods - одна формула массива в трёх форматах.\r\n\
        \x20  Вопрос: показывает ли Excel её в фигурных скобках и та ли у неё область\r\n\
        \x20  (B2:B4). В .xls и .ods особенно интересно.\r\n\r\n\
        probe-protected.xlsx - лист защищён паролем «{PASSWORD}», хеш посчитан нами.\r\n\
        \x20  Вопрос: снимает ли Excel защиту этим паролем.\r\n"
    );
    std::fs::write(format!("{dir}/ЧИТАТЬ.txt"), text)?;
    Ok(())
}
