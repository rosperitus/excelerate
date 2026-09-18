//! Computing a parsed formula against a workbook.
//!
//!
//! An engine could walk a stack in reverse Polish notation; this walks the tree the
//! parser built. A cell that holds a formula is computed when something reads
//! it, and the result is remembered, so a sheet of formulas that all refer to
//! one another is evaluated once rather than once per reader.

use crate::coordinate::{CellRef, Col, Range, Row};
use crate::error::CellError;
use crate::formula::custom::CustomFunctions;
use crate::formula::functions;
use crate::formula::parser::{BinaryOp, Expr, Structured, TablePart, UnaryOp, parse};
use crate::formula::value::{Value, compare};
use crate::model::{CellValue, Spreadsheet};
use crate::progress::{Options, Stage};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Where a formula sits: which sheet it belongs to and which cell holds it.
///
/// A relative reference means nothing without it, and neither do `ROW()` or
/// `COLUMN()` called with no argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Origin {
    /// Tab index of the sheet the formula lives on.
    pub sheet: usize,
    /// The cell the formula lives in.
    pub at: CellRef,
}

impl Origin {
    /// A formula in cell `at` of sheet `sheet`.
    #[must_use]
    pub const fn new(sheet: usize, at: CellRef) -> Self {
        Self { sheet, at }
    }
}

/// Largest number of cells a single reference may pull in.
///
/// `A:A` is a million cells and `1:1` sixteen thousand; the reference is
/// clipped to the part of the sheet that holds anything, but a workbook is
/// untrusted input, so there is a hard ceiling as well. The same ceiling holds
/// for an array a formula builds: `EXPAND`, a row times a column, a function
/// lifted over two arrays.
pub(crate) const MAX_RANGE_CELLS: usize = 4_000_000;

/// How deep a chain of formulas reading formulas is followed in one go.
///
/// A cell is computed when something reads it, so `A1=A2+1, A2=A3+1, ...` down
/// a whole column recurses once per link, and a few thousand links overflow
/// the stack of a normal thread. So the chain is cut here: the cell at this
/// depth is set aside ([`Engine::deferred`]), everything above it unwinds
/// without being cached, the cell is computed from the top, and the read that
/// needed it is tried again. A chain of any length costs its length in work
/// and this much in stack. A hundred links fit the two megabytes of a test
/// thread in a debug build with room to spare; five hundred did not.
const MAX_CHAIN_DEPTH: usize = 100;

/// Evaluates formulas against a workbook.
pub struct Engine<'a> {
    book: &'a Spreadsheet,
    /// Values already computed, so a cell read many times is computed once.
    cache: HashMap<(usize, CellRef), Value>,
    /// Cells being computed right now, which is how a cycle is spotted.
    running: HashSet<(usize, CellRef)>,
    /// Names being resolved right now, for the same reason: a name may stand
    /// for a formula that mentions the name.
    resolving: HashSet<String>,
    /// Functions the caller added, which are tried after the built-ins.
    custom: Option<&'a CustomFunctions>,
    /// Names bound by `LET` and by the parameters of a running `LAMBDA`, the
    /// innermost last. A name is looked up here before the workbook's defined
    /// names, so a parameter shadows a name of the book, as it does in Excel.
    scope: Vec<(String, Value)>,
    /// Used range of each sheet, computed on demand. `Worksheet::dimension`
    /// walks every cell of the sheet, and `clip` asks for it on every
    /// reference; on a workbook of a quarter million cells that scan is the
    /// whole cost of a recalculation. An empty sheet stays `None` and is asked
    /// again, which costs nothing: it has no cells to walk.
    dims: Vec<Option<Range>>,
    /// Rectangles already read, by sheet. A column a thousand formulas read,
    /// as `SKEW(E$17:E$316)` down a sheet, would otherwise be rebuilt cell by
    /// cell a thousand times.
    ranges: HashMap<(usize, Range), Value>,
    /// How many times a cell was read while it was still being computed. A
    /// rectangle read across such a moment holds a stand-in error, not the
    /// cell's value, and must not be kept.
    cycle_hits: u64,
    /// A cell too deep in a chain to compute where it was asked for, to be
    /// computed from the top first. While it is set, nothing computed is
    /// cached: those values stand on its stand-in error.
    deferred: Option<(usize, CellRef)>,
    /// Cells set aside for a deeper one they read, still to be finished.
    waiting: HashSet<(usize, CellRef)>,
    /// The defined names by their lower-cased name, built on first use. A
    /// book with seven hundred names and formulas naming them on every row
    /// cannot afford a walk of the list per mention.
    name_index: Option<HashMap<String, Vec<usize>>>,
    /// Formula texts already parsed that are read again and again: what a
    /// defined name stands for, and the text `INDIRECT` is handed. `None` for
    /// a text that does not parse.
    parsed: HashMap<String, Option<Arc<Expr>>>,
    /// Whether a range given where one value goes narrows to one cell: on
    /// while a formula that is not an array formula runs.
    implicit: bool,
    /// The top left cells of the workbook's array formulas, built on first
    /// use.
    array_starts: Option<HashSet<(usize, CellRef)>>,
    /// Results other engines finished in a parallel pass, looked at before a
    /// formula cell is computed here.
    shared: Option<&'a Shared>,
    /// The formula cells this pass recomputes. Any other formula cell answers
    /// with the result stored beside it, which is what makes an edit cost the
    /// formulas it touched rather than everything above them as well.
    stale: Option<&'a HashSet<(usize, CellRef)>>,
}

/// The results of a parallel pass, one slot per formula of the index, which
/// every engine of the pass reads and the one that computes a formula fills.
pub(crate) struct Shared {
    slots: HashMap<(usize, CellRef), usize>,
    values: Vec<std::sync::OnceLock<Value>>,
    /// Whether an engine has taken a formula on. One that finds a cell taken
    /// waits a little for the answer instead of computing the same chain
    /// beside it.
    claimed: Vec<std::sync::atomic::AtomicBool>,
    /// Rectangles read, as `Engine::ranges` keeps them for one engine: a
    /// table sixteen engines each read into a copy of their own was sixteen
    /// copies.
    ranges: std::sync::RwLock<HashMap<(usize, Range), Value>>,
}

impl Shared {
    fn slot(&self, sheet: usize, at: CellRef) -> Option<usize> {
        self.slots.get(&(sheet, at)).copied()
    }

    fn range(&self, sheet: usize, range: Range) -> Option<Value> {
        self.ranges.read().ok()?.get(&(sheet, range)).cloned()
    }

    fn keep_range(&self, sheet: usize, range: Range, value: &Value) {
        if let Ok(mut ranges) = self.ranges.write() {
            ranges
                .entry((sheet, range))
                .or_insert_with(|| value.clone());
        }
    }

    fn get(&self, sheet: usize, at: CellRef) -> Option<&Value> {
        self.slots
            .get(&(sheet, at))
            .and_then(|&slot| self.values[slot].get())
    }

    /// Takes the formula on, or waits briefly for whoever did.
    ///
    /// The wait is bounded: two engines can be waiting for cells that read
    /// each other - a circular reference through a reference built at
    /// evaluation time - and each has to get on with it rather than hold.
    fn claim(&self, slot: usize) -> Option<&Value> {
        use std::sync::atomic::Ordering;
        if !self.claimed[slot].swap(true, Ordering::AcqRel) {
            return None;
        }
        for _ in 0..WAIT_SPINS {
            if let Some(value) = self.values[slot].get() {
                return Some(value);
            }
            std::thread::yield_now();
        }
        None
    }
}

impl<'a> Engine<'a> {
    /// An engine over a workbook.
    #[must_use]
    pub fn new(book: &'a Spreadsheet) -> Self {
        Self {
            book,
            cache: HashMap::new(),
            running: HashSet::new(),
            resolving: HashSet::new(),
            custom: None,
            scope: Vec::new(),
            dims: vec![None; book.sheets().len()],
            ranges: HashMap::new(),
            cycle_hits: 0,
            deferred: None,
            waiting: HashSet::new(),
            name_index: None,
            parsed: HashMap::new(),
            implicit: false,
            array_starts: None,
            shared: None,
            stale: None,
        }
    }

    /// An engine that also knows the caller's own functions.
    ///
    /// A name the built-ins claim stays theirs: a workbook where `SUM` means
    /// something else is a workbook nobody else can read.
    #[must_use]
    pub fn with_functions(book: &'a Spreadsheet, custom: &'a CustomFunctions) -> Self {
        Self {
            custom: Some(custom),
            ..Self::new(book)
        }
    }

