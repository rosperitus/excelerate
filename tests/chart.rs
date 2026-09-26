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
    Anchor, AxisKind, BarDirection, Chart, ChartAxis, ChartColor, ChartEx, ChartText, ColorBase,
    ColorTransform, DataLabels, DataSource, Dimension, DimensionRole, ExSeries, Fill, Grouping,
    LabelPosition, LegendPosition, LineFormat, Marker, MarkerSymbol, Plot, PlotKind, Series,
    SeriesLayout, ShapeFormat, Title,
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
    // The fill is modelled; what the model does not name is carried in place.
    assert_eq!(
        total.format.as_ref().unwrap().fill,
        Some(Fill::Solid(ChartColor::rgb(0x00_B150)))
    );
    assert!(total.markup.after_format.contains("invertIfNegative"));

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

/// What a 2016 chart says, without where it was read from and without the
/// markup typed text is carried in: a title written as plain text comes back
/// as the `DrawingML` that spells it.
fn said(chart: &ChartEx) -> (String, Anchor, Option<ChartText>, Vec<ExSeries>) {
    let plain = |text: &ChartText| match text {
        ChartText::Text { text, .. } => ChartText::text(text),
        other @ ChartText::Reference { .. } => other.clone(),
    };
    (
        chart.name.clone(),
        chart.anchor,
        chart.title.as_ref().map(plain),
        chart
            .series
            .iter()
            .map(|series| ExSeries {
                name: series.name.as_ref().map(plain),
                ..series.clone()
            })
            .collect(),
    )
}

#[test]
fn a_changed_waterfall_keeps_its_formatting() {
    let mut book = open("chart1.xlsx");
    let sheet = book.sheet_mut(0).unwrap();
    let chart = &mut sheet.extended_charts[0];
    let path = chart.part.clone();
    chart.name = "Bridge & walk".into();
    chart.title = Some(ChartText::text("Profit bridge"));
    chart.series[0].name = Some(ChartText::Reference {
        formula: "Sheet1!$A$1".into(),
        cache: Some("2025".into()),
    });
    chart.series[0].dimensions[1].formula = Some("'Q1'!$C$2:$C$9".into());
    if let Anchor::TwoCell { from, to, .. } = &mut chart.anchor {
        from.row = Row::new(from.row.index() + 3).unwrap();
        to.row = Row::new(to.row.index() + 3).unwrap();
    }
    let wanted = said(chart);

    let back = cycle(&book);
    let after = &back.sheet(0).unwrap().extended_charts[0];
    assert_eq!(said(after), wanted);
    assert_eq!(after.part, path, "the part is edited, not replaced");
    let text = std::str::from_utf8(part(&back, &path)).unwrap();
    assert!(text.contains("<cx:dataPt idx=\"7\">"), "point colours stay");
    assert!(text.contains("<cx:subtotals>"), "subtotal bars stay");
    // Every other sheet's waterfall is untouched.
    for before in book.sheets()[1..].iter().flat_map(|s| &s.extended_charts) {
        assert_eq!(part(&book, &before.part), part(&back, &before.part));
    }
}

#[test]
fn a_removed_waterfall_leaves_the_classic_charts() {
    let mut book = open("chart1.xlsx");
    book.sheet_mut(2).unwrap().extended_charts.clear();
    let back = cycle(&book);
    let sheet = back.sheet(2).unwrap();
    assert!(sheet.extended_charts.is_empty());
    assert_eq!(sheet.charts, book.sheet(2).unwrap().charts);
}

