//! Charts: what a chart says about itself, and that a rewrite keeps it - as
//! bytes when nothing changed, from the model when something did.
//!
//! `chart1.xlsx` is a dashboard saved by Excel: 130 classic charts over 14
//! sheets, 28 of them inside groups of shapes, and a waterfall per sheet.
//! `chart2.xlsx` is one combination chart, Russian throughout. The fixture
//! `fixtures/chart.xlsx` is written by excelize, which puts the chart
//! namespace on the root as a default rather than behind `c:`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::coordinate::{Col, Row};
use excelerate::edit::insert_rows;
use excelerate::model::Spreadsheet;
use excelerate::model::chart::{
    Anchor, AxisKind, BarDirection, Chart, ChartAxis, ChartText, DataSource, Grouping,
    LegendPosition, Marker, Plot, PlotKind, Series, SeriesLayout, Title,
};
use excelerate::reader::xlsx::read_xlsx_from;
use excelerate::writer::xlsx::write_xlsx_to;
use std::io::Cursor;

fn open(name: &str) -> Spreadsheet {
    let path = format!("{}/tests/{name}", env!("CARGO_MANIFEST_DIR"));
    read_xlsx_from(Cursor::new(std::fs::read(path).unwrap())).unwrap()
}

fn cycle(book: &Spreadsheet) -> Spreadsheet {
    let mut bytes = Vec::new();
    write_xlsx_to(book, Cursor::new(&mut bytes)).unwrap();
    read_xlsx_from(Cursor::new(bytes)).unwrap()
}

fn part<'a>(book: &'a Spreadsheet, path: &str) -> &'a [u8] {
    &book
        .parts
        .iter()
        .find(|p| p.path == path)
        .unwrap_or_else(|| panic!("{path} is in the package"))
        .data
}

fn chart_part(chart: &Chart) -> &str {
    chart.origin.as_ref().expect("the chart was read").part()
}

#[test]
fn a_combination_chart_is_read_whole() {
    let book = open("chart2.xlsx");
    let sheet = book.sheet(0).unwrap();
    assert_eq!(sheet.charts.len(), 1);
    let chart = &sheet.charts[0];

    assert_eq!(chart.name, "Диаграмма 2");
    let Anchor::TwoCell { from, to, edit_as } = chart.anchor else {
        panic!("a two-cell anchor, got {:?}", chart.anchor);
    };
    assert_eq!((from.col.index(), from.row.index()), (5, 3));
    assert_eq!((to.col.index(), to.row.index()), (16, 30));
    assert_eq!(edit_as, None);

    let title = chart.title.as_ref().unwrap().text.as_ref().unwrap();
    assert_eq!(
        *title,
        ChartText::Reference {
            formula: "Лист1!$A$3".into(),
            cache: Some("Объемы продаж филиала".into()),
        }
    );

    // Columns for the total, and stacked columns over them for the parts.
    let kinds: Vec<PlotKind> = chart.plots.iter().map(|p| p.kind).collect();
    let bar = |grouping| PlotKind::Bar {
        direction: BarDirection::Column,
        grouping,
        three_d: false,
    };
    assert_eq!(kinds, [bar(Grouping::Clustered), bar(Grouping::Stacked)]);
    assert_eq!(chart.plots[1].series.len(), 2);

    let total = &chart.plots[0].series[0];
    assert_eq!(
        total.name.as_ref().and_then(ChartText::shown),
        Some("Всего продано")
    );
    let Some(DataSource::Numbers {
        formula, points, ..
    }) = &total.values
    else {
        panic!("numbers, got {:?}", total.values);
    };
    assert_eq!(formula.as_deref(), Some("Лист1!$B$6:$B$18"));
    assert_eq!(points[..3], [(0, 149.0), (1, 152.0), (2, 165.0)]);
    let Some(DataSource::Numbers { format_code, .. }) = &total.categories else {
        panic!("dates are numbers");
    };
    assert_eq!(format_code.as_deref(), Some("m/d/yyyy"));
    // The fill is not modelled, but it is carried in place.
    assert!(total.markup.before_data.contains("00B150"));

    // Each plot has its own pair of axes, and each id a plot names exists.
    assert_eq!(chart.axes.len(), 4);
    for plot in &chart.plots {
        for id in &plot.axis_ids {
            assert!(chart.axes.iter().any(|a| a.id == *id), "axis {id}");
        }
    }
    assert!(chart.axes.iter().any(|a| a.kind == AxisKind::Date));
    assert_eq!(chart.legend.as_ref().unwrap().position, LegendPosition::Top);
}

