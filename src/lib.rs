//! Excelerate: spreadsheets in Rust. Reading, writing, a formula engine.

// Tests assert on known-good values; a panic there is the failure report.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod coordinate;
// pub mod edit;
pub mod error;
// #[cfg(feature = "formulas")]
// pub mod formula;

pub use coordinate::{CellRef, Col, Range, Row};
pub use error::{CellError, Error, Result};

pub mod model;
// pub mod progress;
// pub mod reader;
// pub mod shared;
pub mod style;
// #[cfg(feature = "write")]
// pub mod writer;

// #[cfg(target_arch = "wasm32")]
// pub mod wasm;