#[test]
fn a_funnel_made_in_code_is_written() {
    let mut book = open("chart2.xlsx");
    let dimensions = vec![
        Dimension {
            role: DimensionRole::Categories,
            numeric: false,
            formula: Some("Sheet1!$A$2:$A$5".into()),
            levels: vec![vec![(0, "Leads".into()), (1, "Calls".into())]],
        },
        Dimension {
            role: DimensionRole::Values,
            numeric: true,
            formula: Some("Sheet1!$B$2:$B$5".into()),
            levels: Vec::new(),
        },
    ];
    let from = Marker {
        col: Col::new(10).unwrap(),
        row: Row::new(2).unwrap(),
        ..Marker::default()
    };
    let to = Marker {
        col: Col::new(16).unwrap(),
        row: Row::new(18).unwrap(),
        ..Marker::default()
    };
    let mut funnel = ChartEx::new(
        SeriesLayout::Funnel,
        dimensions,
        Anchor::TwoCell {
            from,
            to,
            edit_as: None,
        },
    );
    funnel.name = "Pipeline".into();
    funnel.title = Some(ChartText::text("Sales pipeline"));
    let wanted = said(&funnel);
    let sheet = book.sheet_mut(0).unwrap();
    sheet.extended_charts.push(funnel);
    let charts = sheet.charts.clone();

    let back = cycle(&book);
    let sheet = back.sheet(0).unwrap();
    assert_eq!(sheet.extended_charts.len(), 1);
    assert_eq!(said(&sheet.extended_charts[0]), wanted);
    assert_eq!(sheet.charts, charts, "the classic chart beside it stays");
    // And it is a fixed point from here.
    let again = cycle(&back);
    assert_eq!(
        again.sheet(0).unwrap().extended_charts,
        sheet.extended_charts
    );
}

/// The chart whose part is `path`.
fn chart_at<'a>(book: &'a Spreadsheet, path: &str) -> &'a Chart {
    book.sheets()
        .iter()
        .flat_map(|s| &s.charts)
        .find(|c| c.origin.as_ref().is_some_and(|o| o.part() == path))
        .unwrap_or_else(|| panic!("a chart in {path}"))
}

fn chart_at_mut<'a>(book: &'a mut Spreadsheet, path: &str) -> &'a mut Chart {
    let sheet = (0..book.sheets().len())
        .find(|&i| {
            book.sheet(i)
                .unwrap()
                .charts
                .iter()
                .any(|c| chart_part(c) == path)
        })
        .unwrap_or_else(|| panic!("a chart in {path}"));
    book.sheet_mut(sheet)
        .unwrap()
        .charts
        .iter_mut()
        .find(|c| chart_part(c) == path)
        .unwrap()
}

fn text_of(book: &Spreadsheet, path: &str) -> String {
    String::from_utf8(part(book, path).to_vec()).unwrap()
}

#[test]
fn series_formatting_and_labels_are_read() {
    let book = open("chart2.xlsx");
    let chart = &book.sheet(0).unwrap().charts[0];
    let series = &chart.plots[1].series;
    assert_eq!(
        series[0].format.as_ref().unwrap().fill,
        Some(Fill::Solid(ChartColor::rgb(0xFF_C000)))
    );
    // The third series is an invisible spacer stacked on the second.
    let spacer = &series[1];
    assert_eq!(spacer.format.as_ref().unwrap().fill, Some(Fill::None));
    assert!(spacer.labels.as_ref().unwrap().deleted);
    let plot_labels = chart.plots[0].labels.as_ref().unwrap();
    assert!(!plot_labels.deleted && !plot_labels.show_value && !plot_labels.show_percent);

    let book = open("chart1.xlsx");
    // A pie that gives each slice its colour.
    let pie = &chart_at(&book, "xl/charts/chart4.xml").plots[0];
    assert!(matches!(pie.kind, PlotKind::Pie { .. }));
    let points = &pie.series[0].data_points;
    assert_eq!(
        points.iter().map(|p| p.index).collect::<Vec<_>>(),
        [0, 1, 2, 3, 4]
    );
    let slice = points[1].format.as_ref().unwrap();
    assert_eq!(slice.fill, Some(Fill::Solid(ChartColor::rgb(0x66_FFFF))));
    assert_eq!(
        slice.line,
        Some(LineFormat {
            fill: Some(Fill::None),
            width: Some(19050)
        })
    );
    // A gradient is not described, but it is known to be there.
    assert_eq!(points[0].format.as_ref().unwrap().fill, Some(Fill::Other));
    assert!(pie.labels.is_some());

    // A dashed line with no markers.
    let line = &chart_at(&book, "xl/charts/chart2.xml").plots;
    let planned = line
        .iter()
        .flat_map(|p| &p.series)
        .find(|s| s.name.as_ref().and_then(ChartText::shown) == Some("Planned"))
        .unwrap();
    let stroke = planned.format.as_ref().unwrap().line.as_ref().unwrap();
    assert_eq!(stroke.width, Some(19050));
    assert_eq!(stroke.fill, Some(Fill::Solid(ChartColor::rgb(0xA5_14F6))));
    assert_eq!(
        planned.marker.as_ref().unwrap().symbol,
        Some(MarkerSymbol::None)
    );

    // Labels above the points, and a theme colour with its transforms.
    let chart = chart_at(&book, "xl/charts/chart102.xml");
    let labels: Vec<&DataLabels> = chart
        .plots
        .iter()
        .flat_map(|p| p.series.iter().filter_map(|s| s.labels.as_ref()))
        .collect();
    assert!(
        labels
            .iter()
            .any(|l| l.show_value && l.position == Some(LabelPosition::Top)),
        "{labels:?}"
    );
    let book = open("fixtures/chart.xlsx");
    let labels = book.sheet(0).unwrap().charts[0].plots[0].series[0]
        .labels
        .clone()
        .unwrap();
    assert!(!labels.show_value && !labels.deleted);
}