    /// The caller's functions, if any were given.
    #[must_use]
    pub const fn custom(&self) -> Option<&'a CustomFunctions> {
        self.custom
    }

    /// The workbook being evaluated.
    #[must_use]
    pub const fn book(&self) -> &'a Spreadsheet {
        self.book
    }

    /// Computes a formula as if it sat at `origin`.
    ///
    /// A formula that does not parse is `#NAME?`, which is what Excel shows
    /// for text it cannot make sense of.
    ///
    /// A whole formula never answers "empty": `=B3` over an empty cell is 0,
    /// as it is in Excel. Inside the formula emptiness is still itself, which
    /// is what lets `ISBLANK` and `COUNTBLANK` tell the two apart. An answer
    /// covering a single cell is likewise handed back as that cell's value.
    pub fn eval(&mut self, origin: Origin, formula: &str) -> Value {
        match parse(formula) {
            Ok(expr) => self.eval_tree(origin, &expr),
            Err(_) => Value::Error(CellError::Name),
        }
    }

    /// The same for a formula already parsed.
    pub fn eval_tree(&mut self, origin: Origin, expr: &Expr) -> Value {
        let single = self.holds_single_formula(origin);
        let saved = std::mem::replace(&mut self.implicit, single);
        let value = self.eval_value(origin, expr);
        self.implicit = saved;
        // A result of one cell is that cell: `INDEX(A1:A3,2)` is a number, not
        // a one-element array, and neither is `{5}`.
        let value = match value {
            Value::Array(rows) if rows.len() == 1 && rows[0].len() == 1 => {
                rows.first().and_then(|r| r.first()).cloned()
            }
            other => Some(other),
        };
        match value {
            Some(Value::Blank) | None => Value::Number(0.0),
            Some(other) => other,
        }
    }

    /// Computes an expression where one value is wanted.
    ///
    /// Outside an array formula a range given where a value goes stands for
    /// one of its cells, the one in the formula's own row or column: `=A1:A9`
    /// in B5 is A5, and so is `A1:A9*2` read as `A5*2`. Excel calls this
    /// implicit intersection. A range the formula's row and column miss is
    /// `#VALUE!`. Inside an array formula, or an array parameter, the range
    /// stays whole.
    ///
    /// ponytail: only a reference the formula spells out narrows - a range,
    /// a table column, a defined name, `INDEX` with literal positions. A
    /// reference a function hands back at run time (`IF(c,A1:A9,0)`,
    /// `CHOOSE`, `OFFSET`) arrives as an array and shows its top left value,
    /// because values carry no address. Narrowing those means a reference
    /// value in `Value`.
    pub(crate) fn eval_value(&mut self, origin: Origin, expr: &Expr) -> Value {
        match self.narrowed(origin, expr) {
            Some(Ok(cell)) => self.eval_expr(origin, &cell),
            Some(Err(e)) => Value::Error(e),
            None => self.eval_expr(origin, expr),
        }
    }

    /// The cell a reference narrows to where one value is wanted, as an
    /// expression; `None` when nothing narrows: the formula is an array one,
    /// or the expression is not a reference to several cells.
    pub(crate) fn narrowed(
        &mut self,
        origin: Origin,
        expr: &Expr,
    ) -> Option<std::result::Result<Expr, CellError>> {
        if !self.implicit {
            return None;
        }
        let (sheet, area) = self.area_of(origin, expr, 0)?;
        let at = origin.at;
        let col = if area.width() == 1 {
            area.start.col
        } else if (area.start.col..=area.end.col).contains(&at.col) {
            at.col
        } else {
            return Some(Err(CellError::Value));
        };
        let row = if area.height() == 1 {
            area.start.row
        } else if (area.start.row..=area.end.row).contains(&at.row) {
            at.row
        } else {
            return Some(Err(CellError::Value));
        };
        let cell = CellRef::new(col, row);
        Some(Ok(Expr::Range {
            sheet,
            range: Range {
                start: cell,
                end: cell,
            },
            anchors: crate::formula::parser::Anchors::default(),
        }))
    }

    /// Runs `body` where ranges stay whole, as an array parameter takes them:
    /// `SUMPRODUCT((A1:A9>1)*B1:B9)` compares every cell.
    pub(crate) fn in_array<T>(&mut self, body: impl FnOnce(&mut Self) -> T) -> T {
        let saved = std::mem::replace(&mut self.implicit, false);
        let out = body(self);
        self.implicit = saved;
        out
    }

    /// Whether the cell holds a formula that is not an array formula: the
    /// formulas implicit intersection applies to. A formula asked about from
    /// outside any cell - `Book.evaluate` - keeps its ranges whole.
    fn holds_single_formula(&mut self, origin: Origin) -> bool {
        let book = self.book;
        let Some(sheet) = book.sheet(origin.sheet) else {
            return false;
        };
        if !matches!(
            sheet.get(origin.at).map(|c| &c.value),
            Some(CellValue::Formula { .. })
        ) {
            return false;
        }
        let starts = self.array_starts.get_or_insert_with(|| {
            book.sheets()
                .iter()
                .enumerate()
                .flat_map(|(i, ws)| ws.array_formulas.iter().map(move |r| (i, r.start)))
                .collect()
        });
        !starts.contains(&(origin.sheet, origin.at))
    }

    /// The rectangle of several cells an expression refers to, where that can
    /// be told without reading cells: a range, a table column, a defined name
    /// standing for one of those, or `INDEX` over one with literal positions.
    fn area_of(
        &mut self,
        origin: Origin,
        expr: &Expr,
        depth: u8,
    ) -> Option<(Option<String>, Range)> {
        let (sheet, area) = match expr {
            Expr::Range { sheet, range, .. } if range.start != range.end => (sheet.clone(), *range),
            Expr::Structured(reference) => {
                let (sheet, range) = self.resolve_table(origin, reference).ok()?;
                (Some(sheet), range)
            }
            // A name standing for a name is followed, but not for ever.
            Expr::Name(name)
                if depth < 8 && !self.scope.iter().any(|(b, _)| b.eq_ignore_ascii_case(name)) =>
            {
                let (scope, bare) = self.name_scope(origin, name);
                let found = self.defined_name(bare, scope)?;
                let book = self.book;
                let tree = self.parsed(book.defined_names[found].formula.as_str())?;
                return self.area_of(origin, &tree, depth + 1);
            }
            Expr::Call { name, args } if name == "INDEX" => {
                let (sheet, area) = self.area_of(origin, args.first()?, depth)?;
                let (mut row, mut col) = (position(args.get(1))?, position(args.get(2))?);
                // `INDEX(B5:F5, 3)` picks the column, as the function does.
                if args.len() == 2 && area.height() == 1 {
                    (row, col) = (1, row);
                }
                let pick = |from: u32, len: u32, at: u32| match at {
                    0 => Some((from, from + len - 1)),
                    k if k <= len => Some((from + k - 1, from + k - 1)),
                    _ => None,
                };
                let (r1, r2) = pick(area.start.row.index(), area.height(), row)?;
                let (c1, c2) = pick(area.start.col.index(), area.width(), col)?;
                let area = Range {
                    start: CellRef::new(Col::new(c1)?, Row::new(r1)?),
                    end: CellRef::new(Col::new(c2)?, Row::new(r2)?),
                };
                (sheet, area)
            }
            _ => return None,
        };
        (area.start != area.end).then_some((sheet, area))
    }

    /// The value of one cell, computing its formula if it holds one.
    ///
    /// A cell holds one value. A formula that works out to an array -
    /// `{=TRANSPOSE(...)}` entered over a column - shows its top left value in
    /// its own cell, and the rest of its range holds the rest as stored
    /// values; read through a reference, the whole array would otherwise be
    /// counted again for every cell. [`Engine::spilled`] has the array whole.
    pub fn cell(&mut self, sheet: usize, at: CellRef) -> Value {
        match self.spilled(sheet, at) {
            Value::Array(rows) => rows
                .first()
                .and_then(|row| row.first())
                .cloned()
                .unwrap_or(Value::Blank),
            other => other,
        }
    }

    /// What a cell's formula works out to, an array included, which is what
    /// `A1#` asks for.
    pub fn spilled(&mut self, sheet: usize, at: CellRef) -> Value {
        if !self.running.is_empty() {
            return self.spilled_once(sheet, at);
        }
        // From the top, cells set aside by a chain too deep are computed
        // first, deepest last in, and the read that needed each is tried again.
        // A cell waiting on the one being computed reads it, so reading the
        // waiting cell again from there is a cycle (`Engine::waiting`).
        let mut pending = vec![(sheet, at)];
        loop {
            let Some(&(s, a)) = pending.last() else {
                return Value::Blank;
            };
            let value = self.spilled_once(s, a);
            if let Some(cell) = self.deferred.take() {
                self.waiting.insert((s, a));
                pending.push(cell);
                continue;
            }
            pending.pop();
            match pending.last() {
                Some(below) => {
                    self.waiting.remove(below);
                }
                None => return value,
            }
        }
    }

    fn spilled_once(&mut self, sheet: usize, at: CellRef) -> Value {
        let hits = self.cycle_hits;
        if let Some(v) = self.cache.get(&(sheet, at)) {
            return v.clone();
        }
        let Some(ws) = self.book.sheet(sheet) else {
            return Value::Error(CellError::Ref);
        };
        let stored = ws.get(at).map(|c| c.value.clone());
        let value = match stored {
            None | Some(CellValue::Empty) => Value::Blank,
            Some(CellValue::Number(n)) => Value::Number(n),
            Some(CellValue::Text(t)) => Value::Text(t.to_string()),
            // Formatting inside the cell is presentation; a formula reads the
            // text it spells.
            Some(rich @ CellValue::RichText(_)) => {
                Value::Text(rich.plain_text().unwrap_or_default())
            }
            Some(CellValue::Bool(b)) => Value::Bool(b),
            Some(CellValue::Error(e)) => Value::Error(e),
            Some(CellValue::Formula { .. })
                if let Some(done) = self.shared.and_then(|sh| sh.get(sheet, at)) =>
            {
                done.clone()
            }
            Some(CellValue::Formula {
                cached: Some(cached),
                ..
            }) if self
                .stale
                .is_some_and(|stale| !stale.contains(&(sheet, at))) =>
            {
                stored_value(&cached)
            }
            Some(CellValue::Formula { .. })
                if let Some(sh) = self.shared
                    && let Some(slot) = sh.slot(sheet, at)
                    && let Some(done) = sh.claim(slot) =>
            {
                done.clone()
            }
            Some(CellValue::Formula { formula, .. }) => {
                // A formula that refers back to its own cell would recurse for
                // ever. Excel answers 0 and warns; making it visible is more
                // use than a silent zero.
                if self.waiting.contains(&(sheet, at)) || !self.running.insert((sheet, at)) {
                    self.cycle_hits += 1;
                    return Value::Error(CellError::Ref);
                }
                if self.running.len() > MAX_CHAIN_DEPTH {
                    self.running.remove(&(sheet, at));
                    self.deferred.get_or_insert((sheet, at));
                    // Keeps a rectangle read across this from being cached.
                    self.cycle_hits += 1;
                    return Value::Error(CellError::Value);
                }
                let value = self.eval(Origin::new(sheet, at), &formula);
                self.running.remove(&(sheet, at));
                value
            }
        };
        if self.deferred.is_none() {
            // In a parallel pass a formula's result goes where every engine
            // sees it, and not into this one's cache as well.
            match self
                .shared
                .and_then(|sh| sh.slot(sheet, at).map(|slot| (sh, slot)))
            {
                Some((sh, slot)) if self.cycle_hits == hits => {
                    let _ = sh.values[slot].set(value.clone());
                }
                _ => {
                    self.cache.insert((sheet, at), value.clone());
                }
            }
        }
        value
    }

    /// Keeps what a formula cell worked out to, computed from the top of a
    /// pass, for the formulas after it that read the cell. Without this each
    /// of them computed it again.
    pub(crate) fn remember(&mut self, sheet: usize, at: CellRef, value: &Value) {
        if self.deferred.is_none() {
            self.cache.insert((sheet, at), value.clone());
        }
    }

    /// Computes an already parsed expression.
    pub fn eval_expr(&mut self, origin: Origin, expr: &Expr) -> Value {
        match expr {
            Expr::Number(n) => Value::Number(*n),
            Expr::Text(t) => Value::Text(t.clone()),
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Error(e) => Value::Error(*e),
            Expr::Missing => Value::Blank,
            Expr::Range { sheet, range, .. } => self.range(origin, sheet.as_deref(), *range),
            Expr::Structured(reference) => match self.resolve_table(origin, reference) {
                Ok((sheet, range)) => self.range(origin, Some(&sheet), range),
                Err(e) => Value::Error(e),
            },
            Expr::Name(name) => self.name(origin, name),
            Expr::Unary(op, x) => {
                let v = self.eval_value(origin, x);
                unary(*op, &v)
            }
            Expr::Binary(op, a, b) => self.binary(origin, *op, a, b),
            Expr::Call { name, args } => functions::call(self, origin, name, args),
            Expr::Apply { callee, args } => self.apply_expr(origin, callee, args),
            Expr::Array(rows) => Value::array(
                rows.iter()
                    .map(|r| r.iter().map(|e| self.eval_expr(origin, e)).collect())
                    .collect(),
            ),
        }
    }

    /// Binds `names` for the duration of `body`, then puts the scope back.
    ///
    /// The bindings are pushed as a block, so a `LET` naming three things and
    /// a `LAMBDA` taking three parameters cost one push each.
    pub fn scoped<T>(
        &mut self,
        names: Vec<(String, Value)>,
        body: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let depth = self.scope.len();
        self.scope.extend(names);
        let out = body(self);
        self.scope.truncate(depth);
        out
    }

    /// What a name is bound to right now, if anything: the innermost binding
    /// wins, which is what lets an inner `LET` shadow an outer one.
    fn bound(&self, name: &str) -> Option<Value> {
        self.scope
            .iter()
            .rev()
            .find(|(bound, _)| bound.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }

    /// Calls a value that should be a lambda, with arguments already computed.
    ///
    /// A lambda called with the wrong number of arguments is `#VALUE!`, and a
    /// value that is not a lambda at all is `#CALC!` - the error Excel shows
    /// when something is asked to be a function and is not.
    pub fn apply(&mut self, origin: Origin, callee: &Value, args: Vec<Value>) -> Value {
        // An error where the function should be is that error, not a
        // complaint that it is not a function.
        if let Some(e) = callee.error() {
            return Value::Error(e);
        }
        let Value::Lambda(lambda) = callee.scalar() else {
            return Value::Error(CellError::Calc);
        };
        if args.len() != lambda.params.len() {
            return Value::Error(CellError::Value);
        }
        let lambda = std::sync::Arc::clone(lambda);
        // What the lambda captured where it was written comes first, so its
        // own parameters shadow it.
        let mut bindings = lambda.captured.clone();
        bindings.extend(lambda.params.iter().cloned().zip(args));
        self.scoped(bindings, |engine| engine.eval_expr(origin, &lambda.body))
    }

    /// The bindings in view right now, which is what a new lambda captures.
    #[must_use]
    pub fn captured(&self) -> Vec<(String, Value)> {
        self.scope.clone()
    }

    /// Computes a call written on an expression rather than a name, as
    /// `LAMBDA(x,x+1)(5)` is.
    fn apply_expr(&mut self, origin: Origin, callee: &Expr, args: &[Expr]) -> Value {
        let callee = self.eval_expr(origin, callee);
        let args: Vec<Value> = args.iter().map(|a| self.eval_expr(origin, a)).collect();
        self.apply(origin, &callee, args)
    }

    /// The sheet a name is looked up in, and the name without its sheet.
    fn name_scope<'n>(&self, origin: Origin, name: &'n str) -> (Option<usize>, &'n str) {
        match name.rsplit_once('!') {
            Some((sheet, bare)) => (self.sheet_index(origin, Some(sheet)), bare),
            None => (Some(origin.sheet), name),
        }
    }

    /// Resolves a defined name to what it stands for.
    ///
    /// A name scoped to the sheet asking wins over one that covers the whole
    /// workbook, which is how Excel lets two sheets give the same name two
    /// meanings.
    fn name(&mut self, origin: Origin, name: &str) -> Value {
        // A `LET` binding or a lambda parameter is in view before anything the
        // workbook defines.
        if let Some(value) = self.bound(name) {
            return value;
        }
        // A name may be written qualified, as `Sheet1!Total`; the sheet part
        // only says which scope to look in.
        let (scope, bare) = self.name_scope(origin, name);
        let Some(found) = self.defined_name(bare, scope) else {
            return Value::Error(CellError::Name);
        };
        let key = bare.to_lowercase();
        if !self.resolving.insert(key.clone()) {
            // A name that stands for itself has nothing to stand for.
            return Value::Error(CellError::Ref);
        }
        let formula = self.book.defined_names[found].formula.as_str();
        let value = match self.parsed(formula) {
            Some(expr) => self.eval_expr(origin, &expr),
            None => Value::Error(CellError::Name),
        };
        self.resolving.remove(&key);
        value
    }

    /// The defined name `name` means from a sheet, by its position in the
    /// book's list: the sheet's own name wins over the workbook's, and a name
    /// scoped to another sheet is not in view.
    pub(crate) fn defined_name(&mut self, name: &str, scope: Option<usize>) -> Option<usize> {
        let book = self.book;
        let index = self.name_index.get_or_insert_with(|| {
            let mut index: HashMap<String, Vec<usize>> = HashMap::new();
            for (i, defined) in book.defined_names.iter().enumerate() {
                index
                    .entry(defined.name.to_lowercase())
                    .or_default()
                    .push(i);
            }
            index
        });
        let candidates = index.get(&name.to_lowercase())?;
        let names = &book.defined_names;
        candidates
            .iter()
            .copied()
            .find(|&i| names[i].sheet.is_some() && names[i].sheet == scope)
            .or_else(|| {
                candidates
                    .iter()
                    .copied()
                    .find(|&i| names[i].sheet.is_none())
            })
    }

    /// A formula text parsed once however often it is asked for.
    pub(crate) fn parsed(&mut self, text: &str) -> Option<Arc<Expr>> {
        if let Some(expr) = self.parsed.get(text) {
            return expr.clone();
        }
        let expr = parse(text).ok().map(Arc::new);
        self.parsed.insert(text.to_owned(), expr.clone());
        expr
    }

    /// Reads a reference, as a scalar for one cell and as an array otherwise.
    /// Reads the values of a rectangle of cells.
    /// The sheet and rectangle a structured reference means.
    ///
    /// The table is found by name across the workbook, because a table name is
    /// unique in it; an unqualified `[Column]` means the table the formula
    /// itself sits in, which is how Excel writes a calculated column.
    ///
    /// # Errors
    /// [`CellError::Name`] when no table answers to the name or the column is
    /// not one of its columns, [`CellError::Ref`] when the part asked for is
    /// not there, as `[#Totals]` on a table with no totals row.
    fn resolve_table(
        &self,
        origin: Origin,
        reference: &Structured,
    ) -> core::result::Result<(String, Range), CellError> {
        let (sheet, table) = self.find_table(origin, reference)?;
        let header = table.header_row_count.unwrap_or(1);
        let totals = table.totals_row_count.unwrap_or(0);
        let (top, bottom) = (table.range.start.row.index(), table.range.end.row.index());
        let body = (top + header, bottom.saturating_sub(totals));

        let (first, last) = match reference.part {
            TablePart::All => (top, bottom),
            TablePart::Data => body,
            TablePart::Headers => (top, top + header.saturating_sub(1)),
            TablePart::Totals => (bottom.saturating_sub(totals.saturating_sub(1)), bottom),
            TablePart::HeadersData => (top, body.1),
            TablePart::DataTotals => (body.0, bottom),
            // The row the formula sits on, which has to be one of the table's.
            TablePart::ThisRow => {
                let row = origin.at.row.index();
                if row < top || row > bottom {
                    return Err(CellError::Value);
                }
                (row, row)
            }
        };
        if first > last || (reference.part == TablePart::Totals && totals == 0) {
            return Err(CellError::Ref);
        }

        // A named column narrows the width; without one the reference is as
        // wide as the table.
        let (left, right) = match &reference.columns {
            None => (table.range.start.col.index(), table.range.end.col.index()),
            Some((first_name, last_name)) => {
                let offset = |name: &String| {
                    table
                        .columns
                        .iter()
                        .position(|c| c.name.eq_ignore_ascii_case(name))
                        .and_then(|i| u32::try_from(i).ok())
                        .map(|i| table.range.start.col.index() + i)
                        .ok_or(CellError::Name)
                };
                let start = offset(first_name)?;
                let end = match last_name {
                    Some(name) => offset(name)?,
                    None => start,
                };
                (start.min(end), start.max(end))
            }
        };

        let corner = |col: u32, row: u32| Some(CellRef::new(Col::new(col)?, Row::new(row)?));
        let (start, end) = (corner(left, first), corner(right, last));
        match (start, end) {
            (Some(start), Some(end)) => Ok((sheet, Range { start, end })),
            _ => Err(CellError::Ref),
        }
    }

    /// The table a structured reference names, and the sheet it sits on.
    fn find_table<'t>(
        &'t self,
        origin: Origin,
        reference: &Structured,
    ) -> core::result::Result<(String, &'t crate::model::table::Table), CellError> {
        let Some(name) = &reference.table else {
            // No name: the table this very formula is written inside.
            let sheet = self.book.sheet(origin.sheet).ok_or(CellError::Ref)?;
            let table = sheet
                .tables
                .iter()
                .find(|t| t.range.contains(origin.at))
                .ok_or(CellError::Name)?;
            return Ok((sheet.title().to_owned(), table));
        };
        self.book
            .sheets()
            .iter()
            .find_map(|sheet| {
                let table = sheet
                    .tables
                    .iter()
                    .find(|t| t.display_name.eq_ignore_ascii_case(name))?;
                Some((sheet.title().to_owned(), table))
            })
            .ok_or(CellError::Name)
    }

    pub(crate) fn range(&mut self, origin: Origin, sheet: Option<&str>, range: Range) -> Value {
        if let Some((book, name)) = sheet.and_then(external_ref) {
            return self.external(book, name, range);
        }
        let Some(index) = self.sheet_index(origin, sheet) else {
            return Value::Error(CellError::Ref);
        };
        let Some(range) = self.clip(index, range) else {
            return Value::Blank;
        };
        if range.start == range.end {
            return self.cell(index, range.start);
        }
        if (range.width() as usize).saturating_mul(range.height() as usize) > MAX_RANGE_CELLS {
            return Value::Error(CellError::Value);
        }
        if let Some(value) = self.ranges.get(&(index, range)) {
            return value.clone();
        }
        if let Some(value) = self.shared.and_then(|sh| sh.range(index, range)) {
            return value;
        }
        let hits = self.cycle_hits;
        let rows = (range.start.row.index()..=range.end.row.index())
            .filter_map(Row::new)
            .map(|row| {
                (range.start.col.index()..=range.end.col.index())
                    .filter_map(Col::new)
                    .map(|col| self.cell(index, CellRef::new(col, row)))
                    .collect()
            })
            .collect();
        let value = Value::array(rows);
        if self.cycle_hits == hits && self.deferred.is_none() {
            match self.shared {
                Some(sh) => sh.keep_range(index, range, &value),
                None => {
                    self.ranges.insert((index, range), value.clone());
                }
            }
        }
        value
    }

    /// Trims a reference to the part of the sheet that holds anything.
    ///
    /// `SUM(A:A)` must not build a million values; the end of the reference is
    /// pulled back to the last used row and column, while its start stays put
    /// so that offsets counted from it - `INDEX`, `VLOOKUP` - still land where
    /// the formula meant them to.
    fn clip(&mut self, sheet: usize, range: Range) -> Option<Range> {
        let used = if let Some(used) = self.dims.get(sheet).copied().flatten() {
            used
        } else {
            let used = self.book.sheet(sheet)?.dimension()?;
            if let Some(slot) = self.dims.get_mut(sheet) {
                *slot = Some(used);
            }
            used
        };
        if range.start.row > used.end.row || range.start.col > used.end.col {
            return None;
        }
        Some(Range {
            start: range.start,
            end: CellRef::new(
                range.end.col.min(used.end.col),
                range.end.row.min(used.end.row),
            ),
        })
    }

    /// Resolves a sheet name to its tab index.
    /// Reads a reference into a linked workbook out of its cached values.
    ///
    /// The cache holds only the cells the links asked for, so a gap in it is
    /// blank rather than an error: that is what Excel shows too until the link
    /// is refreshed. A book or sheet that is not there at all is `#REF!`.
    fn external(&mut self, book: usize, sheet: &str, range: Range) -> Value {
        let Some(cells) = self
            .book
            .external
            .get(book)
            .and_then(|b| b.sheet(sheet))
            .map(|s| &s.cells)
        else {
            return Value::Error(CellError::Ref);
        };
        let at = |row, col| match cells.get(&(row, col)) {
            None | Some(CellValue::Empty) => Value::Blank,
            Some(CellValue::Number(n)) => Value::Number(*n),
            Some(CellValue::Text(t)) => Value::Text(t.to_string()),
            Some(CellValue::Bool(b)) => Value::Bool(*b),
            Some(CellValue::Error(e)) => Value::Error(*e),
            // The cache never holds a formula or rich text: it is values only.
            Some(other) => Value::Text(other.plain_text().unwrap_or_default()),
        };
        if range.start == range.end {
            return at(range.start.row, range.start.col);
        }
        // A whole-column reference into a cache would build a million blanks;
        // the last cached cell is as far as there is anything to read.
        let Some(&(last_row, _)) = cells.keys().next_back() else {
            return Value::Blank;
        };
        let end = range.end.row.min(last_row);
        let rows = (range.start.row.index()..=end.index())
            .filter_map(Row::new)
            .map(|row| {
                (range.start.col.index()..=range.end.col.index())
                    .filter_map(Col::new)
                    .map(|col| at(row, col))
                    .collect()
            })
            .collect();
        Value::array(rows)
    }

    pub(crate) fn sheet_index(&self, origin: Origin, sheet: Option<&str>) -> Option<usize> {
        match sheet {
            None => Some(origin.sheet),
            Some(name) => self
                .book
                .sheets()
                .iter()
                .position(|s| same_name(s.title(), name)),
        }
    }

    fn binary(&mut self, origin: Origin, op: BinaryOp, a: &Expr, b: &Expr) -> Value {
        // The reference operators work on the references themselves, so they
        // are handled before either side is read.
        match op {
            BinaryOp::Intersect => return self.intersect(origin, a, b),
            BinaryOp::Union => return self.union(origin, a, b),
            BinaryOp::Span => return self.span(origin, a, b),
            _ => {}
        }
        let (x, y) = (self.eval_value(origin, a), self.eval_value(origin, b));
        binary_values(op, &x, &y)
    }

    /// The cells two references have in common.
    fn intersect(&mut self, origin: Origin, a: &Expr, b: &Expr) -> Value {
        let (
            Expr::Range {
                sheet, range: x, ..
            },
            Expr::Range { range: y, .. },
        ) = (a, b)
        else {
            return Value::Error(CellError::Value);
        };
        let (start, end) = (
            CellRef::new(x.start.col.max(y.start.col), x.start.row.max(y.start.row)),
            CellRef::new(x.end.col.min(y.end.col), x.end.row.min(y.end.row)),
        );
        if start.col > end.col || start.row > end.row {
            // Excel calls an empty intersection `#NULL!`.
            return Value::Error(CellError::Null);
        }
        self.range(origin, sheet.as_deref(), Range { start, end })
    }

    /// The smallest rectangle holding both references.
    ///
    /// `A1:A2:B1` is `A1:B2`: a colon between two areas spans them, the same
    /// as it does between two cells.
    fn span(&mut self, origin: Origin, a: &Expr, b: &Expr) -> Value {
        let Some((sheet, range)) = spanned(&Expr::Binary(
            BinaryOp::Span,
            Box::new(a.clone()),
            Box::new(b.clone()),
        )) else {
            return Value::Error(CellError::Value);
        };
        self.range(origin, sheet.as_deref(), range)
    }

    /// Both areas of a union, one after the other.
    fn union(&mut self, origin: Origin, left: &Expr, right: &Expr) -> Value {
        let areas = [self.eval_expr(origin, left), self.eval_expr(origin, right)];
        let mut rows = Vec::new();
        for v in areas {
            match v {
                Value::Array(r) => rows.extend(std::sync::Arc::unwrap_or_clone(r)),
                other => rows.push(vec![other]),
            }
        }
        Value::array(rows)
    }
}

