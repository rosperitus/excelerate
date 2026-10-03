//! Workbooks with a password to open. The fixtures are `sample.xlsx`
//! encrypted by msoffcrypto-tool, which writes the agile encryption Excel
//! 2010 and later write: one with the password `пароль`, one with the
//! password Excel uses for a workbook encrypted only to be opened read-only.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::Error;
use excelerate::model::Spreadsheet;
use excelerate::progress::Options;
use excelerate::reader::detect::{read, read_bytes, read_bytes_limited_with};
use excelerate::reader::encryption::is_encrypted;

const LIMIT: u64 = 1 << 29;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("tests/fixtures/{name}")).unwrap()
}

/// What a workbook holds, sheet by sheet, cell by cell.
fn cells(book: &Spreadsheet) -> Vec<Vec<String>> {
    book.sheets()
        .iter()
        .map(|ws| {
            ws.iter()
                .map(|(at, cell)| format!("{at}={:?}", cell.value))
                .collect()
        })
        .collect()
}

#[test]
fn the_password_opens_what_was_encrypted() {
    let bytes = fixture("encrypted.xlsx");
    assert!(is_encrypted(&bytes));
    let book = read_bytes_limited_with(
        &bytes,
        Some("encrypted.xlsx"),
        LIMIT,
        &Options::new().password("пароль"),
    )
    .unwrap();
    let plain = read("tests/fixtures/sample.xlsx").unwrap();
    assert_eq!(cells(&book), cells(&plain));
}

#[test]
fn a_missing_or_wrong_password_is_said_so() {
    let bytes = fixture("encrypted.xlsx");
    assert_eq!(read_bytes(&bytes, None).unwrap_err(), Error::WrongPassword);
    let wrong = Options::new().password("Пароль");
    assert_eq!(
        read_bytes_limited_with(&bytes, None, LIMIT, &wrong).unwrap_err(),
        Error::WrongPassword
    );
    // The path reader sees a compound file, as an xls is, and still finds
    // the package inside.
    assert_eq!(
        read("tests/fixtures/encrypted.xlsx").unwrap_err(),
        Error::WrongPassword
    );
}

#[test]
fn a_workbook_encrypted_only_to_be_opened_read_only_needs_no_password() {
    let book = read("tests/fixtures/encrypted-readonly.xlsx").unwrap();
    let plain = read("tests/fixtures/sample.xlsx").unwrap();
    assert_eq!(cells(&book), cells(&plain));
}

#[test]
fn an_ordinary_xls_is_not_taken_for_encrypted() {
    assert!(!is_encrypted(&fixture("formulas.xls")));
    assert!(!is_encrypted(&fixture("sample.xlsx")));
}