/// Counted independently of this crate, from the drawings and their
/// relationships.
#[test]
fn every_chart_of_a_dashboard_is_found() {
    let book = open("chart1.xlsx");
    let classic = [12, 8, 8, 7, 10, 10, 10, 12, 8, 8, 7, 10, 10, 10];
    for (index, expected) in classic.iter().enumerate() {
        let sheet = book.sheet(index).unwrap();
        assert_eq!(sheet.charts.len(), *expected, "sheet {}", sheet.title());
        assert_eq!(sheet.extended_charts.len(), 1, "sheet {}", sheet.title());

        let waterfall = &sheet.extended_charts[0];
        assert_eq!(waterfall.series[0].layout, SeriesLayout::Waterfall);
        // Excel puts a hidden name where the range belongs.
        let dimension = &waterfall.series[0].dimensions[0];
        assert!(
            dimension
                .formula
                .as_deref()
                .unwrap()
                .starts_with("_xlchart.")
        );
        let range = dimension.reference(&book.defined_names).unwrap();
        assert!(
            range.contains('!') && !range.starts_with("_xlchart"),
            "{range}"
        );
    }
    let total: usize = book.sheets().iter().map(|s| s.charts.len()).sum();
    assert_eq!(total, 130, "one chart per chart part");
}

#[test]
fn an_untouched_chart_goes_back_as_it_came() {
    for name in ["chart1.xlsx", "chart2.xlsx"] {
        let book = open(name);
        let back = cycle(&book);
        for (before, after) in book.sheets().iter().zip(back.sheets()) {
            assert_eq!(before.charts, after.charts, "{name}: {}", before.title());
            assert_eq!(before.extended_charts, after.extended_charts);
            for chart in &before.charts {
                assert!(chart.is_unchanged());
                let path = chart_part(chart);
                assert_eq!(part(&book, path), part(&back, path), "{name}: {path}");
            }
        }
    }
}

#[test]
fn a_changed_chart_is_written_from_the_model() {
    let mut book = open("chart2.xlsx");
    let original = book.sheet(0).unwrap().charts[0].clone();
    let chart = &mut book.sheet_mut(0).unwrap().charts[0];
    chart.title = Some(Title {
        text: Some(ChartText::text("Продажи по годам")),
        ..chart.title.clone().unwrap()
    });
    chart.plots[0].series[0].values = Some(DataSource::numbers("Лист1!$B$6:$B$17"));
    chart.axes[1].max = Some(250.0);
    let edited = chart.clone();
    assert!(!edited.is_unchanged());

    let back = cycle(&book);
    let after = &back.sheet(0).unwrap().charts[0];
    let path = chart_part(&original);
    assert_ne!(
        part(&book, path),
        part(&back, path),
        "the part was rewritten"
    );

    assert_eq!(
        after.title.as_ref().unwrap().text.as_ref().unwrap().shown(),
        Some("Продажи по годам")
    );
    // Everything else the model holds survives the rewrite exactly, carried
    // formatting included.
    assert_eq!(after.plots, edited.plots);
    assert_eq!(after.axes, edited.axes);
    assert_eq!(after.legend, edited.legend);
    assert_eq!(after.markup, edited.markup);
    assert_eq!(
        (&after.name, after.anchor),
        (&original.name, original.anchor)
    );
}