/// A position written into `INDEX` as a literal: 0 or left out for the whole
/// row or column. Excel truncates a fractional one.
fn position(e: Option<&Expr>) -> Option<u32> {
    match e {
        None | Some(Expr::Missing) => Some(0),
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the guard keeps it within a sheet's rows"
        )]
        Some(Expr::Number(n)) if (0.0..1_048_577.0).contains(n) => Some(n.trunc() as u32),
        _ => None,
    }
}

/// Whether two sheet or defined names are the same name. Excel compares them
/// without case, Cyrillic included, and every qualified reference and every
/// name in a formula asks this, so it compares in place rather than
/// lower-casing copies of both.
pub(crate) fn same_name(a: &str, b: &str) -> bool {
    a == b
        || a.chars()
            .flat_map(char::to_lowercase)
            .eq(b.chars().flat_map(char::to_lowercase))
}

/// Applies a one-operand operator.
///
/// A leading `+` is no operator at all in Excel: `=+Sheet!A1` is how Lotus
/// users write a reference, and it gives the text or the logical the cell
/// holds, not `#VALUE!` for failing to be a number.
fn unary(op: UnaryOp, v: &Value) -> Value {
    if op == UnaryOp::Plus {
        return v.clone();
    }
    if let Value::Array(rows) = v {
        return Value::array(
            rows.iter()
                .map(|r| r.iter().map(|x| unary(op, x)).collect())
                .collect(),
        );
    }
    let n = match v.number() {
        Ok(n) => n,
        Err(e) => return Value::Error(e),
    };
    Value::Number(match op {
        UnaryOp::Neg => -n,
        UnaryOp::Plus => n,
        UnaryOp::Percent => n / 100.0,
    })
}

