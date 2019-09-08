//! The Excel function library.
//!
//!
//! A function is written in one of two shapes:
//!
//! * **eager** — `fn(&[Arg]) -> Value`, for the great majority. Every argument
//!   is computed before the call.
//! * **dated** — `fn(Epoch, &[Arg]) -> Value`, the same but told which base
//!   date the workbook counts from. A serial number means a different day in a
//!   1904 workbook, so anything reading or building one needs to know.
//! * **lazy** — `fn(&mut Engine, Origin, &[Expr]) -> Value`, for the few that
//!   must not compute all of them (`IF` runs one branch) or that need the
//!   reference rather than its value (`ROW`, `COLUMN`).

// Every function in here is reached through [`call`] by pointer, never by a
// caller who could forget the result, and there are hundreds of them.
#![allow(
    clippy::must_use_candidate,
    reason = "a `#[must_use]` on each of hundreds of table entries says nothing"
)]

// pub mod database;
pub mod date;
// pub mod distributions;
// pub mod engineering;
// pub mod ets;
// pub mod financial;
pub mod info;
// pub mod lambda;
pub mod logical;
// pub mod lookup;
pub mod math;
// pub mod regression;
// pub mod securities;
// pub mod stats;
pub mod text;
// pub mod web;

use crate::error::CellError;
use crate::formula::eval::{Engine, Origin};
use crate::formula::parser::{BinaryOp, Expr};
use crate::formula::value::Value;
use crate::shared::date::Epoch;

/// A function whose arguments are all computed before it runs.
type Eager = fn(&[Arg]) -> Value;

/// The same, plus the workbook's base date.
type Dated = fn(Epoch, &[Arg]) -> Value;

