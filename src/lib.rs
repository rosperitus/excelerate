//! Excelerate: spreadsheets in Rust. Reading, writing, a formula engine.

// Tests assert on known-good values; a panic there is the failure report.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod coordinate;
pub mod edit;
pub mod error;
#[cfg(feature = "formulas")]
pub mod formula;

pub use coordinate::{CellRef, Col, Range, Row};
pub use error::{CellError, Error, Result};

pub mod model;
pub mod progress;
pub mod reader;
pub mod shared;
pub mod style;
#[cfg(feature = "write")]
pub mod writer;

#[cfg(target_arch = "wasm32")]
pub mod wasm;

/// The README and the pages under `docs/`, pulled in so that `cargo test`
/// compiles every example in them. Nothing else uses this type, and it exists
/// only while doctests are collected.
///
/// A page that needs a file on disk marks its block `no_run`; a block that is
/// an excerpt of a type rather than code to run is fenced as `text`.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
#[doc = include_str!("../docs/getting-started.md")]
#[doc = include_str!("../docs/model.md")]
#[doc = include_str!("../docs/editing.md")]
#[doc = include_str!("../docs/sheet-features.md")]
#[doc = include_str!("../docs/long-operations.md")]
#[doc = include_str!("../docs/recipes.md")]
#[doc = include_str!("../docs/formats.md")]
#[doc = include_str!("../docs/formulas.md")]
#[doc = include_str!("../docs/styles.md")]
#[doc = include_str!("../docs/external-links.md")]
#[doc = include_str!("../docs/wasm.md")]
pub struct Documentation;
