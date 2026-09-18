//! Any bytes as a workbook, through format detection, then written back and
//! recalculated. The first byte picks the file name the bytes arrive under,
//! since detection falls back on the extension where the signature is silent.

#![no_main]

use excelerate::formula::eval::recalculate;
use excelerate::progress::Options;
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

const NAMES: [Option<&str>; 9] = [
    None,
    Some("a.xlsx"),
    Some("a.xlsb"),
    Some("a.xls"),
    Some("a.csv"),
    Some("a.html"),
    Some("a.slk"),
    Some("a.xml"),
    Some("a.ods"),
];

fuzz_target!(|data: &[u8]| {
    let Some((&pick, bytes)) = data.split_first() else {
        return;
    };
    let name = NAMES[usize::from(pick) % NAMES.len()];
    // A package may expand to far more than the fuzzer's memory limit; a
    // real caller picks the cap, and this one picks a small one.
    let Ok(mut book) = excelerate::reader::read_bytes_limited(bytes, name, 64 << 20) else {
        return;
    };
    let cells: usize = book.sheets().iter().map(excelerate::model::Worksheet::len).sum();
    if cells > 20_000 {
        return;
    }
    recalculate(&mut book, None, &Options::default());
    let _ = excelerate::writer::xlsx::write_xlsx_to(&book, Cursor::new(Vec::new()));
    let _ = excelerate::writer::xls::write_xls_to(&book, Vec::new());
    let _ = excelerate::writer::csv::write_csv_to(&book, 0, Vec::new(), ',');
});
