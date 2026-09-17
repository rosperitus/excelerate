//! Bytes as the workbook stream of an xls: records, strings split over
//! `CONTINUE`, formula tokens. Wrapped in a compound file first, so the
//! mutations land on the records rather than on the container.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // A stream the reader never saw a valid header for is refused early;
    // an arbitrary one read as a container exercises the OLE reader too.
    let _ = excelerate::reader::xls::read_xls_from(data);
});