/// Applies a two-operand operator to values that are already computed.
///
/// Arrays are combined element by element, the way an array formula does.
#[must_use]
pub fn binary_values(op: BinaryOp, a: &Value, b: &Value) -> Value {
    if matches!(a, Value::Array(_)) || matches!(b, Value::Array(_)) {
        return broadcast(op, a, b);
    }
    match op {
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
            if let Some(e) = a.error().or_else(|| b.error()) {
                return Value::Error(e);
            }
            let ord = compare(a, b);
            Value::Bool(match op {
                BinaryOp::Eq => ord.is_eq(),
                BinaryOp::Ne => ord.is_ne(),
                BinaryOp::Lt => ord.is_lt(),
                BinaryOp::Le => ord.is_le(),
                BinaryOp::Gt => ord.is_gt(),
                _ => ord.is_ge(),
            })
        }
        BinaryOp::Concat => match (a.text(), b.text()) {
            (Ok(x), Ok(y)) => Value::Text(x + &y),
            (Err(e), _) | (_, Err(e)) => Value::Error(e),
        },
        _ => {
            let (left, right) = match (a.number(), b.number()) {
                (Ok(x), Ok(y)) => (x, y),
                (Err(e), _) | (_, Err(e)) => return Value::Error(e),
            };
            match op {
                BinaryOp::Add => Value::Number(left + right),
                BinaryOp::Sub => Value::Number(left - right),
                BinaryOp::Mul => Value::Number(left * right),
                BinaryOp::Div => {
                    if right == 0.0 {
                        Value::Error(CellError::Div0)
                    } else {
                        Value::Number(left / right)
                    }
                }
                BinaryOp::Pow => {
                    let r = left.powf(right);
                    if r.is_finite() {
                        Value::Number(r)
                    } else {
                        // A power Excel cannot express, such as a negative
                        // base with a fractional exponent.
                        Value::Error(CellError::Num)
                    }
                }
                _ => Value::Error(CellError::Value),
            }
        }
    }
}

