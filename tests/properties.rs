//! Document properties: read from a file another program wrote, kept byte for
//! byte while untouched, and written from the model once changed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::model::{PropertyValue, Spreadsheet, Worksheet};
use excelerate::reader::xlsx::read_xlsx_from;
use excelerate::writer::xlsx::write_xlsx_to;
use std::io::{Cursor, Read};

fn fixture() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/sample.xlsx"
    ))
    .unwrap()
}

fn write(book: &Spreadsheet) -> Vec<u8> {
    let mut out = Vec::new();
    write_xlsx_to(book, Cursor::new(&mut out)).unwrap();
    out
}

fn part(package: &[u8], name: &str) -> Option<String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(package)).unwrap();
    let mut file = zip.by_name(name).ok()?;
    let mut text = String::new();
    file.read_to_string(&mut text).unwrap();
    Some(text)
}

#[test]
fn the_properties_of_a_file_are_read() {
    let book = read_xlsx_from(Cursor::new(fixture())).unwrap();
    let props = &book.properties;
    assert_eq!(props.creator.as_deref(), Some("Unknown Creator"));
    assert_eq!(props.last_modified_by.as_deref(), Some("Unknown Creator"));
    assert_eq!(props.created.as_deref(), Some("2026-08-23T14:26:08+00:00"));
}

#[test]
fn untouched_properties_travel_as_their_bytes() {
    let original = fixture();
    let book = read_xlsx_from(Cursor::new(original.clone())).unwrap();
    let written = write(&book);
    for name in ["docProps/core.xml", "docProps/app.xml"] {
        assert_eq!(part(&written, name), part(&original, name), "{name}");
    }
}