/// excelize binds the chart namespace as the default, so the carried markup
/// has no prefix and a rewrite must not give it one.
#[test]
fn a_chart_under_a_default_namespace_is_rewritten_in_it() {
    let mut book = open("fixtures/chart.xlsx");
    let chart = &mut book.sheet_mut(0).unwrap().charts[0];
    assert_eq!(
        chart.plots[0].series[0].values.as_ref().unwrap().formula(),
        Some("Sheet1!$B$2:$B$5")
    );
    chart.plots[0].series[0].name = Some(ChartText::text("Выручка"));
    let edited = chart.clone();

    let back = cycle(&book);
    let text = String::from_utf8(part(&back, chart_part(&edited)).to_vec()).unwrap();
    assert!(
        text.contains("<chartSpace xmlns=") && !text.contains("<c:ser>"),
        "{text}"
    );
    let after = &back.sheet(0).unwrap().charts[0];
    assert_eq!(after.plots, edited.plots);
    assert_eq!(after.axes, edited.axes);
}

#[test]
fn a_chart_made_in_code_is_written() {
    let mut book = Spreadsheet::new();
    let mut plot = Plot::new(PlotKind::Line {
        grouping: Grouping::Standard,
        three_d: false,
    });
    plot.series.push(Series {
        name: Some(ChartText::Reference {
            formula: "Worksheet!$B$1".into(),
            cache: None,
        }),
        categories: Some(DataSource::strings("Worksheet!$A$2:$A$5")),
        values: Some(DataSource::numbers("Worksheet!$B$2:$B$5")),
        ..Series::default()
    });
    plot.axis_ids = vec![1, 2];
    let anchor = Anchor::TwoCell {
        from: Marker {
            col: Col::new(3).unwrap(),
            ..Marker::default()
        },
        to: Marker {
            col: Col::new(10).unwrap(),
            row: Row::new(18).unwrap(),
            ..Marker::default()
        },
        edit_as: None,
    };
    book.sheet_mut(0).unwrap().charts.push(Chart {
        name: "Выручка".into(),
        anchor,
        title: Some(Title {
            text: Some(ChartText::text("Выручка\nпо кварталам")),
            markup: String::new(),
        }),
        plots: vec![plot.clone()],
        axes: vec![ChartAxis::category(1, 2), ChartAxis::value(2, 1)],
        legend: Some(excelerate::model::chart::Legend::default()),
        ..Chart::default()
    });

    let back = cycle(&book);
    let sheet = back.sheet(0).unwrap();
    assert_eq!(sheet.charts.len(), 1);
    let chart = &sheet.charts[0];
    assert_eq!((chart.name.as_str(), chart.anchor), ("Выручка", anchor));
    assert_eq!(
        chart.title.as_ref().unwrap().text.as_ref().unwrap().shown(),
        Some("Выручка\nпо кварталам")
    );
    assert_eq!(chart.plots.len(), 1);
    assert_eq!(chart.plots[0].kind, plot.kind);
    assert_eq!(chart.plots[0].series[0].values, plot.series[0].values);
    assert_eq!(chart.plots[0].series[0].name, plot.series[0].name);
    let kinds: Vec<AxisKind> = chart.axes.iter().map(|a| a.kind).collect();
    assert_eq!(kinds, [AxisKind::Category, AxisKind::Value]);
    // Read back, it is an ordinary chart: a second write changes nothing.
    assert!(chart.is_unchanged());
    assert_eq!(cycle(&back).sheet(0).unwrap().charts, sheet.charts);
}