/// Combines two operands element by element, stretching a scalar or a single
/// row or column over the shape of the other side.
fn broadcast(op: BinaryOp, a: &Value, b: &Value) -> Value {
    let (ar, ac) = shape(a);
    let (br, bc) = shape(b);
    let (rows, cols) = (ar.max(br), ac.max(bc));
    if rows.saturating_mul(cols) > MAX_RANGE_CELLS {
        return Value::Error(CellError::Num);
    }
    let out = (0..rows)
        .map(|r| {
            (0..cols)
                .map(|c| binary_values(op, &at(a, r, c), &at(b, r, c)))
                .collect()
        })
        .collect();
    Value::array(out)
}

/// Rows and columns a value covers; a scalar covers one of each.
pub(crate) fn shape(v: &Value) -> (usize, usize) {
    match v {
        Value::Array(rows) => (rows.len(), rows.iter().map(Vec::len).max().unwrap_or(0)),
        _ => (1, 1),
    }
}

/// The element at a position, repeating the only row or column when the value
/// is narrower than the shape being filled - Excel stretches a single row or
/// column across the whole result and pads the rest with `#N/A`.
pub(crate) fn at(v: &Value, row: usize, col: usize) -> Value {
    match v {
        Value::Array(rows) => {
            let r = if rows.len() == 1 { 0 } else { row };
            let Some(line) = rows.get(r) else {
                return Value::Error(CellError::Na);
            };
            let c = if line.len() == 1 { 0 } else { col };
            line.get(c).cloned().unwrap_or(Value::Error(CellError::Na))
        }
        other => other.clone(),
    }
}

/// The reference an expression names, following chains of `:`.
///
/// `E5:H7:B1` is one reference - the rectangle covering all of them - and it
/// is the reference `ROWS` and `COLUMNS` measure, not the values behind it.
#[must_use]
pub fn spanned(expr: &Expr) -> Option<(Option<String>, Range)> {
    match expr {
        Expr::Range { sheet, range, .. } => Some((sheet.clone(), *range)),
        Expr::Binary(BinaryOp::Span, a, b) => {
            let (sheet, x) = spanned(a)?;
            let (_, y) = spanned(b)?;
            Some((
                sheet,
                Range {
                    start: CellRef::new(x.start.col.min(y.start.col), x.start.row.min(y.start.row)),
                    end: CellRef::new(x.end.col.max(y.end.col), x.end.row.max(y.end.row)),
                },
            ))
        }
        _ => None,
    }
}

/// Recomputes every formula of a workbook, or of one sheet, and stores each
/// result as that cell's cached value. Returns how many formulas were computed.
///
/// The cache beside a formula is whatever application saved the file last, so
/// this is what makes it ours: after it, a reader that trusts the cache - CSV,
/// HTML, or anything reading the value rather than the formula - sees this
/// engine's answers.
///
/// Reading and writing cannot overlap (the engine borrows the workbook), so the
/// results are collected first and stored after. Every formula is therefore
/// computed against the values the file was opened with, which is also what
/// makes the pass independent of the order cells are visited in.
///
/// `options` carries the two things a pass over a whole workbook needs from
/// the outside: somewhere to report how far along it is, and the functions a
/// workbook may call that this crate does not define. Neither of those is
/// wanted often enough to have its own entry point, so a caller with neither
/// passes `&Options::default()`.
pub fn recalculate(book: &mut Spreadsheet, sheet: Option<usize>, options: &Options<'_>) -> usize {
    // The trees are kept beside the index for this one pass, so each formula
    // is parsed once rather than once to index it and again to compute it.
    let (deps, trees) = Dependencies::of_with_trees(book);
    // Formulas the index knows come first, each after what it reads; one that
    // does not parse is not in the index and is computed after, where it
    // answers `#NAME?` on its own.
    let indexed: Vec<usize> = (0..deps.formulas.len())
        .filter(|&i| sheet.is_none_or(|only| only == deps.formulas[i].sheet))
        .collect();
    let mut results = pass(book, &deps, &indexed, &|i| &trees[i], None, options);
    let known: HashSet<(usize, CellRef)> = indexed
        .iter()
        .map(|&i| (deps.formulas[i].sheet, deps.formulas[i].at))
        .collect();
    {
        let mut engine = match options.functions() {
            Some(custom) => Engine::with_functions(book, custom),
            None => Engine::new(book),
        };
        for (index, s) in book.sheets().iter().enumerate() {
            if sheet.is_some_and(|only| only != index) {
                continue;
            }
            for (at, cell) in s.iter() {
                if let CellValue::Formula { formula, .. } = &cell.value
                    && !known.contains(&(index, at))
                {
                    let value = engine.eval(Origin::new(index, at), formula);
                    results.push((index, at, value));
                }
            }
        }
    }
    store(book, results)
}

/// Computes `nodes` and answers what each one worked out, in waves so that a
/// formula comes after everything it reads.
///
/// `stale` is the set of formula cells the pass recomputes; outside it a
/// formula answers with the result stored beside it rather than being
/// computed again. `None` recomputes whatever is read, which is what a full
/// pass wants.
fn pass<'t>(
    book: &Spreadsheet,
    deps: &Dependencies,
    nodes: &[usize],
    tree_of: &(dyn Fn(usize) -> &'t Expr + Sync),
    stale: Option<&HashSet<(usize, CellRef)>>,
    options: &Options<'_>,
) -> Vec<(usize, CellRef, Value)> {
    let (levels, tail) = deps.levels(nodes);
    let threads = workers(nodes.len(), options);
    let shared = (threads > 1).then(|| Shared {
        slots: nodes
            .iter()
            .enumerate()
            .map(|(slot, &i)| ((deps.formulas[i].sheet, deps.formulas[i].at), slot))
            .collect(),
        values: nodes.iter().map(|_| std::sync::OnceLock::new()).collect(),
        claimed: nodes
            .iter()
            .map(|_| std::sync::atomic::AtomicBool::new(false))
            .collect(),
        ranges: std::sync::RwLock::default(),
    });
    let mut engine = match options.functions() {
        Some(custom) => Engine::with_functions(book, custom),
        None => Engine::new(book),
    };
    engine.stale = stale;
    let mut results = Vec::with_capacity(nodes.len());
    let total = nodes.len();
    let mut done = 0;
    let mut compute = |engine: &mut Engine<'_>, i: usize, results: &mut Vec<_>| {
        options.report(Stage::Recalculating, done, Some(total), "");
        done += 1;
        let Node { sheet, at, .. } = deps.formulas[i];
        let value = engine.eval_tree(Origin::new(sheet, at), tree_of(i));
        engine.remember(sheet, at, &value);
        results.push((sheet, at, value));
    };
    if let Some(shared) = &shared {
        parallel(
            book, deps, tree_of, nodes, &levels, shared, stale, threads, options,
        );
        // What is left goes round a cycle, and only one engine can see that
        // happen.
        engine.shared = Some(shared);
        for (slot, &i) in nodes.iter().enumerate() {
            if let Some(value) = shared.values[slot].get() {
                results.push((deps.formulas[i].sheet, deps.formulas[i].at, value.clone()));
            }
        }
        for &i in &tail {
            compute(&mut engine, i, &mut results);
        }
    } else {
        for i in levels.into_iter().flatten().chain(tail) {
            compute(&mut engine, i, &mut results);
        }
    }
    results
}

/// Writes what a pass worked out into the cells' caches, and answers how many
/// there were.
fn store(book: &mut Spreadsheet, results: Vec<(usize, CellRef, Value)>) -> usize {
    let computed = results.len();
    for (index, at, value) in results {
        let Some(sheet) = book.sheet_mut(index) else {
            continue;
        };
        if let CellValue::Formula { cached, .. } = &mut sheet.entry(at).value {
            *cached = Some(Box::new(stored(&value)));
        }
    }
    computed
}

/// A computed value as it is stored in a cell: an array shows its top-left
/// value, the way a single cell can only show one.
/// Recomputes one formula and stores the result as that cell's cached value.
/// Returns whether there was a formula there to compute.
///
/// This is not [`recalculate_from`]: that one answers "I changed this cell",
/// and recomputes the formulas *reading* `at` while leaving `at` alone. This
/// one recomputes `at` itself and nothing else - what a cell holding
/// `=NOW()` or a formula whose function the caller just registered needs.
///
/// Whatever the formula reads is computed on the way, but only in the engine's
/// own cache: no other cell of the workbook is written.
pub fn recalculate_cell(book: &mut Spreadsheet, sheet: usize, at: CellRef) -> bool {
    recalculate_cell_with(book, sheet, at, &Options::default())
}

