//! Password hashes of protected sheets and workbooks, checked against a file
//! `excelize` protected (`tests/fixtures/protected.xlsx`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::model::protection::PasswordHash;
use excelerate::reader::xlsx::read_xlsx;
use excelerate::writer::xlsx::write_xlsx_to;
use std::io::Cursor;

#[test]
fn passwords_another_program_set_verify() {
    let book = read_xlsx("tests/fixtures/protected.xlsx").unwrap();
    let sheet = book.sheet(0).unwrap().protection.password.clone().unwrap();
    assert!(matches!(&sheet, PasswordHash::Iso { algorithm, .. } if algorithm == "SHA-512"));
    assert!(sheet.verify("пароль secret"));
    assert!(!sheet.verify("пароль Secret"));

    let legacy = book.sheet(1).unwrap().protection.password.clone().unwrap();
    assert_eq!(legacy, PasswordHash::Legacy("83AF".to_owned()));
    assert!(legacy.verify("password"));
    assert_eq!(PasswordHash::legacy("password"), legacy);

    let workbook = book.protection.workbook_password.clone().unwrap();
    assert!(workbook.verify("book"));
    assert!(!workbook.verify("books"));
}

#[test]
fn a_password_set_here_survives_a_round_trip() {
    let mut book = read_xlsx("tests/fixtures/protected.xlsx").unwrap();
    let hash = PasswordHash::new("новый").unwrap();
    book.sheet_mut(1).unwrap().protection.password = Some(hash);
    let mut bytes = Cursor::new(Vec::new());
    write_xlsx_to(&book, &mut bytes).unwrap();
    let back = excelerate::reader::xlsx::read_xlsx_from(Cursor::new(bytes.into_inner())).unwrap();
    let hash = back.sheet(1).unwrap().protection.password.clone().unwrap();
    assert!(hash.verify("новый"));
    assert!(!hash.verify("старый"));
}
