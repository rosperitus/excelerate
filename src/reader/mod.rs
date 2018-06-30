//! Readers for spreadsheet file formats.

// pub(crate) mod chart;
// pub mod csv;
// pub mod detect;
// pub mod gnumeric;
// pub mod html;
// pub mod ods;
// pub(crate) mod ole;
// pub mod slk;
// pub mod xls;
pub mod xlsx;
// pub mod xml2003;
pub(crate) mod zipxml;

// pub use csv::{CsvOptions, FormattedNumbers, read_csv, read_csv_str, read_csv_with};
// pub use detect::{
//     Format, format_of, read, read_bytes, read_bytes_limited, read_bytes_limited_with,
// };
// pub use gnumeric::{read_gnumeric, read_gnumeric_from, read_gnumeric_str};
// pub use html::{read_html, read_html_str};
// pub use ods::{read_ods, read_ods_from};
// pub use slk::{read_slk, read_slk_str};
// pub use xls::{read_xls, read_xls_from};
pub use xlsx::read_xlsx;
// pub use xml2003::{read_xml2003, read_xml2003_str};