/// The same, told which of the caller's functions to know about.
pub fn recalculate_cell_with(
    book: &mut Spreadsheet,
    sheet: usize,
    at: CellRef,
    options: &Options<'_>,
) -> bool {
    let value = {
        let Some(CellValue::Formula { formula, .. }) =
            book.sheet(sheet).and_then(|s| s.get(at)).map(|c| &c.value)
        else {
            return false;
        };
        let formula = formula.clone();
        let mut engine = match options.functions() {
            Some(custom) => Engine::with_functions(book, custom),
            None => Engine::new(book),
        };
        engine.eval(Origin::new(sheet, at), &formula)
    };
    let Some(s) = book.sheet_mut(sheet) else {
        return false;
    };
    let CellValue::Formula { cached, .. } = &mut s.entry(at).value else {
        return false;
    };
    *cached = Some(Box::new(stored(&value)));
    true
}

/// How long an engine waits for another to finish a formula it wants, in
/// turns of the scheduler. Long enough for a formula of a few hundred cells,
/// short enough that a wait on a cycle is over before it is felt.
const WAIT_SPINS: usize = 1024;

/// How many threads a pass over `formulas` formulas should use: one for a
/// small book, for a caller's functions (which need not be safe to share), and
/// in WebAssembly, which has no threads.
fn workers(formulas: usize, options: &Options<'_>) -> usize {
    // Below this a pass is quicker than starting the threads.
    const MIN_PARALLEL: usize = 20_000;
    if cfg!(target_arch = "wasm32") || formulas < MIN_PARALLEL || options.functions().is_some() {
        return 1;
    }
    // Past eight the engines spend more time on each other than on formulas:
    // COIN computes slower on sixteen than on eight.
    std::thread::available_parallelism().map_or(1, |n| n.get().min(8))
}

/// Computes the waves of formulas on `threads` threads, each with an engine of
/// its own, into `shared`.
///
/// A wave with few formulas is computed by the calling thread alone while the
/// others wait for the next large one: waking eight threads for one formula
/// costs more than the formula.
#[expect(clippy::too_many_arguments, reason = "the parts of one pass, borrowed")]
fn parallel<'t>(
    book: &Spreadsheet,
    deps: &Dependencies,
    tree_of: &(dyn Fn(usize) -> &'t Expr + Sync),
    indexed: &[usize],
    levels: &[Vec<usize>],
    shared: &Shared,
    stale: Option<&HashSet<(usize, CellRef)>>,
    threads: usize,
    options: &Options<'_>,
) {
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};
    const MIN_WAVE: usize = 256;
    let slot_of: HashMap<usize, usize> = indexed
        .iter()
        .enumerate()
        .map(|(slot, &i)| (i, slot))
        .collect();
    let cursors: Vec<AtomicUsize> = levels.iter().map(|_| AtomicUsize::new(0)).collect();
    let done = AtomicUsize::new(0);
    let barrier = Barrier::new(threads);
    let total = indexed.len();
    let work = |main: bool, report: &dyn Fn(usize)| {
        let mut engine = Engine::new(book);
        engine.shared = Some(shared);
        engine.stale = stale;
        for (level, wave) in levels.iter().enumerate() {
            let large = wave.len() >= MIN_WAVE;
            if !large && !main {
                continue;
            }
            if large {
                barrier.wait();
            }
            loop {
                let k = cursors[level].fetch_add(1, Ordering::Relaxed);
                let Some(&i) = wave.get(k) else { break };
                let Node { sheet, at, .. } = deps.formulas[i];
                let value = engine.eval_tree(Origin::new(sheet, at), tree_of(i));
                let _ = shared.values[slot_of[&i]].set(value);
                let finished = done.fetch_add(1, Ordering::Relaxed);
                report(finished);
            }
            if large {
                // Everyone finishes the wave before the next one starts.
                barrier.wait();
            }
        }
    };
    std::thread::scope(|scope| {
        for _ in 1..threads {
            scope.spawn(|| work(false, &|_| {}));
        }
        work(true, &|finished| {
            options.report(Stage::Recalculating, finished, Some(total), "");
        });
    });
}

/// What a value stored in a cell is to a formula reading it.
pub(crate) fn stored_value(cell: &CellValue) -> Value {
    match cell {
        CellValue::Number(n) => Value::Number(*n),
        CellValue::Text(t) => Value::Text(t.to_string()),
        // Formatting inside the cell is presentation; a formula reads the
        // text it spells.
        rich @ CellValue::RichText(_) => Value::Text(rich.plain_text().unwrap_or_default()),
        CellValue::Bool(b) => Value::Bool(*b),
        CellValue::Error(e) => Value::Error(*e),
        // A formula holding a formula is not something a file says.
        CellValue::Empty | CellValue::Formula { .. } => Value::Blank,
    }
}

/// A computed value as a cell holds it: an array shows its top-left value,
/// and a function, which no cell can hold, the error Excel shows instead.
pub(crate) fn stored(value: &Value) -> CellValue {
    match value {
        Value::Blank => CellValue::Empty,
        Value::Number(n) => CellValue::Number(*n),
        Value::Text(t) => CellValue::Text(t.as_str().into()),
        Value::Bool(b) => CellValue::Bool(*b),
        Value::Error(e) => CellValue::Error(*e),
        // A lambda is not something a cell can hold, so what is stored is the
        // error Excel shows in its place.
        Value::Lambda(_) => CellValue::Error(CellError::Calc),
        Value::Array(rows) => rows
            .first()
            .and_then(|r| r.first())
            .map_or(CellValue::Empty, stored),
    }
}

/// The formulas and the rectangles between them, as [`Dependencies::graph`]
/// builds them.
#[derive(Default)]
struct Graph {
    /// Which rectangle each id stands for.
    ranges: Vec<(usize, Range)>,
    /// The formulas inside each rectangle.
    members: Vec<Vec<usize>>,
    /// The formulas reading each rectangle.
    readers: Vec<Vec<usize>>,
    /// The rectangles each formula reads, by formula.
    reads: Vec<Vec<usize>>,
    /// The rectangles each formula sits in, by formula.
    member_of: Vec<Vec<usize>>,
}

/// One formula of a workbook: where it sits, what it reads, and whether it
/// has to be recomputed whatever else changed.
struct Node {
    sheet: usize,
    at: CellRef,
    reads: Vec<(usize, Range)>,
    always: bool,
}

/// Functions whose answer does not follow from what `reads` holds: the clock
/// and the random generator answer differently on every call, and `OFFSET`,
/// `INDIRECT`, `CELL` and `INFO` build the reference they read at evaluation
/// time, so no static walk of the formula can see it. Excel calls them
/// volatile and recomputes them on every pass; a formula calling one is
/// therefore recomputed on every edit.
const VOLATILE: [&str; 8] = [
    "NOW",
    "TODAY",
    "RAND",
    "RANDBETWEEN",
    "OFFSET",
    "INDIRECT",
    "CELL",
    "INFO",
];

/// What every formula of a workbook reads, so that a cell edit can be answered
/// without evaluating - or even parsing - the whole book again.
///
/// Building it parses every formula once, which on a large workbook is most of
/// what a recalculation costs. Keep one across a run of edits; the free
/// [`recalculate_from`] builds a throwaway for a single edit.
///
/// The index describes the formulas as they were when it was built. Editing a
/// cell's *value* leaves it valid; adding, changing or deleting a *formula*
/// does not, so tell it with [`Dependencies::note`].
pub struct Dependencies {
    formulas: Vec<Node>,
}

impl Dependencies {
    /// Reads every formula of the workbook and remembers what it depends on.
    #[must_use]
    pub fn of(book: &Spreadsheet) -> Self {
        Self::of_with_trees(book).0
    }

    /// The same, with each formula's parsed tree at the same position.
    fn of_with_trees(book: &Spreadsheet) -> (Self, Vec<Expr>) {
        let mut formulas = Vec::new();
        let mut trees = Vec::new();
        let cells: Vec<(usize, CellRef, &CellValue)> = book
            .sheets()
            .iter()
            .enumerate()
            .flat_map(|(index, sheet)| {
                sheet
                    .iter()
                    .filter(|(_, cell)| matches!(cell.value, CellValue::Formula { .. }))
                    .map(move |(at, cell)| (index, at, &cell.value))
            })
            .collect();
        // Parsing is most of what building the index costs, and every formula
        // parses on its own, so the cells are split between threads.
        let parse_chunk = |chunk: &[(usize, CellRef, &CellValue)]| {
            let mut refs = Refs::new(book);
            chunk
                .iter()
                .filter_map(|&(index, at, value)| node_of(&mut refs, index, at, value))
                .collect::<Vec<_>>()
        };
        let threads = workers(cells.len(), &Options::default());
        let parts: Vec<Vec<(Node, Expr)>> = if threads > 1 {
            let size = cells.len().div_ceil(threads);
            std::thread::scope(|scope| {
                let handles: Vec<_> = cells
                    .chunks(size)
                    .map(|chunk| scope.spawn(move || parse_chunk(chunk)))
                    .collect();
                handles.into_iter().filter_map(|h| h.join().ok()).collect()
            })
        } else {
            vec![parse_chunk(&cells)]
        };
        for (node, tree) in parts.into_iter().flatten() {
            formulas.push(node);
            trees.push(tree);
        }
        (Self { formulas }, trees)
    }