#[test]
fn a_changed_title_rewrites_core_and_a_company_patches_app_in_place() {
    let original = fixture();
    let mut book = read_xlsx_from(Cursor::new(original.clone())).unwrap();
    book.properties.title = Some("Квартальный отчёт & план".into());
    book.properties.company = Some("ООО «Ромашка»".into());
    let written = write(&book);

    let core = part(&written, "docProps/core.xml").unwrap();
    assert!(core.contains("<dc:title>Квартальный отчёт &amp; план</dc:title>"));
    // The dates keep the type Excel insists on.
    assert!(core.contains(r#"<dcterms:created xsi:type="dcterms:W3CDTF">"#));

    // The file had an empty <Company>; it is filled where it stood, and every
    // other byte Excel keeps in app.xml is as it was.
    let app = part(&written, "docProps/app.xml").unwrap();
    let before = part(&original, "docProps/app.xml").unwrap();
    assert_eq!(
        app.replace("<Company>ООО «Ромашка»</Company>", "<Company></Company>"),
        before
    );

    let again = read_xlsx_from(Cursor::new(written)).unwrap();
    assert_eq!(again.properties, book.properties);
}

#[test]
fn a_new_book_gets_the_parts_it_needs_and_no_others() {
    let mut book = Spreadsheet::empty();
    book.add_sheet(Worksheet::new("Sheet1").unwrap()).unwrap();
    let bare = write(&book);
    assert_eq!(part(&bare, "docProps/core.xml"), None);
    assert_eq!(part(&bare, "docProps/custom.xml"), None);

    book.properties.creator = Some("Ann".into());
    book.properties.manager = Some("Bob".into());
    book.properties.set_custom("Отдел", "Продажи");
    book.properties.set_custom("Страниц", 12_i64);
    book.properties.set_custom("Большое", 5_000_000_000_i64);
    book.properties.set_custom("Курс", 92.5);
    book.properties.set_custom("Проверено", true);
    book.properties
        .set_custom("Срок", PropertyValue::Date("2026-12-31T00:00:00Z".into()));
    // Same name in another case replaces rather than adds.
    book.properties.set_custom("отдел", "Закупки");
    assert_eq!(book.properties.custom.len(), 6);

    let written = write(&book);
    assert!(
        part(&written, "docProps/app.xml")
            .unwrap()
            .contains("<Manager>Bob</Manager>")
    );
    let custom = part(&written, "docProps/custom.xml").unwrap();
    assert!(custom.contains("<vt:i8>5000000000</vt:i8>"));
    let types = part(&written, "[Content_Types].xml").unwrap();
    assert!(types.contains("custom-properties+xml"));
    assert!(
        part(&written, "_rels/.rels")
            .unwrap()
            .contains("custom-properties")
    );

    let again = read_xlsx_from(Cursor::new(written)).unwrap();
    assert_eq!(again.properties, book.properties);
    assert_eq!(
        again.properties.custom("ОТДЕЛ"),
        Some(&PropertyValue::Text("Закупки".into()))
    );
}

#[test]
fn clearing_the_custom_fields_removes_their_part() {
    let mut book = Spreadsheet::empty();
    book.add_sheet(Worksheet::new("Sheet1").unwrap()).unwrap();
    book.properties.set_custom("x", 1_i64);
    let mut again = read_xlsx_from(Cursor::new(write(&book))).unwrap();
    again.properties.custom.clear();
    let written = write(&again);
    assert_eq!(part(&written, "docProps/custom.xml"), None);
    assert!(
        !part(&written, "_rels/.rels")
            .unwrap()
            .contains("custom-properties")
    );
    assert!(
        !part(&written, "[Content_Types].xml")
            .unwrap()
            .contains("custom-properties")
    );
}

/// ODS keeps the same facts under ODF's names. Category, status, identifier,
/// version, company and manager have no element there and do not survive.
#[test]
fn ods_carries_what_odf_has_a_place_for() {
    use excelerate::reader::ods::read_ods_from;
    use excelerate::writer::ods::write_ods_to;

    let mut book = Spreadsheet::empty();
    book.add_sheet(Worksheet::new("Sheet1").unwrap()).unwrap();
    let props = &mut book.properties;
    props.title = Some("Отчёт".into());
    props.subject = Some("Продажи".into());
    props.creator = Some("Анна".into());
    props.last_modified_by = Some("Борис".into());
    props.keywords = Some("отчёт, квартал".into());
    props.description = Some("<проверка> & ещё".into());
    props.created = Some("2026-09-27T10:00:00".into());
    props.modified = Some("2026-09-27T11:00:00".into());
    props.revision = Some("3".into());
    props.category = Some("lost".into());
    props.company = Some("lost".into());
    props.set_custom("Отдел", "Продажи");
    props.set_custom("Страниц", 12_i64);
    props.set_custom("Курс", 92.5);
    props.set_custom("Проверено", true);
    props.set_custom("Срок", PropertyValue::Date("2026-12-31T00:00:00".into()));

    let mut bytes = Vec::new();
    write_ods_to(&book, Cursor::new(&mut bytes)).unwrap();
    let again = read_ods_from(Cursor::new(bytes)).unwrap().properties;

    let mut expected = book.properties.clone();
    expected.category = None;
    expected.company = None;
    assert_eq!(again, expected);
}

/// xls keeps them in two property set streams beside the workbook. Every
/// field Excel shows goes both ways; a whole number past 32 bits comes back
/// as a fraction-less `Number`, since version 0 sets have no 64-bit integer.
#[test]
fn xls_carries_them_in_its_property_sets() {
    use excelerate::reader::xls::read_xls_from;
    use excelerate::writer::xls::write_xls_to;

    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Лист1").unwrap();
    sheet.set(excelerate::CellRef::parse("A1").unwrap(), 1.0);
    book.add_sheet(sheet).unwrap();
    let props = &mut book.properties;
    props.title = Some("Отчёт".into());
    props.subject = Some("Продажи".into());
    props.creator = Some("Анна".into());
    props.keywords = Some("отчёт; квартал".into());
    props.description = Some("Описание".into());
    props.last_modified_by = Some("Борис".into());
    props.revision = Some("4".into());
    props.created = Some("2026-09-27T10:00:00Z".into());
    props.modified = Some("2026-09-27T11:30:15Z".into());
    props.category = Some("Финансы".into());
    props.company = Some("ООО «Ромашка»".into());
    props.manager = Some("Виктор".into());
    props.set_custom("Отдел", "Продажи");
    props.set_custom("Страниц", 12_i64);
    props.set_custom("Курс", 92.5);
    props.set_custom("Проверено", true);
    props.set_custom("Срок", PropertyValue::Date("2026-12-31T00:00:00Z".into()));

    let mut bytes = Vec::new();
    write_xls_to(&book, &mut bytes).unwrap();
    let again = read_xls_from(&bytes).unwrap();
    assert_eq!(again.properties, book.properties);
    // The workbook itself is still there.
    assert_eq!(again.sheets()[0].title(), "Лист1");
}