#[test]
fn a_new_chart_joins_a_drawing_that_already_has_charts() {
    let mut book = open("chart2.xlsx");
    let mut copy = book.sheet(0).unwrap().charts[0].clone();
    copy.origin = None;
    copy.name = "Копия".into();
    book.sheet_mut(0).unwrap().charts.push(copy.clone());

    let back = cycle(&book);
    let charts = &back.sheet(0).unwrap().charts;
    assert_eq!(charts.len(), 2);
    let (old, new) = (&charts[0], &charts[1]);
    assert_eq!(old.name, "Диаграмма 2");
    assert!(old.is_unchanged());
    assert_eq!(new.name, "Копия");
    assert_ne!(chart_part(old), chart_part(new), "a part of its own");
    assert_eq!(new.plots, copy.plots);
    assert_eq!(new.axes, copy.axes);

    // Two objects of one drawing sharing an id is a file Excel repairs.
    let drawing =
        String::from_utf8(part(&back, new.origin.as_ref().unwrap().drawing()).to_vec()).unwrap();
    let mut ids: Vec<&str> = drawing
        .split("cNvPr id=\"")
        .skip(1)
        .filter_map(|s| s.split('"').next())
        .collect();
    let count = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), count, "object ids are unique: {drawing}");
}

#[test]
fn a_removed_chart_leaves_its_neighbours_alone() {
    let mut book = open("chart1.xlsx");
    let removed = book.sheet_mut(1).unwrap().charts.remove(0);
    let back = cycle(&book);

    let before = &book.sheet(1).unwrap().charts;
    let after = &back.sheet(1).unwrap().charts;
    assert_eq!(after.len(), before.len());
    assert!(!after.iter().any(|c| chart_part(c) == chart_part(&removed)));
    for (b, a) in before.iter().zip(after) {
        assert_eq!(b.name, a.name);
        assert_eq!(b.anchor, a.anchor);
        assert_eq!(part(&book, chart_part(b)), part(&back, chart_part(a)));
    }
    // The other sheets did not notice.
    assert_eq!(book.sheet(2).unwrap().charts, back.sheet(2).unwrap().charts);
}

#[test]
fn a_moved_chart_keeps_its_part() {
    let mut book = open("chart2.xlsx");
    let anchor = Anchor::OneCell {
        from: Marker {
            col: Col::new(1).unwrap(),
            row: Row::new(40).unwrap(),
            ..Marker::default()
        },
        width: 5_000_000,
        height: 3_000_000,
    };
    book.sheet_mut(0).unwrap().charts[0].anchor = anchor;

    let back = cycle(&book);
    let chart = &back.sheet(0).unwrap().charts[0];
    assert_eq!(chart.anchor, anchor);
    let path = chart_part(chart);
    assert_eq!(part(&book, path), part(&back, path), "only the frame moved");
}

#[test]
fn inserted_rows_move_the_chart_and_what_it_reads() {
    let mut book = open("chart2.xlsx");
    insert_rows(&mut book, 0, Row::from_one_based(1).unwrap(), 2).unwrap();
    let chart = &book.sheet(0).unwrap().charts[0];
    let Anchor::TwoCell { from, .. } = chart.anchor else {
        panic!("a two-cell anchor");
    };
    assert_eq!(from.row.index(), 5);
    assert_eq!(
        chart.plots[0].series[0].values.as_ref().unwrap().formula(),
        Some("Лист1!$B$8:$B$20")
    );
    // The bytes moved in step, so the chart still goes back as bytes.
    assert!(chart.is_unchanged());
    assert_eq!(
        cycle(&book).sheet(0).unwrap().charts[0].anchor,
        chart.anchor
    );
}

#[test]
fn a_plot_with_no_axes_is_refused() {
    let mut book = Spreadsheet::new();
    let mut plot = Plot::new(PlotKind::Bar {
        direction: BarDirection::Bar,
        grouping: Grouping::Clustered,
        three_d: false,
    });
    plot.series.push(Series {
        values: Some(DataSource::numbers("Worksheet!$A$1:$A$3")),
        ..Series::default()
    });
    book.sheet_mut(0).unwrap().charts.push(Chart {
        plots: vec![plot],
        ..Chart::default()
    });
    let mut bytes = Vec::new();
    assert!(write_xlsx_to(&book, Cursor::new(&mut bytes)).is_err());
}