    /// How many formulas the index holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.formulas.len()
    }

    /// Whether the workbook holds no formulas at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.formulas.is_empty()
    }

    /// Brings one cell up to date, after its formula was written, changed or
    /// removed. A cell that only changed value needs no call.
    pub fn note(&mut self, book: &Spreadsheet, sheet: usize, at: CellRef) {
        self.formulas
            .retain(|node| node.sheet != sheet || node.at != at);
        if let Some(value) = book.sheet(sheet).and_then(|s| s.get(at)).map(|c| &c.value)
            && let Some((node, _)) = node_of(&mut Refs::new(book), sheet, at, value)
        {
            self.formulas.push(node);
        }
    }

    /// Orders `nodes` so that a formula comes after every formula it reads.
    ///
    /// A formula is computed when something reads it, which recurses, and a
    /// chain thousands of links long would recurse thousands deep. Computed in
    /// this order every link is already in the engine's cache when the next one
    /// asks for it, so the recursion stays one deep and [`MAX_CHAIN_DEPTH`] is
    /// never in the way of a workbook that merely has a long column.
    ///
    /// Formulas caught in a cycle cannot be ordered; they are left at the end,
    /// where the engine answers them `#REF!` the way it always has.
    fn ordered(&self, nodes: &[usize]) -> Vec<usize> {
        let (levels, tail) = self.levels(nodes);
        levels.into_iter().flatten().chain(tail).collect()
    }

    /// Every formula that has to be computed again after `changed` was
    /// written: the ones reading those cells, the ones reading those, and the
    /// ones that are recomputed whatever moved (a defined name, a volatile
    /// function).
    ///
    /// Walked over the graph, so each formula and each rectangle is looked at
    /// once. Asking every formula whether any dirty cell is inside what it
    /// reads, and asking it again on every round, cost the dirty set times the
    /// reads: one edit in a workbook of 650,000 formulas did not finish.
    fn affected(&self, changed: &[(usize, CellRef)]) -> Vec<usize> {
        let all: Vec<usize> = (0..self.formulas.len()).collect();
        let graph = self.graph(&all);
        let mut dirty = vec![false; self.formulas.len()];
        let mut queue: Vec<usize> = Vec::new();
        let mut dirty_range = vec![false; graph.members.len()];
        let mut ranges: Vec<usize> = Vec::new();
        // The edited cells themselves: which rectangles hold them.
        for (id, &(sheet, range)) in graph.ranges.iter().enumerate() {
            if changed
                .iter()
                .any(|&(s, at)| s == sheet && range.contains(at))
            {
                dirty_range[id] = true;
                ranges.push(id);
            }
        }
        for (i, node) in self.formulas.iter().enumerate() {
            if node.always {
                dirty[i] = true;
                queue.push(i);
            }
        }
        while !queue.is_empty() || !ranges.is_empty() {
            while let Some(id) = ranges.pop() {
                for &r in &graph.readers[id] {
                    if !dirty[r] {
                        dirty[r] = true;
                        queue.push(r);
                    }
                }
            }
            while let Some(i) = queue.pop() {
                for &id in &graph.member_of[i] {
                    if !dirty_range[id] {
                        dirty_range[id] = true;
                        ranges.push(id);
                    }
                }
            }
        }
        all.into_iter().filter(|&i| dirty[i]).collect()
    }

    /// The graph a pass walks: the formulas and the rectangles between them.
    ///
    /// Edges run through the ranges rather than straight from formula to
    /// formula: a formula reads each distinct range and a range holds the
    /// formulas inside it. Edges straight between formulas number the reads
    /// times the formulas inside each, and fifty thousand formulas reading one
    /// table of fifty thousand is billions of them; through the range it is
    /// fifty thousand plus fifty thousand.
    fn graph(&self, nodes: &[usize]) -> Graph {
        // Formulas of one sheet in address order, so a range finds the ones
        // inside it without a scan of the whole workbook.
        let mut by_sheet: HashMap<usize, Vec<(CellRef, usize)>> = HashMap::new();
        for &i in nodes {
            by_sheet
                .entry(self.formulas[i].sheet)
                .or_default()
                .push((self.formulas[i].at, i));
        }
        for list in by_sheet.values_mut() {
            list.sort_unstable();
        }
        // Which distinct rectangles are read, and by whom. Kept in vectors
        // indexed by formula rather than in maps: a workbook of 650,000
        // formulas has millions of these edges, and hashing each of them was
        // most of what the walk cost.
        let mut ids: HashMap<(usize, Range), usize> = HashMap::new();
        let mut graph = Graph {
            reads: vec![Vec::new(); self.formulas.len()],
            member_of: vec![Vec::new(); self.formulas.len()],
            ..Graph::default()
        };
        for &i in nodes {
            for &(sheet, range) in &self.formulas[i].reads {
                let id = *ids.entry((sheet, range)).or_insert_with(|| {
                    graph.readers.push(Vec::new());
                    graph.ranges.push((sheet, range));
                    graph.ranges.len() - 1
                });
                // A formula reads a handful of rectangles, so a look through
                // them beats a set of its own for every formula.
                if !graph.reads[i].contains(&id) {
                    graph.readers[id].push(i);
                    graph.reads[i].push(id);
                }
            }
        }
        // What sits inside each rectangle: one lookup per rectangle, and they
        // are independent of each other.
        graph.members = map_chunks(&graph.ranges, |&(sheet, range)| {
            formulas_in(&by_sheet, sheet, range)
        });
        for (id, members) in graph.members.iter().enumerate() {
            for &j in members {
                graph.member_of[j].push(id);
            }
        }
        graph
    }

    /// The formulas among `nodes` in waves, each after the waves it reads,
    /// and the ones no wave could take - the formulas on a cycle - apart.
    fn levels(&self, nodes: &[usize]) -> (Vec<Vec<usize>>, Vec<usize>) {
        let graph = self.graph(nodes);
        let mut range_left: Vec<usize> = graph.members.iter().map(Vec::len).collect();
        let mut waiting: HashMap<usize, usize> =
            nodes.iter().map(|&i| (i, graph.reads[i].len())).collect();
        let (range_readers, member_of) = (&graph.readers, &graph.member_of);

        // Kahn's walk in waves: a wave is every formula whose inputs the waves
        // before it finished, so the formulas of one wave can be computed in
        // any order, or at the same time.
        let mut wave: Vec<usize> = nodes.iter().copied().filter(|i| waiting[i] == 0).collect();
        let mut ready: Vec<usize> = (0..range_left.len())
            .filter(|&id| range_left[id] == 0)
            .collect();
        let mut release = |ready: &mut Vec<usize>, next: &mut Vec<usize>| {
            while let Some(id) = ready.pop() {
                for &r in &range_readers[id] {
                    if let Some(left) = waiting.get_mut(&r) {
                        *left -= 1;
                        if *left == 0 {
                            next.push(r);
                        }
                    }
                }
            }
        };
        release(&mut ready, &mut wave);
        let mut levels: Vec<Vec<usize>> = Vec::new();
        while !wave.is_empty() {
            let mut next = Vec::new();
            for &i in &wave {
                for &id in &member_of[i] {
                    range_left[id] -= 1;
                    if range_left[id] == 0 {
                        ready.push(id);
                    }
                }
            }
            release(&mut ready, &mut next);
            levels.push(std::mem::replace(&mut wave, next));
        }
        let placed: HashSet<usize> = levels.iter().flatten().copied().collect();
        let tail = nodes
            .iter()
            .copied()
            .filter(|i| !placed.contains(i))
            .collect();
        (levels, tail)
    }

    /// Recomputes every formula that reads one of `changed`, directly or
    /// through other formulas, and stores the results. Returns how many were
    /// computed.
    ///
    /// The whole batch of edits is answered in one pass, so editing a hundred
    /// cells costs one walk of the index rather than a hundred.
    pub fn recalculate_from(&self, book: &mut Spreadsheet, changed: &[(usize, CellRef)]) -> usize {
        self.recalculate_from_with(book, changed, &Options::default())
    }

    /// The same, with the caller's functions and a place to report progress.
    pub fn recalculate_from_with(
        &self,
        book: &mut Spreadsheet,
        changed: &[(usize, CellRef)],
        options: &Options<'_>,
    ) -> usize {
        let nodes = self.affected(changed);
        let stale: HashSet<(usize, CellRef)> = nodes
            .iter()
            .map(|&i| (self.formulas[i].sheet, self.formulas[i].at))
            .collect();
        // The index holds what each formula reads, not the formula itself, so
        // the ones this pass computes are parsed here - beside each other, and
        // on several threads when there are enough of them.
        let texts: Vec<&str> = nodes
            .iter()
            .filter_map(|&i| {
                let Node { sheet, at, .. } = self.formulas[i];
                match book.sheet(sheet).and_then(|s| s.get(at)).map(|c| &c.value) {
                    Some(CellValue::Formula { formula, .. }) => Some(formula.as_str()),
                    _ => None,
                }
            })
            .collect();
        if texts.len() != nodes.len() {
            // A cell that lost its formula since the index was built; the
            // slow path answers correctly whatever happened to the book.
            return self.recompute_one_by_one(book, &nodes, options);
        }
        let trees = parse_all(&texts, options);
        let placed: HashMap<usize, usize> =
            nodes.iter().enumerate().map(|(k, &i)| (i, k)).collect();
        let blank = Expr::Missing;
        let results = pass(
            book,
            self,
            &nodes,
            &|i| placed.get(&i).map_or(&blank, |&k| &trees[k]),
            Some(&stale),
            options,
        );
        store(book, results)
    }

    /// The pass as it was before the waves: one formula at a time, in an order
    /// that puts each after what it reads. Kept for a workbook that changed
    /// under the index.
    fn recompute_one_by_one(
        &self,
        book: &mut Spreadsheet,
        nodes: &[usize],
        options: &Options<'_>,
    ) -> usize {
        let order = self.ordered(nodes);
        let mut results = Vec::new();
        {
            let mut engine = match options.functions() {
                Some(custom) => Engine::with_functions(book, custom),
                None => Engine::new(book),
            };
            let total = order.len();
            for (step, &i) in order.iter().enumerate() {
                options.report(Stage::Recalculating, step, Some(total), "");
                let Node { sheet, at, .. } = self.formulas[i];
                let Some(CellValue::Formula { formula, .. }) =
                    book.sheet(sheet).and_then(|s| s.get(at)).map(|c| &c.value)
                else {
                    continue;
                };
                let value = engine.eval(Origin::new(sheet, at), formula);
                engine.remember(sheet, at, &value);
                results.push((sheet, at, value));
            }
        }
        store(book, results)
    }
}