#[test]
fn a_theme_colour_resolves_through_its_transforms() {
    let accent = |transforms| ChartColor {
        base: ColorBase::Scheme("accent1".into()),
        transforms,
    };
    // Office's accent 1 and Excel's own names for two of its shades.
    assert_eq!(accent(vec![]).resolve(None), Some(0x44_72C4));
    assert_eq!(
        accent(vec![ColorTransform::LumMod(75000)]).resolve(None),
        Some(0x2F_5597),
        "darker 25%"
    );
    // Excel rounds through its own HSL; a unit per channel apart is the same
    // colour.
    let lighter = accent(vec![
        ColorTransform::LumMod(60000),
        ColorTransform::LumOff(40000),
    ])
    .resolve(None)
    .unwrap();
    let excel: u32 = 0x8F_AADC;
    for shift in [16, 8, 0] {
        let channel = |c: u32| i64::from((c >> shift) & 0xFF);
        assert!(
            (channel(lighter) - channel(excel)).abs() <= 1,
            "lighter 40%: {lighter:06X}"
        );
    }
    assert_eq!(ChartColor::scheme("phClr").resolve(None), None);
}

#[test]
fn a_changed_series_colour_is_written_and_read_back() {
    let mut book = open("chart2.xlsx");
    let path = chart_part(&book.sheet(0).unwrap().charts[0]).to_owned();
    let before = text_of(&book, &path);
    let chart = &mut book.sheet_mut(0).unwrap().charts[0];
    let red = Some(Fill::Solid(ChartColor::rgb(0xFF_0000)));
    chart.plots[0].series[0].format.as_mut().unwrap().fill = red.clone();
    let labels = chart.plots[0].labels.as_mut().unwrap();
    labels.show_value = true;
    labels.position = Some(LabelPosition::OutsideEnd);
    let edited = chart.clone();

    let back = cycle(&book);
    let text = text_of(&back, &path);
    let after = &back.sheet(0).unwrap().charts[0];
    let total = &after.plots[0].series[0];
    assert_eq!(total.format.as_ref().unwrap().fill, red);
    assert!(!text.contains("00B150"), "the old colour is gone");
    // What was not modelled around it stays.
    assert_eq!(total.markup, edited.plots[0].series[0].markup);
    assert!(total.labels.as_ref().unwrap().deleted);
    let labels = after.plots[0].labels.as_ref().unwrap();
    assert!(labels.show_value && !labels.show_category_name);
    assert_eq!(labels.position, Some(LabelPosition::OutsideEnd));
    assert!(text.contains(r#"<c:showBubbleSize val="0"/>"#));
    // The series nobody touched keep their elements byte for byte.
    let other = r#"<c:spPr><a:solidFill><a:srgbClr val="FFC000"/></a:solidFill></c:spPr>"#;
    assert!(before.contains(other) && text.contains(other));
    assert_eq!(after.plots[1], edited.plots[1]);
}

#[test]
fn a_recoloured_slice_keeps_the_rest_of_its_formatting() {
    let path = "xl/charts/chart4.xml";
    let mut book = open("chart1.xlsx");
    let chart = chart_at_mut(&mut book, path);
    let theme = ChartColor {
        base: ColorBase::Scheme("accent2".into()),
        transforms: vec![ColorTransform::LumMod(50000)],
    };
    let points = &mut chart.plots[0].series[0].data_points;
    points[1].format.as_mut().unwrap().fill = Some(Fill::Solid(theme.clone()));
    // A line made thicker, and a point that had no formatting of its own.
    points[2]
        .format
        .as_mut()
        .unwrap()
        .line
        .as_mut()
        .unwrap()
        .width = Some(38100);
    points.push(excelerate::model::chart::DataPoint {
        index: 7,
        format: Some(ShapeFormat::solid(ChartColor::rgb(0x12_3456))),
        source: None,
    });

    let back = cycle(&book);
    let text = text_of(&back, path);
    let points = &chart_at(&back, path).plots[0].series[0].data_points;
    assert_eq!(
        points[1].format.as_ref().unwrap().fill,
        Some(Fill::Solid(theme))
    );
    assert_eq!(
        points[2].format.as_ref().unwrap().line,
        Some(LineFormat {
            fill: Some(Fill::None),
            width: Some(38100)
        })
    );
    assert_eq!(
        (
            points[5].index,
            points[5].format.as_ref().unwrap().fill.clone()
        ),
        (7, Some(Fill::Solid(ChartColor::rgb(0x12_3456))))
    );
    // The slice kept its outline, effects and extension, and the gradient of
    // the one before it is untouched.
    let slice = &text[text.find(r#"<c:idx val="1"/>"#).unwrap()..];
    let slice = &slice[..slice.find("</c:dPt>").unwrap()];
    assert!(slice.contains(r#"<a:ln w="19050"><a:noFill/></a:ln><a:effectLst/>"#));
    assert!(slice.contains("c16:uniqueId") && slice.contains("<c:bubble3D"));
    assert!(text.contains(r#"<a:gs pos="100000"><a:srgbClr val="8940D9"/>"#));
    assert!(
        text.contains(r#"<a:ln w="38100"><a:noFill/></a:ln>"#),
        "{text}"
    );
}

/// A chart changed elsewhere writes its series formatting as it was read.
#[test]
fn formatting_the_model_did_not_change_is_written_as_read() {
    for path in [
        "xl/charts/chart2.xml",
        "xl/charts/chart4.xml",
        "xl/charts/chart102.xml",
    ] {
        let mut book = open("chart1.xlsx");
        let before = text_of(&book, path);
        let chart = chart_at_mut(&mut book, path);
        chart.auto_title_deleted = !chart.auto_title_deleted;
        let edited = chart.clone();
        let back = cycle(&book);
        let text = text_of(&back, path);
        assert_ne!(before, text);
        let series = edited.plots.iter().flat_map(|p| &p.series);
        for s in series {
            let sources = s
                .format
                .iter()
                .filter_map(|f| f.source.as_deref())
                .chain(s.marker.iter().filter_map(|m| m.source.as_deref()))
                .chain(s.data_points.iter().filter_map(|p| p.source.as_deref()))
                .chain(s.labels.iter().filter_map(|l| l.source.as_deref()));
            for source in sources {
                assert!(text.contains(source), "{path}: {source}");
            }
        }
        let after = chart_at(&back, path);
        assert_eq!(after.plots, edited.plots, "{path}");
    }
}
