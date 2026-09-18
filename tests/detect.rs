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
    // A real package compresses a few times over, so past the cap it is still
    // read; the cap alone cannot tell a bomb from a big workbook.
    assert!(read_bytes_limited(&bytes, None, 16).is_ok());
    assert!(read_bytes_limited(&bytes, None, u64::MAX).is_ok());
}

/// What stops a zip bomb is how hard it compresses: past the cap, a package
/// that expands more than a hundred times its size is refused.
#[test]
fn a_package_that_expands_like_a_bomb_is_refused() {
    use excelerate::reader::read_bytes_limited;
    use std::io::Write;

    let mut bytes = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("[Content_Types].xml", options).unwrap();
        zip.write_all(&vec![b' '; 50 << 20]).unwrap();
        zip.finish().unwrap();
    }
    // Fifty megabytes of spaces deflate to well under half a megabyte.
    let err = read_bytes_limited(&bytes, Some("bomb.xlsx"), 1 << 20).unwrap_err();
    assert!(
        format!("{err}").contains("times its compressed size"),
        "{err}"
    );
}

/// Part names in a package are separated by a forward slash, but writers on
/// Windows have shipped packages spelled `xl\workbook.xml`. Excel opens those,
/// and so does this reader.
#[test]
fn a_part_name_written_with_a_backslash_is_still_found() {
    use std::io::{Read, Write};

    let book = read("tests/fixtures/sample.xlsx").unwrap();
    let mut original = Vec::new();
    excelerate::writer::write_xlsx_to(&book, std::io::Cursor::new(&mut original)).unwrap();

    // Repack it under the same names with the separator Windows used.
    let mut source = zip::ZipArchive::new(std::io::Cursor::new(&original)).unwrap();
    let mut backslashed = Vec::new();
    {
        let mut out = zip::ZipWriter::new(std::io::Cursor::new(&mut backslashed));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for i in 0..source.len() {
            let mut part = source.by_index(i).unwrap();
            let name = part.name().replace('/', "\\");
            let mut data = Vec::new();
            part.read_to_end(&mut data).unwrap();
            out.start_file(name, options).unwrap();
            out.write_all(&data).unwrap();
        }
        out.finish().unwrap();
    }

    let reread = excelerate::reader::read_bytes(&backslashed, Some("windows.xlsx")).unwrap();
    assert_eq!(reread.sheets().len(), book.sheets().len());
    assert_eq!(
        reread.sheets()[0].iter().count(),
        book.sheets()[0].iter().count()
    );
}

/// Excel 97 calls the stream `Workbook` and Excel 5 calls it `Book`, but files
/// naming it `BOOK` exist and `LibreOffice` opens them.
#[test]
fn the_workbook_stream_is_found_whatever_its_case() {
    let book = read("tests/fixtures/sample.xls").unwrap();
    let mut bytes = Vec::new();
    excelerate::writer::write_xls_to(&book, std::io::Cursor::new(&mut bytes)).unwrap();

    // The directory holds the name as UTF-16LE, so upper-casing it in place
    // keeps every offset where it was.
    let name: Vec<u8> = "Workbook"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let upper: Vec<u8> = "WORKBOOK"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let at = bytes
        .windows(name.len())
        .position(|w| w == name.as_slice())
        .expect("the writer names the stream Workbook");
    bytes[at..at + upper.len()].copy_from_slice(&upper);

    let reread = excelerate::reader::read_bytes(&bytes, Some("shouty.xls")).unwrap();
    assert_eq!(reread.sheets().len(), book.sheets().len());
    assert_eq!(
        reread.sheets()[0].iter().count(),
        book.sheets()[0].iter().count()
    );
}