/// A function that decides for itself what to compute.
type Lazy = fn(&mut Engine<'_>, Origin, &[Expr]) -> Value;

/// One computed argument.
#[derive(Debug, Clone)]
pub struct Arg {
    /// What the argument evaluated to.
    pub value: Value,
    /// Whether it came from a reference rather than being written out.
    ///
    /// Excel's aggregates treat the two differently: `SUM(TRUE)` is 1, but
    /// `SUM(A1)` with `TRUE` in A1 is 0. Text and booleans are converted when
    /// they are written into the formula and skipped when they are read out of
    /// a cell.
    pub reference: bool,
}

impl Arg {
    /// The argument as a number, with Excel's conversions.
    ///
    /// # Errors
    /// [`CellError::Value`] for text that is not a number.
    pub fn number(&self) -> Result<f64, CellError> {
        self.value.number()
    }

    /// The argument as text.
    ///
    /// # Errors
    /// Whatever error the argument carried.
    pub fn text(&self) -> Result<String, CellError> {
        self.value.text()
    }

    /// The argument as a serial date, reading a date out of text if need be.
    ///
    /// This is the path only the date functions take. `DAY("31-May-2015")` is
    /// 31 because `DAY` asks for a date and text may spell one; `SUM` asks for
    /// a number, and text that merely looks like a date is not one to it.
    ///
    /// # Errors
    /// [`CellError::Value`] for text that spells no date, and whatever error
    /// the argument already carried.
    pub fn serial(&self, epoch: Epoch) -> Result<f64, CellError> {
        let Value::Text(text) = self.value.scalar() else {
            return self.value.scalar().number();
        };
        // Text spelling a number is that serial already: `DAY("35")` is the
        // fourth of February 1900, not a date read out of the digits.
        if let Ok(n) = self.value.scalar().number() {
            return Ok(n);
        }
        crate::shared::date_parse::parse(text)
            .and_then(|parsed| parsed.serial(epoch))
            .ok_or(CellError::Value)
    }

    /// Whether the argument was left out, as the second one in `IF(A1,,0)`.
    #[must_use]
    pub const fn missing(&self) -> bool {
        matches!(self.value, Value::Blank) && !self.reference
    }
}

/// Calls a function by name.
///
/// Names are matched upper-cased, as the parser hands them over.
pub fn call(engine: &mut Engine<'_>, origin: Origin, name: &str, args: &[Expr]) -> Value {
    if let Some(f) = lazy(name) {
        return f(engine, origin, args);
    }
    let eager = eager(name);
    let dated = dated(name);
    // A function the caller registered is tried only where no built-in claims
    // the name, and it takes its arguments already computed.
    let custom = if eager.is_none() && dated.is_none() {
        match engine.custom().and_then(|set| set.get(name)) {
            Some(f) => Some(f),
            None => return Value::Error(CellError::Name),
        }
    } else {
        None
    };
    let epoch = engine.book().epoch;
    let args: Vec<Arg> = args
        .iter()
        .map(|e| Arg {
            value: engine.eval_expr(origin, e),
            reference: is_reference(e),
        })
        .collect();
    if let Some(f) = custom {
        let values: Vec<Value> = args.into_iter().map(|a| a.value).collect();
        return f(&values);
    }
    match (eager, dated) {
        (Some(f), _) => f(&args),
        (_, Some(f)) => f(epoch, &args),
        _ => Value::Error(CellError::Name),
    }
}

/// Whether an expression names cells rather than computing a value.
///
/// The three reference operators build references out of references, so
/// `SUM(A1:A2:D1)` reads its cells the way `SUM(A1:D2)` does — skipping the
/// text and the booleans among them rather than converting them.
fn is_reference(e: &Expr) -> bool {
    match e {
        Expr::Range { .. } => true,
        Expr::Binary(BinaryOp::Span | BinaryOp::Intersect | BinaryOp::Union, a, b) => {
            is_reference(a) && is_reference(b)
        }
        _ => false,
    }
}

/// Whether a function of this name exists yet.
#[must_use]
pub fn is_known(name: &str) -> bool {
    lazy(name).is_some() || eager(name).is_some() || dated(name).is_some()
}

/// Functions that choose what to evaluate for themselves.
fn lazy(name: &str) -> Option<Lazy> {
    Some(match name {
//         "BYCOL" => lambda::bycol,
//         "BYROW" => lambda::byrow,
        "IF" => logical::if_,
//         "LAMBDA" => lambda::lambda,
//         "LET" => lambda::let_,
//         "MAKEARRAY" => lambda::makearray,
//         "MAP" => lambda::map,
//         "REDUCE" => lambda::reduce,
//         "SCAN" => lambda::scan,
        "IFERROR" => logical::iferror,
        "IFNA" => logical::ifna,
        "IFS" => logical::ifs,
        "SWITCH" => logical::switch,
        "SUBTOTAL" => math::subtotal,
        "AGGREGATE" => math::aggregate,
//         "AREAS" => lookup::areas,
        "ISREF" => info::isref,
        "ISOMITTED" => info::isomitted,
        "ISFORMULA" => info::isformula,
        "FORMULATEXT" => info::formulatext,
        "SHEET" => info::sheet,
        "SHEETS" => info::sheets,
        "CELL" => info::cell_info,
//         "ANCHORARRAY" => lookup::anchorarray,
//         "INDIRECT" => lookup::indirect,
//         "SINGLE" => lookup::single,
//         "OFFSET" => lookup::offset,
//         "ROW" => lookup::row,
//         "COLUMN" => lookup::column,
//         "ROWS" => lookup::rows,
//         "COLUMNS" => lookup::columns,
        "TEXT" => text::text_format,
        "VALUE" => text::value,
        _ => return None,
    })
}

/// Functions that work in serial dates, and so need the workbook's epoch.
fn dated(name: &str) -> Option<Dated> {
    Some(match name {
//         "AMORDEGRC" => financial::amordegrc,
//         "COUPDAYBS" => securities::coupdaybs,
//         "COUPDAYS" => securities::coupdays,
//         "COUPDAYSNC" => securities::coupdaysnc,
//         "COUPNCD" => securities::coupncd,
//         "COUPNUM" => securities::coupnum,
//         "COUPPCD" => securities::couppcd,
//         "ACCRINT" => securities::accrint,
//         "ACCRINTM" => securities::accrintm,
//         "DISC" => securities::disc,
//         "INTRATE" => securities::intrate,
//         "ODDFPRICE" => securities::oddfprice,
//         "ODDFYIELD" => securities::oddfyield,
//         "ODDLPRICE" => securities::oddlprice,
//         "ODDLYIELD" => securities::oddlyield,
//         "PRICE" => securities::price,
//         "PRICEDISC" => securities::pricedisc,
//         "PRICEMAT" => securities::pricemat,
//         "RECEIVED" => securities::received,
//         "TBILLEQ" => securities::tbilleq,
//         "TBILLPRICE" => securities::tbillprice,
//         "TBILLYIELD" => securities::tbillyield,
//         "DURATION" => securities::duration,
//         "MDURATION" => securities::mduration,
//         "YIELD" => securities::yield_,
//         "YIELDDISC" => securities::yielddisc,
//         "YIELDMAT" => securities::yieldmat,
//         "AMORLINC" => financial::amorlinc,
        "DATE" => date::date,
        "DATEVALUE" => date::datevalue,
        "TIMEVALUE" => date::timevalue,
        "DATEDIF" => date::datedif,
        "DAY" => date::day,
        "DAYS" => date::days,
        "DAYS360" => date::days360,
        "EDATE" => date::edate,
        "EOMONTH" => date::eomonth,
        "HOUR" => date::hour,
        "ISOWEEKNUM" => date::isoweeknum,
        "MINUTE" => date::minute,
        "MONTH" => date::month,
        "NETWORKDAYS" => date::networkdays,
        "NETWORKDAYS.INTL" => date::networkdays_intl,
        "NOW" => date::now,
        "SECOND" => date::second,
        "TIME" => date::time,
        "TODAY" => date::today,
        "WEEKDAY" => date::weekday,
        "WEEKNUM" => date::weeknum,
        "WORKDAY" => date::workday,
        "WORKDAY.INTL" => date::workday_intl,
        "YEAR" => date::year,
        "YEARFRAC" => date::yearfrac,
        _ => return None,
    })
}

/// Functions whose arguments are all computed first.
///
/// One table of names, split only where clippy insists; the parts run in the
/// order the categories are listed and each falls through to the next.
fn eager(name: &str) -> Option<Eager> {
    // Ordered by how often each table hits: math first for a reason.
    const TABLES: [fn(&str) -> Option<Eager>; 7] = [
        eager_math,
        eager_finance,
        eager_stats,
        eager_engineering,
        eager_distributions,
        eager_logic_text,
        eager_lookup,
    ];
    TABLES.iter().find_map(|table| table(name))
}

/// Math, trigonometry and the rounding family.
fn eager_math(name: &str) -> Option<Eager> {
    Some(match name {
        // Math and trigonometry.
        "ABS" => math::abs,
        // `.ODS` is the same function under the name OpenDocument gave it.
        "CEILING" | "CEILING.ODS" => math::ceiling,
        "EXP" => math::exp,
        "FLOOR" | "FLOOR.ODS" => math::floor,
        "INT" => math::int,
        "LN" => math::ln,
        "LOG" => math::log,
        "LOG10" => math::log10,
        "MOD" => math::mod_,
        "PI" => math::pi,
        "POWER" => math::power,
        "PRODUCT" => math::product,
        "ROUND" => math::round,
        "ROUNDDOWN" => math::rounddown,
        "ROUNDUP" => math::roundup,
        "SIGN" => math::sign,
        "SQRT" => math::sqrt,
        "SUM" => math::sum,
//         "SUMIF" => stats::sumif,
//         "SUMIFS" => stats::sumifs,
        "SUMPRODUCT" => math::sumproduct,
        "TRUNC" => math::trunc,
        // Trigonometry, in radians throughout.
        "SIN" => math::sin,
        "COS" => math::cos,
        "TAN" => math::tan,
        "COT" => math::cot,
        "SEC" => math::sec,
        "CSC" => math::csc,
        "ASIN" => math::asin,
        "ACOS" => math::acos,
        "ATAN" => math::atan,
        "ACOT" => math::acot,
        "ATAN2" => math::atan2,
        "SINH" => math::sinh,
        "COSH" => math::cosh,
        "TANH" => math::tanh,
        "COTH" => math::coth,
        "SECH" => math::sech,
        "CSCH" => math::csch,
        "ASINH" => math::asinh,
        "ACOSH" => math::acosh,
        "ATANH" => math::atanh,
        "ACOTH" => math::acoth,
        "DEGREES" => math::degrees,
        "RADIANS" => math::radians,
        // Rounding to a step, and the whole-number functions.
        "EVEN" => math::even,
        "ODD" => math::odd,
        "MROUND" => math::mround,
        "QUOTIENT" => math::quotient,
        "GCD" => math::gcd,
        "LCM" => math::lcm,
        // Combinatorics.
        "FACT" => math::fact,
        "FACTDOUBLE" => math::factdouble,
        "COMBIN" => math::combin,
        "COMBINA" => math::combina,
        "PERMUT" => math::permut,
        "MULTINOMIAL" => math::multinomial,
        // Sums over pairs and series.
        "SUMSQ" => math::sumsq,
        "SUMX2MY2" => math::sumx2my2,
        "SUMX2PY2" => math::sumx2py2,
        "SUMXMY2" => math::sumxmy2,
        "SERIESSUM" => math::seriessum,
        "SQRTPI" => math::sqrtpi,
        // Matrices, sequences and the rest of the rounding family.
        "MUNIT" => math::munit,
        "MMULT" => math::mmult,
        "MDETERM" => math::mdeterm,
        "MINVERSE" => math::minverse,
        "SEQUENCE" => math::sequence,
        "RAND" => math::rand,
        "RANDBETWEEN" => math::randbetween,
        "RANDARRAY" => math::randarray,
//         "WRAPROWS" => lookup::wraprows,
//         "WRAPCOLS" => lookup::wrapcols,
//         "EXPAND" => lookup::expand,
        "ROMAN" => math::roman,
        "ARABIC" => math::arabic,
        "BASE" => math::base,
        "DECIMAL" => math::decimal,
        "CEILING.MATH" | "CEILING.XCL" => math::ceiling_math,
        "FLOOR.MATH" | "FLOOR.XCL" => math::floor_math,
        "CEILING.PRECISE" | "ISO.CEILING" | "ECMA.CEILING" => math::ceiling_precise,
        "FLOOR.PRECISE" => math::floor_precise,
        _ => return None,
    })
}

/// Finance.
fn eager_finance(name: &str) -> Option<Eager> {
    Some(match name {
        // Finance.
//         "FV" => financial::fv,
//         "PV" => financial::pv,
//         "PMT" => financial::pmt,
//         "NPER" => financial::nper,
//         "RATE" => financial::rate,
//         "IPMT" => financial::ipmt,
//         "PPMT" => financial::ppmt,
//         "CUMIPMT" => financial::cumipmt,
//         "CUMPRINC" => financial::cumprinc,
//         "ISPMT" => financial::ispmt,
//         "NPV" => financial::npv,
//         "IRR" => financial::irr,
//         "MIRR" => financial::mirr,
//         "XNPV" => financial::xnpv,
//         "XIRR" => financial::xirr,
//         "FVSCHEDULE" => financial::fvschedule,
//         "SLN" => financial::sln,
//         "SYD" => financial::syd,
//         "DB" => financial::db,
//         "DDB" => financial::ddb,
//         "VDB" => financial::vdb,
//         "EFFECT" => financial::effect,
//         "NOMINAL" => financial::nominal,
//         "RRI" => financial::rri,
//         "PDURATION" => financial::pduration,
//         "DOLLARDE" => financial::dollarde,
//         "DOLLARFR" => financial::dollarfr,
        _ => return None,
    })
}
/// Statistics: the descriptive kind, quantiles, ranks and correlation.
fn eager_stats(name: &str) -> Option<Eager> {
    Some(match name {
        // Statistics.
//         "AVERAGE" => stats::average,
//         "AVERAGEIF" => stats::averageif,
//         "AVERAGEIFS" => stats::averageifs,
//         "COUNT" => stats::count,
//         "COUNTA" => stats::counta,
//         "COUNTIF" => stats::countif,
//         "COUNTIFS" => stats::countifs,
//         "COUNTBLANK" => stats::countblank,
//         "MAX" => stats::max,
//         "LARGE" => stats::large,
//         "MEDIAN" => stats::median,
//         "MEDIANIF" => stats::medianif,
//         "RANK" | "RANK.EQ" => stats::rank,
//         "SMALL" => stats::small,
//         "STDEV" | "STDEV.S" => stats::stdev,
//         "STDEVP" | "STDEV.P" => stats::stdevp,
//         "VAR" | "VAR.S" => stats::var,
//         "VARP" | "VAR.P" => stats::varp,
        // Descriptive statistics.
//         "AVEDEV" => stats::avedev,
//         "DEVSQ" => stats::devsq,
//         "GEOMEAN" => stats::geomean,
//         "HARMEAN" => stats::harmean,
//         "SKEW" => stats::skew,
//         "SKEW.P" => stats::skewp,
//         "KURT" => stats::kurt,
//         "MODE" | "MODE.SNGL" => stats::mode,
//         "TRIMMEAN" => stats::trimmean,
//         "STANDARDIZE" => stats::standardize,
//         "PERMUTATIONA" => stats::permutationa,
        // Quantiles and ranks.
//         "PERCENTILE" | "PERCENTILE.INC" => stats::percentile,
//         "PERCENTILE.EXC" => stats::percentile_exc,
//         "QUARTILE" | "QUARTILE.INC" => stats::quartile,
//         "QUARTILE.EXC" => stats::quartile_exc,
//         "PERCENTRANK" | "PERCENTRANK.INC" => stats::percentrank,
//         "PERCENTRANK.EXC" => stats::percentrank_exc,
        // Correlation and the line of best fit.
//         "CORREL" | "PEARSON" => stats::correl,
//         "RSQ" => stats::rsq,
//         "COVAR" | "COVARIANCE.P" => stats::covar,
//         "COVARIANCE.S" => stats::covariance_s,
//         "SLOPE" => stats::slope,
//         "INTERCEPT" => stats::intercept,
//         "STEYX" => stats::steyx,
//         "FORECAST" | "FORECAST.LINEAR" => stats::forecast,
//         "FORECAST.ETS" => ets::forecast_ets,
//         "FORECAST.ETS.CONFINT" => ets::forecast_ets_confint,
//         "FORECAST.ETS.SEASONALITY" => ets::forecast_ets_seasonality,
//         "FORECAST.ETS.STAT" => ets::forecast_ets_stat,
//         "FISHER" => stats::fisher,
//         "FISHERINV" => stats::fisherinv,
//         "PHI" => stats::phi,
        // The `A` forms, which count text as zero.
//         "AVERAGEA" => stats::averagea,
//         "MAXA" => stats::maxa,
//         "MINA" => stats::mina,
//         "STDEVA" => stats::stdeva,
//         "STDEVPA" => stats::stdevpa,
//         "VARA" => stats::vara,
//         "VARPA" => stats::varpa,
//         "MAXIFS" => stats::maxifs,
//         "MINIFS" => stats::minifs,
//         "RANK.AVG" => stats::rank_avg,
//         "MODE.MULT" => stats::mode_mult,
//         "FREQUENCY" => stats::frequency,
//         "PROB" => stats::prob,
        _ => return None,
    })
}

/// Databases and engineering: number bases, bits and complex numbers.
fn eager_engineering(name: &str) -> Option<Eager> {
    Some(match name {
        // Databases.
//         "DSUM" => database::dsum,
//         "DPRODUCT" => database::dproduct,
//         "DAVERAGE" => database::daverage,
//         "DMAX" => database::dmax,
//         "DMIN" => database::dmin,
//         "DCOUNT" => database::dcount,
//         "DCOUNTA" => database::dcounta,
//         "DSTDEV" => database::dstdev,
//         "DSTDEVP" => database::dstdevp,
//         "DVAR" => database::dvar,
//         "DVARP" => database::dvarp,
//         "DGET" => database::dget,
        // Engineering: number bases, bits, steps and the error function.
//         "BIN2DEC" => engineering::bin2dec,
//         "OCT2DEC" => engineering::oct2dec,
//         "HEX2DEC" => engineering::hex2dec,
//         "DEC2BIN" => engineering::dec2bin,
//         "DEC2OCT" => engineering::dec2oct,
//         "DEC2HEX" => engineering::dec2hex,
//         "BIN2OCT" => engineering::bin2oct,
//         "BIN2HEX" => engineering::bin2hex,
//         "OCT2BIN" => engineering::oct2bin,
//         "OCT2HEX" => engineering::oct2hex,
//         "HEX2BIN" => engineering::hex2bin,
//         "HEX2OCT" => engineering::hex2oct,
//         "BITAND" => engineering::bitand,
//         "BITOR" => engineering::bitor,
//         "BITXOR" => engineering::bitxor,
//         "BITLSHIFT" => engineering::bitlshift,
//         "BITRSHIFT" => engineering::bitrshift,
//         "DELTA" => engineering::delta,
//         "GESTEP" => engineering::gestep,
//         "BESSELJ" => engineering::besselj,
//         "BESSELI" => engineering::besseli,
//         "BESSELK" => engineering::besselk,
//         "BESSELY" => engineering::bessely,
//         "CONVERT" => engineering::convert,
//         "ERF" | "ERF.PRECISE" => engineering::erf_fn,
//         "ERFC" | "ERFC.PRECISE" => engineering::erfc_fn,
        // Complex numbers, written as text.
//         "COMPLEX" => engineering::complex,
//         "IMREAL" => engineering::imreal,
//         "IMAGINARY" => engineering::imaginary,
//         "IMABS" => engineering::imabs,
//         "IMARGUMENT" => engineering::imargument,
//         "IMCONJUGATE" => engineering::imconjugate,
//         "IMSUM" => engineering::imsum,
//         "IMSUB" => engineering::imsub,
//         "IMPRODUCT" => engineering::improduct,
//         "IMDIV" => engineering::imdiv,
//         "IMEXP" => engineering::imexp,
//         "IMLN" => engineering::imln,
//         "IMLOG10" => engineering::imlog10,
//         "IMLOG2" => engineering::imlog2,
//         "IMPOWER" => engineering::impower,
//         "IMSQRT" => engineering::imsqrt,
//         "IMSIN" => engineering::imsin,
//         "IMCOS" => engineering::imcos,
//         "IMSINH" => engineering::imsinh,
//         "IMCOSH" => engineering::imcosh,
//         "IMTAN" => engineering::imtan,
//         "IMCOT" => engineering::imcot,
//         "IMSEC" => engineering::imsec,
//         "IMCSC" => engineering::imcsc,
//         "IMSECH" => engineering::imsech,
//         "IMCSCH" => engineering::imcsch,
        _ => return None,
    })
}

/// Least squares, tests of a hypothesis and the distributions.
fn eager_distributions(name: &str) -> Option<Eager> {
    Some(match name {
        // Least squares.
//         "LINEST" => regression::linest,
//         "LOGEST" => regression::logest,
//         "TREND" => regression::trend,
//         "GROWTH" => regression::growth,
        // Tests of a hypothesis.
//         "CHISQ.TEST" | "CHITEST" => distributions::chisq_test,
//         "F.TEST" | "FTEST" => distributions::f_test,
//         "T.TEST" | "TTEST" => distributions::t_test,
//         "Z.TEST" | "ZTEST" => distributions::z_test,
        // Distributions.
//         "NORM.DIST" | "NORMDIST" => distributions::norm_dist,
//         "NORM.S.DIST" | "NORMSDIST" => distributions::norm_s_dist,
//         "NORM.INV" | "NORMINV" => distributions::norm_inv,
//         "NORM.S.INV" | "NORMSINV" => distributions::norm_s_inv,
//         "GAUSS" => distributions::gauss,
//         "CONFIDENCE" | "CONFIDENCE.NORM" => distributions::confidence,
//         "CONFIDENCE.T" => distributions::confidence_t,
//         "BINOM.DIST.RANGE" => distributions::binom_dist_range,
//         "LOGNORM.DIST" | "LOGNORMDIST" => distributions::lognorm_dist,
//         "LOGNORM.INV" | "LOGINV" => distributions::lognorm_inv,
//         "GAMMA" => distributions::gamma_fn,
//         "GAMMALN" | "GAMMALN.PRECISE" => distributions::gammaln,
//         "GAMMA.DIST" | "GAMMADIST" => distributions::gamma_dist,
//         "GAMMA.INV" | "GAMMAINV" => distributions::gamma_inv,
//         "CHISQ.DIST" => distributions::chisq_dist,
//         "CHISQ.DIST.RT" | "CHIDIST" => distributions::chisq_dist_rt,
//         "CHISQ.INV" => distributions::chisq_inv,
//         "CHISQ.INV.RT" | "CHIINV" => distributions::chisq_inv_rt,
//         "T.DIST" => distributions::t_dist,
//         "T.DIST.RT" => distributions::t_dist_rt,
//         "T.DIST.2T" => distributions::t_dist_2t,
//         "TDIST" => distributions::tdist,
//         "T.INV" => distributions::t_inv,
//         "T.INV.2T" | "TINV" => distributions::t_inv_2t,
//         "F.DIST" => distributions::f_dist,
//         "F.DIST.RT" | "FDIST" => distributions::f_dist_rt,
//         "F.INV" => distributions::f_inv,
//         "F.INV.RT" | "FINV" => distributions::f_inv_rt,
//         "BETA.DIST" => distributions::beta_dist,
//         "BETADIST" => distributions::betadist,
//         "BETA.INV" | "BETAINV" => distributions::beta_inv,
//         "EXPON.DIST" | "EXPONDIST" => distributions::expon_dist,
//         "WEIBULL.DIST" | "WEIBULL" => distributions::weibull_dist,
//         "POISSON.DIST" | "POISSON" => distributions::poisson_dist,
//         "BINOM.DIST" | "BINOMDIST" => distributions::binom_dist,
//         "NEGBINOM.DIST" | "NEGBINOMDIST" => distributions::negbinom_dist,
//         "HYPGEOM.DIST" | "HYPGEOMDIST" => distributions::hypgeom_dist,
//         "BINOM.INV" | "CRITBINOM" => distributions::binom_inv,
//         "MIN" => stats::min,
        _ => return None,
    })
}

/// Logic, text and information.
fn eager_logic_text(name: &str) -> Option<Eager> {
    Some(match name {
        // Logic.
        "AND" => logical::and,
        "FALSE" => logical::false_,
        "NOT" => logical::not,
        "OR" => logical::or,
        "TRUE" => logical::true_,
        "XOR" => logical::xor,
        // Text.
        "CONCAT" | "CONCATENATE" => text::concatenate,
        "EXACT" => text::exact,
        "ASC" => text::asc,
        "BAHTTEXT" => text::bahttext,
        "ISTHAIDIGIT" => text::isthaidigit,
        "THAIDIGIT" => text::thaidigit,
        "ROUNDBAHTUP" => text::roundbahtup,
        "ROUNDBAHTDOWN" => text::roundbahtdown,
        "DBCS" | "JIS" => text::dbcs,
        "FIND" | "FINDB" => text::find,
        "LEFT" | "LEFTB" => text::left,
        "LEN" | "LENB" => text::len,
        "LOWER" => text::lower,
        "MID" | "MIDB" => text::mid,
        "REPT" => text::rept,
        "RIGHT" | "RIGHTB" => text::right,
        "SEARCH" | "SEARCHB" => text::search,
        "SUBSTITUTE" => text::substitute,
        "TRIM" => text::trim,
        "UPPER" => text::upper,
        "DOLLAR" => text::dollar,
        "TEXTSPLIT" => text::textsplit,
        "ARRAYTOTEXT" => text::arraytotext,
        "VALUETOTEXT" => text::valuetotext,
//         "ENCODEURL" => web::encodeurl,
        "PROPER" => text::proper,
        "CLEAN" => text::clean,
        "T" => text::t,
        "CHAR" => text::char_,
        "UNICHAR" => text::unichar,
        "CODE" => text::code,
        "UNICODE" => text::unicode,
        "REPLACE" | "REPLACEB" => text::replace,
        "TEXTJOIN" => text::textjoin,
        "TEXTBEFORE" => text::textbefore,
        "TEXTAFTER" => text::textafter,
        "NUMBERVALUE" => text::numbervalue,
        "FIXED" => text::fixed,
        // Information.
        "ISBLANK" => info::isblank,
        "ISERR" => info::iserr,
        "ISERROR" => info::iserror,
        "ISLOGICAL" => info::islogical,
        "ISNA" => info::isna,
        "ISNONTEXT" => info::isnontext,
        "ISNUMBER" => info::isnumber,
        "ISTEXT" => info::istext,
        "N" => info::n,
        "NA" => info::na,
        "TYPE" => info::type_,
        _ => return None,
    })
}

/// Lookup, references and the arrays built from them.
fn eager_lookup(name: &str) -> Option<Eager> {
    Some(match name {
        // Lookup and reference.
//         "CHOOSE" => lookup::choose,
        // Arrays and references.
//         "TRANSPOSE" => lookup::transpose,
//         "ADDRESS" => lookup::address,
//         "SORT" => lookup::sort,
//         "SORTBY" => lookup::sortby,
//         "UNIQUE" => lookup::unique,
//         "FILTER" => lookup::filter,
//         "TAKE" => lookup::take,
//         "DROP" => lookup::drop,
//         "CHOOSEROWS" => lookup::chooserows,
//         "CHOOSECOLS" => lookup::choosecols,
//         "VSTACK" => lookup::vstack,
//         "HSTACK" => lookup::hstack,
//         "TOROW" => lookup::torow,
//         "TOCOL" => lookup::tocol,
//         "HYPERLINK" => lookup::hyperlink,
//         "XLOOKUP" => lookup::xlookup,
//         "XMATCH" => lookup::xmatch,
//         "LOOKUP" => lookup::lookup_vector,
        // Information.
        "ISEVEN" => info::iseven,
        "ISODD" => info::isodd,
        "ERROR.TYPE" => info::error_type,
//         "HLOOKUP" => lookup::hlookup,
//         "INDEX" => lookup::index,
//         "MATCH" => lookup::match_,
//         "VLOOKUP" => lookup::vlookup,
        _ => return None,
    })
}

/// The first error among the arguments, so a function can bail out before it
/// does any work.
pub(crate) fn first_error(args: &[Arg]) -> Option<CellError> {
    let mut flat = Vec::new();
    for a in args {
        a.value.flatten(&mut flat);
    }
    flat.iter().find_map(|v| v.error())
}

/// The numbers an aggregate should work on.
///
/// A value written into the formula is converted — `SUM("1",TRUE)` is 2 — while
/// one read out of a cell is skipped unless it is already a number.
pub(crate) fn aggregate_numbers(args: &[Arg]) -> Result<Vec<f64>, CellError> {
    let mut out = Vec::new();
    for arg in args {
        let mut flat = Vec::new();
        arg.value.flatten(&mut flat);
        for v in flat {
            match v {
                Value::Error(e) => return Err(*e),
                Value::Number(n) => out.push(*n),
                Value::Blank => {}
                Value::Bool(_) | Value::Text(_) if arg.reference => {}
                other => out.push(other.number()?),
            }
        }
    }
    Ok(out)
}

/// A test a value has to pass, as `COUNTIF` and its family spell one.
///
pub(crate) struct Criterion {
    order: [bool; 3],
    against: Value,
    pattern: Option<String>,
}

impl Criterion {
    /// Reads a criterion out of the value a function was given.
    pub(crate) fn parse(value: &Value) -> Self {
        let text = match value.scalar() {
            Value::Text(t) => t.clone(),
            other => {
                return Self {
                    // Anything but text is an equality test against itself.
                    order: [false, true, false],
                    against: other.clone(),
                    pattern: None,
                };
            }
        };
        // Which of less, equal and greater the criterion accepts.
        let (order, rest) = if let Some(rest) = text.strip_prefix("<>") {
            ([true, false, true], rest)
        } else if let Some(rest) = text.strip_prefix(">=") {
            ([false, true, true], rest)
        } else if let Some(rest) = text.strip_prefix("<=") {
            ([true, true, false], rest)
        } else if let Some(rest) = text.strip_prefix('>') {
            ([false, false, true], rest)
        } else if let Some(rest) = text.strip_prefix('<') {
            ([true, false, false], rest)
        } else if let Some(rest) = text.strip_prefix('=') {
            ([false, true, false], rest)
        } else {
            ([false, true, false], text.as_str())
        };
        // A criterion that says nothing after the operator tests for empty.
        let against = match rest.trim().parse::<f64>() {
            Ok(n) if !rest.trim().is_empty() => Value::Number(n),
            _ => Value::Text(rest.to_owned()),
        };
        // Wildcards only mean anything to a plain equality test.
        let pattern = matches!(against, Value::Text(_))
            .then(|| rest.to_owned())
            .filter(|p| {
                p.contains(['*', '?'])
                    && (order == [false, true, false] || order == [true, false, true])
            });
        Self {
            order,
            against,
            pattern,
        }
    }

    /// Whether a value passes the test.
    pub(crate) fn matches(&self, value: &Value) -> bool {
        if let Some(pattern) = &self.pattern {
            // Wildcards test text and nothing else: in Excel a number, a
            // boolean or an empty cell never matches `"*"`. Rendering
            // every value as a string first and so counts all three.
            let hit = match value.scalar() {
                Value::Text(text) => wildcard_match(&pattern.to_uppercase(), &text.to_uppercase()),
                _ => false,
            };
            // The only two orders a pattern is kept for are `=` and `<>`.
            return hit == self.order[1];
        }
        // A blank cell answers no to every test but "is it empty".
        if matches!(value, Value::Blank) {
            return matches!(&self.against, Value::Text(t) if t.is_empty()) && self.order[1];
        }
        // Numbers and text never compare against each other here: `">5"` does
        // not count a cell holding "abc".
        let comparable = match (value.scalar(), &self.against) {
            (Value::Number(_) | Value::Bool(_), Value::Number(_) | Value::Bool(_))
            | (Value::Text(_), Value::Text(_)) => true,
            // An empty criterion body after `<>` means "anything at all".
            (_, Value::Text(t)) if t.is_empty() => return self.order[0] && self.order[2],
            _ => false,
        };
        if !comparable {
            return false;
        }
        let ord = crate::formula::value::compare(value.scalar(), &self.against);
        self.order[match ord {
            std::cmp::Ordering::Less => 0,
            std::cmp::Ordering::Equal => 1,
            std::cmp::Ordering::Greater => 2,
        }]
    }
}

/// Matches text against a pattern holding `*` and `?`, with `~` escaping one.
pub(crate) fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    // The usual two-cursor walk with a remembered star, so it stays linear.
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut retry) = (None, 0);
    while ti < t.len() {
        let literal = match p.get(pi) {
            Some('~') => p.get(pi + 1).copied().map(|c| (c, 2)),
            Some('?') => {
                pi += 1;
                ti += 1;
                continue;
            }
            Some('*') => {
                star = Some(pi);
                pi += 1;
                retry = ti;
                continue;
            }
            Some(c) => Some((*c, 1)),
            None => None,
        };
        match literal {
            Some((c, width)) if c == t[ti] => {
                pi += width;
                ti += 1;
            }
            _ => match star {
                Some(at) => {
                    pi = at + 1;
                    retry += 1;
                    ti = retry;
                }
                None => return false,
            },
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// The first position in `text` at or after `from` where `pattern` starts to
/// match, counted in characters from one.
///
/// `SEARCH` takes wildcards where `FIND` does not, and it answers where the
/// match begins rather than whether the whole string matches. Each starting
/// point is tried in turn against the pattern with a `*` appended, which is
/// what makes the match a prefix one.
pub(crate) fn wildcard_position(pattern: &str, text: &str, from: usize) -> Option<usize> {
    let chars: Vec<char> = text.chars().collect();
    // A cell holds at most 32767 characters, so trying each start is fine;
    // an index built over the pattern would be faster and much longer.
    let anchored = format!("{pattern}*");
    for start in from..=chars.len() {
        let tail: String = chars[start..].iter().collect();
        if wildcard_match(&anchored, &tail) {
            return Some(start + 1);
        }
    }
    None
}

/// Flattens an argument into the list of values a criteria function walks.
pub(crate) fn cells(arg: &Arg) -> Vec<&Value> {
    let mut out = Vec::new();
    arg.value.flatten(&mut out);
    out
}

/// Which positions of a range pass every (range, criterion) pair.
///
/// The ranges must all be the same size, as Excel requires of `SUMIFS` and its
/// family; a mismatch is `#VALUE!`.
pub(crate) fn selected(pairs: &[&Arg], len: usize) -> Result<Vec<bool>, CellError> {
    let mut keep = vec![true; len];
    for [range, criterion] in pairs.as_chunks::<2>().0 {
        let values = cells(range);
        if values.len() != len {
            return Err(CellError::Value);
        }
        let test = Criterion::parse(&criterion.value);
        for (slot, value) in keep.iter_mut().zip(values) {
            *slot = *slot && test.matches(value);
        }
    }
    Ok(keep)
}

/// The error to answer with when reading the arguments failed.
///
/// A failed read is usually an argument that is already an error, and Excel
/// passes that one along rather than replacing it: `INDEX(A1:B2,#N/A,1)` is
/// `#N/A`, not `#VALUE!`.
pub(crate) fn read_error<T: Copy>(reads: &[Result<T, CellError>]) -> Value {
    Value::Error(
        reads
            .iter()
            .find_map(|r| r.err())
            .unwrap_or(CellError::Value),
    )
}

/// A single number argument, for the many one-argument functions.
pub(crate) fn one(args: &[Arg], f: impl Fn(f64) -> Value) -> Value {
    match args {
        [a] => match a.number() {
            Ok(n) => f(n),
            Err(e) => Value::Error(e),
        },
        _ => Value::Error(CellError::Value),
    }
}
