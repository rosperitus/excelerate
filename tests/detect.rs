//! `reader::read` picks the right reader for a file.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::reader::{Format, format_of, read};
use excelerate::writer::{write_csv, write_html, write_ods, write_xlsx};

/// A path in the test's own temporary directory.
fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("excelerate-detect-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn committed_fixtures_identify_themselves() {
    for (file, expected) in [
        ("sample.xlsx", Format::Xlsx),
        ("styles.xlsx", Format::Xlsx),
        ("sample.xls", Format::Xls),
        ("sample.slk", Format::Slk),
        ("sample.gnumeric", Format::Gnumeric),
        ("sample.xml", Format::Xml2003),
    ] {
        let path = format!("tests/fixtures/{file}");
        assert_eq!(format_of(&path).unwrap(), expected, "{file}");
        let book = read(&path).unwrap_or_else(|e| panic!("{file} reads: {e}"));
        assert!(!book.sheets().is_empty(), "{file}");
    }
}

#[test]
fn written_files_identify_themselves() {
    let book = read("tests/fixtures/sample.xlsx").unwrap();

    for (name, expected) in [
        ("out.ods", Format::Ods),
        ("out.csv", Format::Csv),
        ("out.html", Format::Html),
        ("out.xlsx", Format::Xlsx),
    ] {
        let path = tmp(name);
        match expected {
            Format::Ods => write_ods(&book, &path),
            Format::Csv => write_csv(&book, 0, &path),
            Format::Html => write_html(&book, &path),
            _ => write_xlsx(&book, &path),
        }
        .unwrap();
        assert_eq!(format_of(&path).unwrap(), expected, "{name}");
        assert!(!read(&path).unwrap().sheets().is_empty(), "{name}");
        std::fs::remove_file(&path).unwrap();
    }
}

#[test]
fn the_signature_outranks_a_lying_extension() {
    // An xlsx named `.csv` is still an xlsx, and a CSV named `.xlsx` is not one.
    let book = read("tests/fixtures/sample.xlsx").unwrap();
    let lying = tmp("really-xlsx.csv");
    write_xlsx(&book, &lying).unwrap();
    assert_eq!(format_of(&lying).unwrap(), Format::Xlsx);
    assert_eq!(read(&lying).unwrap().sheets().len(), book.sheets().len());
    std::fs::remove_file(&lying).unwrap();

    let plain = tmp("really-csv.xlsx");
    std::fs::write(&plain, "a,b\n1,2\n").unwrap();
    // Plain text says nothing about itself, so the extension is believed and
    // the xlsx reader is the one that reports the mismatch.
    assert_eq!(format_of(&plain).unwrap(), Format::Xlsx);
    assert!(read(&plain).is_err());
    std::fs::remove_file(&plain).unwrap();
}

#[test]
fn text_with_an_unknown_extension_is_csv() {
    let path = tmp("data.dat");
    std::fs::write(&path, "a,b\n1,2\n").unwrap();
    assert_eq!(format_of(&path).unwrap(), Format::Csv);
    let book = read(&path).unwrap();
    assert_eq!(book.sheets()[0].len(), 4);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn a_missing_file_says_so_before_any_reader_runs() {
    let err = format_of("tests/fixtures/nothing-here.xlsx").unwrap_err();
    assert!(matches!(err, excelerate::Error::Io(_)), "{err:?}");
}

#[test]
fn bytes_are_identified_the_same_way() {
    use excelerate::reader::read_bytes;

    for file in [
        "sample.xlsx",
        "sample.xls",
        "sample.slk",
        "sample.gnumeric",
        "sample.xml",
    ] {
        let bytes = std::fs::read(format!("tests/fixtures/{file}")).unwrap();
        let book = read_bytes(&bytes, None).unwrap_or_else(|e| panic!("{file}: {e}"));
        let from_path = read(format!("tests/fixtures/{file}")).unwrap();
        assert_eq!(book.sheets().len(), from_path.sheets().len(), "{file}");
    }

    // A SYLK file carries no sheet name; the file name lends it one.
    let slk = std::fs::read("tests/fixtures/sample.slk").unwrap();
    assert_eq!(
        read_bytes(&slk, None).unwrap().sheets()[0].title(),
        "Worksheet"
    );
    assert_eq!(
        read_bytes(&slk, Some("Отчёт.slk")).unwrap().sheets()[0].title(),
        "Отчёт"
    );

    // Plain text says nothing, so the name decides - and CSV is the default.
    assert_eq!(
        read_bytes(b"a,b\n1,2\n", None).unwrap().sheets()[0].len(),
        4
    );
    // The same bytes named `.html` go to the HTML reader, which finds no
    // markup in them and so no cells at all.
    assert_eq!(
        read_bytes(b"a,b\n1,2\n", Some("page.html"))
            .unwrap()
            .sheets()[0]
            .len(),
        0
    );
}

#[test]
fn the_expansion_cap_can_be_raised() {
    use excelerate::reader::read_bytes_limited;

    let bytes = std::fs::read("tests/fixtures/sample.xlsx").unwrap();
    // A cap below what the package expands to is what stops a zip bomb.
    let err = read_bytes_limited(&bytes, None, 16).unwrap_err();
    assert!(format!("{err}").contains("over the 16 limit"), "{err}");
    assert!(read_bytes_limited(&bytes, None, u64::MAX).is_ok());
}
