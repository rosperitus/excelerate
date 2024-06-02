//! Functions a caller adds to the engine.
//!
//! Excel calls these user-defined functions and keeps them in VBA; here one is
//! any Rust closure that takes the computed arguments and answers with a
//! value. A workbook cannot carry them — a file holds the *name* of a function
//! and nothing more — so a book that uses one shows `#NAME?` until the caller
//! registers it, which is what Excel does with macros disabled.
//!
//! A built-in of the same name wins. Excel refuses to let a user-defined
//! function shadow `SUM`, and a workbook where `SUM` means something else is a
//! workbook nobody else can read.

use crate::formula::value::Value;
use std::collections::HashMap;
use std::rc::Rc;

/// A function written by the caller: the arguments already computed, one value
/// back.
///
/// A range argument arrives as [`Value::Array`], the way it does inside every
/// built-in, so a function summing `A1:A9` sees the cells rather than a
/// reference to them.
pub type CustomFn = dyn Fn(&[Value]) -> Value;

/// The functions a caller has added, looked up by name without regard to case.
#[derive(Clone, Default)]
pub struct CustomFunctions {
    by_name: HashMap<String, Rc<CustomFn>>,
}

impl CustomFunctions {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a function, replacing one registered under the same name.
    ///
    /// The name is matched the way Excel matches function names: ignoring
    /// case, so `myrate` and `MYRATE` are one function.
    pub fn register(&mut self, name: &str, function: impl Fn(&[Value]) -> Value + 'static) {
        self.by_name
            .insert(name.to_ascii_uppercase(), Rc::new(function));
    }

    /// Removes a function; answers whether there was one.
    pub fn remove(&mut self, name: &str) -> bool {
        self.by_name.remove(&name.to_ascii_uppercase()).is_some()
    }

    /// The function registered under a name, if any.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Rc<CustomFn>> {
        self.by_name.get(&name.to_ascii_uppercase()).cloned()
    }

    /// How many functions are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// Whether none are.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// The names registered, in no particular order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }
}

impl std::fmt::Debug for CustomFunctions {
    /// Closures have nothing to show but their names.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomFunctions")
            .field("names", &self.by_name.keys().collect::<Vec<_>>())
            .finish()
    }
}