/// The formulas of `sheet` that sit inside `range`.
///
/// The sheet's formulas are in address order, so the rows the range covers are
/// a slice of that list rather than a scan of it: a workbook where every
/// formula reads `A:A` would otherwise cost a pass over all of them each time.
fn formulas_in(
    by_sheet: &HashMap<usize, Vec<(CellRef, usize)>>,
    sheet: usize,
    range: Range,
) -> Vec<usize> {
    let Some(list) = by_sheet.get(&sheet) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // The list is ordered by column, then row, so each column of the range is
    // a run inside it that binary search finds: `A1:A1` on a sheet whose whole
    // column A is formulas must not cost a walk of that column.
    for col in range.start.col.index()..=range.end.col.index() {
        let Some(col) = Col::new(col) else { continue };
        let first = CellRef::new(col, range.start.row);
        let from = list.partition_point(|(at, _)| *at < first);
        out.extend(
            list[from..]
                .iter()
                .take_while(|(at, _)| at.col == col && at.row <= range.end.row)
                .map(|(_, i)| *i),
        );
    }
    out
}

/// The index entry for a cell, if it holds a formula that parses.
fn node_of(
    refs: &mut Refs<'_>,
    sheet: usize,
    at: CellRef,
    value: &CellValue,
) -> Option<(Node, Expr)> {
    let CellValue::Formula { formula, .. } = value else {
        return None;
    };
    let expr = parse(formula).ok()?;
    let (mut reads, mut always) = (Vec::new(), false);
    collect_refs(&expr, sheet, refs, &mut reads, &mut always);
    Some((
        Node {
            sheet,
            at,
            reads,
            always,
        },
        expr,
    ))
}

/// Maps `items` in chunks, on several threads when there are enough of them.
fn map_chunks<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = workers(items.len(), &Options::default());
    if threads <= 1 {
        return items.iter().map(&f).collect();
    }
    let size = items.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let handles: Vec<_> = items
            .chunks(size)
            .map(|chunk| {
                let f = &f;
                scope.spawn(move || chunk.iter().map(f).collect::<Vec<_>>())
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .flatten()
            .collect()
    })
}

/// Parses a batch of formulas, on several threads when there are enough of
/// them. A formula that does not parse becomes `#NAME?` where it is computed.
fn parse_all(texts: &[&str], options: &Options<'_>) -> Vec<Expr> {
    let one = |text: &&str| parse(text).unwrap_or(Expr::Error(CellError::Name));
    let threads = workers(texts.len(), options);
    if threads <= 1 {
        return texts.iter().map(one).collect();
    }
    let size = texts.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let handles: Vec<_> = texts
            .chunks(size)
            .map(|chunk| scope.spawn(move || chunk.iter().map(one).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .flatten()
            .collect()
    })
}

/// Recomputes only the formulas that depend on `changed`, directly or through
/// other formulas, and stores their results. Returns how many were computed.
///
/// This is what a cell edit needs: [`recalculate`] answers the same, but pays
/// for every formula in the workbook, and a workbook has tens of thousands.
///
/// The index of what reads what is built here and thrown away. For a run of
/// edits, build a [`Dependencies`] once and keep it.
pub fn recalculate_from(book: &mut Spreadsheet, changed: &[(usize, CellRef)]) -> usize {
    Dependencies::of(book).recalculate_from(book, changed)
}

/// The same, with the caller's functions and a place to report progress.
pub fn recalculate_from_with(
    book: &mut Spreadsheet,
    changed: &[(usize, CellRef)],
    options: &Options<'_>,
) -> usize {
    Dependencies::of(book).recalculate_from_with(book, changed, options)
}

/// Every cell range an expression reads, with the sheet each one lives on.
/// `always` is set if the formula reads a defined name or calls a volatile
/// function: either way `reads` may not describe it.
///
/// A name that stands for ranges, and `INDIRECT` handed a literal text, add
/// what they point at as well. The formula stays `always`, but the order of a
/// pass knows to compute those cells first; without it every engine of a
/// parallel pass computed them again for itself.
fn collect_refs(
    expr: &Expr,
    own_sheet: usize,
    refs: &mut Refs<'_>,
    out: &mut Vec<(usize, Range)>,
    always: &mut bool,
) {
    match expr {
        Expr::Range { sheet, range, .. } => {
            let index = match sheet {
                None => Some(own_sheet),
                Some(name) => refs
                    .book
                    .sheets()
                    .iter()
                    .position(|s| same_name(s.title(), name)),
            };
            if let Some(index) = index {
                out.push((index, *range));
            }
        }
        Expr::Name(name) => {
            // A name whose ranges are known is a reference like any other. One
            // that stands for something built at evaluation time, or for
            // nothing this walk can see, keeps the formula in every pass.
            let (ranges, dynamic) = refs.name(own_sheet, name);
            if ranges.is_empty() || dynamic {
                *always = true;
            }
            out.extend(ranges);
        }
        // A table names its cells rather than pointing at them, and where
        // those cells are is a property of the table, not of the formula. So
        // the formula is recalculated whatever moved, the same as one reading
        // a defined name.
        Expr::Structured(_) => *always = true,
        Expr::Unary(_, inner) => collect_refs(inner, own_sheet, refs, out, always),
        Expr::Binary(_, a, b) => {
            collect_refs(a, own_sheet, refs, out, always);
            collect_refs(b, own_sheet, refs, out, always);
        }
        Expr::Call { name, args } => {
            let upper = name.to_ascii_uppercase();
            if VOLATILE.contains(&upper.as_str()) {
                *always = true;
            }
            if upper == "INDIRECT"
                && args.len() == 1
                && let Some(text) = refs.text_of(own_sheet, &args[0])
                && let Ok(target) = parse(&text)
                && matches!(target, Expr::Range { .. } | Expr::Name(_))
            {
                collect_refs(&target, own_sheet, refs, out, always);
            }
            for arg in args {
                collect_refs(arg, own_sheet, refs, out, always);
            }
        }
        Expr::Apply { callee, args } => {
            collect_refs(callee, own_sheet, refs, out, always);
            for arg in args {
                collect_refs(arg, own_sheet, refs, out, always);
            }
        }
        Expr::Array(rows) => {
            for cell in rows.iter().flatten() {
                collect_refs(cell, own_sheet, refs, out, always);
            }
        }
        Expr::Number(_) | Expr::Text(_) | Expr::Bool(_) | Expr::Error(_) | Expr::Missing => {}
    }
}

/// What a defined name was found to stand for: the rectangles, and whether
/// any part of it is built at evaluation time.
type NameReads = (Vec<(usize, Range)>, bool);

/// The workbook a dependency walk reads, and what its names were found to
/// point at, so seven hundred names are not parsed again for every formula.
pub(crate) struct Refs<'b> {
    book: &'b Spreadsheet,
    names: HashMap<(usize, String), NameReads>,
    depth: usize,
}

impl<'b> Refs<'b> {
    fn new(book: &'b Spreadsheet) -> Self {
        Self {
            book,
            names: HashMap::new(),
            depth: 0,
        }
    }

    /// The text an argument spells before anything is computed: a literal, or
    /// what a single cell holds - its cached result, if it is a formula. Only
    /// the order of a pass rests on it, so a stale cache costs work, never a
    /// wrong answer.
    fn text_of(&self, own_sheet: usize, arg: &Expr) -> Option<String> {
        match arg {
            Expr::Text(text) => Some(text.clone()),
            Expr::Range { sheet, range, .. } if range.start == range.end => {
                let index = match sheet {
                    None => own_sheet,
                    Some(name) => self
                        .book
                        .sheets()
                        .iter()
                        .position(|s| same_name(s.title(), name))?,
                };
                match &self.book.sheet(index)?.get(range.start)?.value {
                    CellValue::Text(text) => Some(text.to_string()),
                    CellValue::Formula { cached, .. } => match cached.as_deref()? {
                        CellValue::Text(text) => Some(text.to_string()),
                        _ => None,
                    },
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// The ranges a defined name stands for, read from the sheet `sheet`, and
    /// whether what it stands for is built at evaluation time.
    fn name(&mut self, sheet: usize, name: &str) -> NameReads {
        // A name that names a name is followed a few steps, not forever.
        const MAX_NAME_DEPTH: usize = 8;
        let key = (sheet, name.to_lowercase());
        if let Some(found) = self.names.get(&key) {
            return found.clone();
        }
        if self.depth >= MAX_NAME_DEPTH {
            return (Vec::new(), true);
        }
        let defined = self
            .book
            .defined_names
            .iter()
            .filter(|n| n.name.eq_ignore_ascii_case(name) || n.name.to_lowercase() == key.1)
            .min_by_key(|n| n.sheet != Some(sheet));
        let mut out = Vec::new();
        let mut dynamic = defined.is_none();
        if let Some(expr) = defined.and_then(|n| parse(&n.formula).ok()) {
            self.depth += 1;
            collect_refs(&expr, sheet, self, &mut out, &mut dynamic);
            self.depth -= 1;
        }
        self.names.insert(key, (out.clone(), dynamic));
        (out, dynamic)
    }
}

/// Splits `[1]Sheet1` into the workbook's index in [`Spreadsheet::external`]
/// and the sheet name inside it.
fn external_ref(sheet: &str) -> Option<(usize, &str)> {
    let (index, name) = sheet.strip_prefix('[')?.split_once(']')?;
    Some((index.parse::<usize>().ok()?.checked_sub(1)?, name))
}
