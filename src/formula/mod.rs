//! The formula engine: parsing formula text and computing its value.
//!
//!
//! ```text
//! parser  formula text -> Expr
//! value   the values and the conversions between them
//! eval    Expr + a workbook -> Value
//! ```

pub mod custom;
pub mod eval;
pub mod functions;
pub mod parser;
pub mod value;

pub use custom::CustomFunctions;
pub use eval::{Engine, Origin};
pub use parser::{BinaryOp, Expr, UnaryOp, parse};
pub use value::Value;
