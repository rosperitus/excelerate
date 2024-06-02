//! Watching a long operation, and lending it functions of your own.
//!
//! Reading a hundred-megabyte package, writing one back and recalculating
//! twenty thousand formulas all take long enough that a caller wants to draw a
//! bar, and a workbook may call a function this crate does not define. Both
//! arrive the same way: through [`Options`], which every `*_with` function
//! takes.

#[cfg(feature = "formulas")]
use crate::formula::CustomFunctions;

/// Which part of the work is being reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Reading a workbook: `done` counts sheets.
    Reading,
    /// Writing one: `done` counts the parts written.
    Writing,
    /// Recalculating: `done` counts formulas.
    Recalculating,
}

/// How far along an operation is.
#[derive(Debug, Clone, Copy)]
pub struct Progress<'a> {
    /// What is being done.
    pub stage: Stage,
    /// How many units are finished.
    pub done: usize,
    /// How many there are in total, where that is known before starting.
    ///
    /// Reading does not know how many sheets a package holds until it has
    /// read the workbook part, so the first few reports carry `None`.
    pub total: Option<usize>,
    /// What is being worked on right now: a sheet name, a part path, or the
    /// empty string where the stage has nothing to name.
    pub what: &'a str,
}

impl Progress<'_> {
    /// The fraction finished, where the total is known.
    #[must_use]
    pub fn fraction(&self) -> Option<f64> {
        let total = self.total?;
        if total == 0 {
            return Some(1.0);
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "counts of sheets, parts and formulas are far below 2^53"
        )]
        Some((self.done as f64) / (total as f64))
    }
}

/// What a caller lends to a long operation: somewhere to report, and functions
/// the workbook may call.
///
/// The callback is `Fn`, not `FnMut`, so that one `Options` can be shared with
/// an engine that is borrowing it at the same time. A callback that has to
/// accumulate — count what it saw, remember the last stage — does it through a
/// `Cell` or an `AtomicUsize` it captures, which is the usual shape for a
/// progress callback anyway.
///
/// ```
/// # #[cfg(feature = "formulas")] {
/// use std::cell::Cell;
/// use excelerate::progress::Options;
///
/// let seen = Cell::new(0usize);
/// let report = |_: excelerate::progress::Progress<'_>| seen.set(seen.get() + 1);
/// let options = Options::new().reporting(&report);
/// let mut book = excelerate::model::Spreadsheet::empty();
/// excelerate::formula::eval::recalculate(&mut book, None, &options);
/// # }
/// ```
#[derive(Default, Clone, Copy)]
pub struct Options<'a> {
    /// Where to report progress, if anywhere.
    progress: Option<&'a dyn Fn(Progress<'_>)>,
    /// The caller's own functions, if any. Only a build with the formula
    /// engine has anywhere to put them.
    #[cfg(feature = "formulas")]
    functions: Option<&'a CustomFunctions>,
    /// Ties the lifetime to the struct in a build without the engine, where
    /// the field above is not there to do it.
    #[cfg(not(feature = "formulas"))]
    lifetime: std::marker::PhantomData<&'a ()>,
}

impl<'a> Options<'a> {
    /// Options that report nowhere and add no functions.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reports progress to `report`.
    #[must_use]
    pub fn reporting(self, report: &'a dyn Fn(Progress<'_>)) -> Self {
        Self {
            progress: Some(report),
            ..self
        }
    }

    /// Lends the operation these functions.
    #[cfg(feature = "formulas")]
    #[must_use]
    pub fn with_functions(self, functions: &'a CustomFunctions) -> Self {
        Self {
            functions: Some(functions),
            ..self
        }
    }

    /// The functions lent, if any.
    #[cfg(feature = "formulas")]
    #[must_use]
    pub const fn functions(&self) -> Option<&'a CustomFunctions> {
        self.functions
    }

    /// Reports one step. Costs nothing when no callback was given.
    pub fn report(&self, stage: Stage, done: usize, total: Option<usize>, what: &str) {
        if let Some(report) = self.progress {
            report(Progress {
                stage,
                done,
                total,
                what,
            });
        }
    }
}

impl std::fmt::Debug for Options<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field("reporting", &self.progress.is_some())
            .finish()
    }
}
