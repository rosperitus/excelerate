//! Maths and trigonometry.

use super::{Arg, aggregate_numbers, first_error, one};
use crate::error::CellError;
use crate::formula::eval::{Engine, Origin};
use crate::formula::parser::Expr;
use crate::formula::value::Value;

/// `SUM(number1, ...)`
pub fn sum(args: &[Arg]) -> Value {
    match aggregate_numbers(args) {
        // Folded from a positive zero on purpose: summing an empty iterator
        // starts at -0.0 in Rust, and Excel never shows a negative zero.
        Ok(ns) => Value::Number(ns.iter().fold(0.0, |acc, n| acc + n)),
        Err(e) => Value::Error(e),
    }
}

/// `PRODUCT(number1, ...)`
pub fn product(args: &[Arg]) -> Value {
    match aggregate_numbers(args) {
        // Excel answers 0 for a product of nothing, not 1.
        Ok(ns) if ns.is_empty() => Value::Number(0.0),
        Ok(ns) => Value::Number(ns.iter().product()),
        Err(e) => Value::Error(e),
    }
}

/// `ABS(number)`
pub fn abs(args: &[Arg]) -> Value {
    one(args, |n| Value::Number(n.abs()))
}

/// `SIGN(number)`
pub fn sign(args: &[Arg]) -> Value {
    one(args, |n| {
        Value::Number(if n > 0.0 {
            1.0
        } else if n < 0.0 {
            -1.0
        } else {
            0.0
        })
    })
}

/// `INT(number)` - towards minus infinity, so `INT(-1.5)` is -2.
pub fn int(args: &[Arg]) -> Value {
    one(args, |n| Value::Number(n.floor()))
}

/// `SQRT(number)`
pub fn sqrt(args: &[Arg]) -> Value {
    one(args, |n| {
        if n < 0.0 {
            Value::Error(CellError::Num)
        } else {
            Value::Number(n.sqrt())
        }
    })
}

/// `EXP(number)`
pub fn exp(args: &[Arg]) -> Value {
    one(args, |n| Value::Number(n.exp()))
}

/// `LN(number)`
pub fn ln(args: &[Arg]) -> Value {
    one(args, |n| {
        if n <= 0.0 {
            Value::Error(CellError::Num)
        } else {
            Value::Number(n.ln())
        }
    })
}

/// `LOG10(number)`
pub fn log10(args: &[Arg]) -> Value {
    one(args, |n| {
        if n <= 0.0 {
            Value::Error(CellError::Num)
        } else {
            Value::Number(n.log10())
        }
    })
}

/// `PI()`
pub fn pi(args: &[Arg]) -> Value {
    if args.is_empty() {
        Value::Number(std::f64::consts::PI)
    } else {
        Value::Error(CellError::Value)
    }
}

/// `LOG(number, [base])` - base 10 when the base is left out.
pub fn log(args: &[Arg]) -> Value {
    let (n, base) = match args {
        [n] => (n.number(), Ok(10.0)),
        [n, b] => (n.number(), b.number()),
        _ => return Value::Error(CellError::Value),
    };
    match (n, base) {
        (Ok(n), Ok(b)) if n > 0.0 && b > 0.0 && (b - 1.0).abs() > f64::EPSILON => {
            Value::Number(n.log(b))
        }
        (Err(e), _) | (_, Err(e)) => Value::Error(e),
        _ => Value::Error(CellError::Num),
    }
}

/// `POWER(number, power)`
pub fn power(args: &[Arg]) -> Value {
    let [x, y] = args else {
        return Value::Error(CellError::Value);
    };
    match (x.number(), y.number()) {
        (Ok(x), Ok(y)) => {
            let r = x.powf(y);
            if r.is_finite() {
                Value::Number(r)
            } else {
                Value::Error(CellError::Num)
            }
        }
        (Err(e), _) | (_, Err(e)) => Value::Error(e),
    }
}

/// `MOD(number, divisor)`
///
/// The result takes the sign of the divisor, so `MOD(-3,2)` is 1 - Rust's `%`
/// would answer -1.
pub fn mod_(args: &[Arg]) -> Value {
    let [x, y] = args else {
        return Value::Error(CellError::Value);
    };
    match (x.number(), y.number()) {
        (Err(e), _) | (_, Err(e)) => Value::Error(e),
        (Ok(n), Ok(d)) => {
            if d == 0.0 {
                Value::Error(CellError::Div0)
            } else {
                Value::Number(n - d * (n / d).floor())
            }
        }
    }
}

/// Rounds half away from zero, which is what Excel does and what Rust's
/// `round` does not for ties in binary.
fn round_to(n: f64, digits: f64) -> f64 {
    let factor = 10f64.powf(digits);
    let scaled = n * factor;
    if !scaled.is_finite() {
        return n;
    }
    let r = if scaled < 0.0 {
        -(-scaled).round()
    } else {
        scaled.round()
    };
    r / factor
}

/// `ROUND(number, digits)`
pub fn round(args: &[Arg]) -> Value {
    two_numbers(args, |n, d| Value::Number(round_to(n, d)))
}

/// `ROUNDUP(number, digits)` - away from zero.
pub fn roundup(args: &[Arg]) -> Value {
    two_numbers(args, |n, d| {
        let f = 10f64.powf(d);
        Value::Number((n * f).abs().ceil().copysign(n) / f)
    })
}

/// `ROUNDDOWN(number, digits)` - towards zero.
pub fn rounddown(args: &[Arg]) -> Value {
    two_numbers(args, |n, d| {
        let f = 10f64.powf(d);
        Value::Number((n * f).abs().floor().copysign(n) / f)
    })
}

/// `TRUNC(number, [digits])`
pub fn trunc(args: &[Arg]) -> Value {
    match args {
        [n] => match n.number() {
            Ok(n) => Value::Number(n.trunc()),
            Err(e) => Value::Error(e),
        },
        [_, _] => rounddown(args),
        _ => Value::Error(CellError::Value),
    }
}

/// `CEILING(number, significance)` - up to the next multiple.
pub fn ceiling(args: &[Arg]) -> Value {
    multiple_of(args, f64::ceil)
}

/// `FLOOR(number, significance)` - down to the previous multiple.
pub fn floor(args: &[Arg]) -> Value {
    multiple_of(args, f64::floor)
}

/// Shared body of `CEILING` and `FLOOR`.
///
/// Only one combination of signs is refused: a positive number with a negative
/// step. A negative number rounds to a positive step perfectly well -
/// `FLOOR(-2.05, 2)` is -4 - and two negatives work too.
fn multiple_of(args: &[Arg], round: fn(f64) -> f64) -> Value {
    two_numbers(args, |n, step| {
        if step == 0.0 {
            // Excel answers 0 rather than dividing by zero.
            return Value::Number(0.0);
        }
        if n > 0.0 && step < 0.0 {
            return Value::Error(CellError::Num);
        }
        Value::Number(round(n / step) * step)
    })
}

/// Applies a body to exactly two numeric arguments.
fn two_numbers(args: &[Arg], body: impl Fn(f64, f64) -> Value) -> Value {
    if let Some(err) = first_error(args) {
        return Value::Error(err);
    }
    let [first, second] = args else {
        return Value::Error(CellError::Value);
    };
    match (first.number(), second.number()) {
        (Ok(x), Ok(y)) => body(x, y),
        (Err(err), _) | (_, Err(err)) => Value::Error(err),
    }
}

/// `SUMPRODUCT(array1, ...)` - the arrays multiplied element by element and
/// added up. Anything that is not a number counts as zero, as in Excel.
pub fn sumproduct(args: &[Arg]) -> Value {
    let Some((first, rest)) = args.split_first() else {
        return Value::Error(CellError::Value);
    };
    let mut totals: Vec<f64> = numbers_of(first);
    for arg in rest {
        let next = numbers_of(arg);
        if next.len() != totals.len() {
            // Excel wants the arrays the same size and says so.
            return Value::Error(CellError::Value);
        }
        for (slot, n) in totals.iter_mut().zip(next) {
            *slot *= n;
        }
    }
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    Value::Number(totals.iter().fold(0.0, |acc, n| acc + n))
}

/// Every element of an argument as a number, with non-numbers as zero.
fn numbers_of(arg: &Arg) -> Vec<f64> {
    super::cells(arg)
        .into_iter()
        .map(|v| match v {
            Value::Number(n) => *n,
            Value::Bool(b) => f64::from(u8::from(*b)),
            _ => 0.0,
        })
        .collect()
}

/// `SIN(number)` - the angle is in radians, as everywhere in this category.
pub fn sin(args: &[Arg]) -> Value {
    one(args, |n| Value::Number(n.sin()))
}

/// `COS(number)`
pub fn cos(args: &[Arg]) -> Value {
    one(args, |n| Value::Number(n.cos()))
}

/// `TAN(number)`
pub fn tan(args: &[Arg]) -> Value {
    one(args, |n| Value::Number(n.tan()))
}

/// `COT(number)` - the reciprocal of the tangent, undefined at zero.
pub fn cot(args: &[Arg]) -> Value {
    one(args, |n| {
        if n == 0.0 {
            Value::Error(CellError::Div0)
        } else {
            Value::Number(1.0 / n.tan())
        }
    })
}

/// `SEC(number)`
pub fn sec(args: &[Arg]) -> Value {
    one(args, |n| Value::Number(1.0 / n.cos()))
}

/// `CSC(number)`
pub fn csc(args: &[Arg]) -> Value {
    one(args, |n| {
        if n == 0.0 {
            Value::Error(CellError::Div0)
        } else {
            Value::Number(1.0 / n.sin())
        }
    })
}

/// `ASIN(number)` - outside `-1..=1` there is no angle.
pub fn asin(args: &[Arg]) -> Value {
    one(args, |n| bounded(n, f64::asin))
}

/// `ACOS(number)`
pub fn acos(args: &[Arg]) -> Value {
    one(args, |n| bounded(n, f64::acos))
}

/// `ATAN(number)`
pub fn atan(args: &[Arg]) -> Value {
    one(args, |n| Value::Number(n.atan()))
}

/// `ACOT(number)` - Excel's range is `0..pi`, not the `-pi/2..pi/2` that
/// `atan` of the reciprocal would give for a negative argument.
pub fn acot(args: &[Arg]) -> Value {
    one(args, |n| {
        Value::Number(core::f64::consts::FRAC_PI_2 - n.atan())
    })
}

/// `ATAN2(x, y)` - Excel takes x first, the reverse of every library's `atan2`.
pub fn atan2(args: &[Arg]) -> Value {
    two_numbers(args, |x, y| {
        if x == 0.0 && y == 0.0 {
            return Value::Error(CellError::Div0);
        }
        Value::Number(y.atan2(x))
    })
}

/// `SINH(number)`
pub fn sinh(args: &[Arg]) -> Value {
    one(args, |n| Value::Number(n.sinh()))
}

/// `COSH(number)`
pub fn cosh(args: &[Arg]) -> Value {
    one(args, |n| Value::Number(n.cosh()))
}

/// `TANH(number)`
pub fn tanh(args: &[Arg]) -> Value {
    one(args, |n| Value::Number(n.tanh()))
}

/// `COTH(number)`
pub fn coth(args: &[Arg]) -> Value {
    one(args, |n| {
        if n == 0.0 {
            Value::Error(CellError::Div0)
        } else {
            Value::Number(1.0 / n.tanh())
        }
    })
}

/// `SECH(number)`
pub fn sech(args: &[Arg]) -> Value {
    one(args, |n| Value::Number(1.0 / n.cosh()))
}

/// `CSCH(number)`
pub fn csch(args: &[Arg]) -> Value {
    one(args, |n| {
        if n == 0.0 {
            Value::Error(CellError::Div0)
        } else {
            Value::Number(1.0 / n.sinh())
        }
    })
}

/// `ASINH(number)`
pub fn asinh(args: &[Arg]) -> Value {
    one(args, |n| Value::Number(n.asinh()))
}

/// `ACOSH(number)` - defined from 1 upwards.
pub fn acosh(args: &[Arg]) -> Value {
    one(args, |n| {
        if n < 1.0 {
            Value::Error(CellError::Num)
        } else {
            Value::Number(n.acosh())
        }
    })
}

/// `ATANH(number)` - defined strictly inside `-1..1`.
pub fn atanh(args: &[Arg]) -> Value {
    one(args, |n| {
        if n <= -1.0 || n >= 1.0 {
            Value::Error(CellError::Num)
        } else {
            Value::Number(n.atanh())
        }
    })
}

/// `ACOTH(number)` - defined strictly outside `-1..1`.
pub fn acoth(args: &[Arg]) -> Value {
    one(args, |n| {
        if n > -1.0 && n < 1.0 {
            Value::Error(CellError::Num)
        } else {
            Value::Number((1.0 / n).atanh())
        }
    })
}

/// An inverse function that only has an answer for `-1..=1`.
fn bounded(n: f64, f: fn(f64) -> f64) -> Value {
    if (-1.0..=1.0).contains(&n) {
        Value::Number(f(n))
    } else {
        Value::Error(CellError::Num)
    }
}

/// `DEGREES(radians)`
pub fn degrees(args: &[Arg]) -> Value {
    one(args, |n| Value::Number(n.to_degrees()))
}

/// `RADIANS(degrees)`
pub fn radians(args: &[Arg]) -> Value {
    one(args, |n| Value::Number(n.to_radians()))
}

/// `EVEN(number)` - away from zero to the next even integer.
pub fn even(args: &[Arg]) -> Value {
    one(args, |n| Value::Number(away_to_step(n, 2.0)))
}

/// `ODD(number)` - away from zero to the next odd integer.
pub fn odd(args: &[Arg]) -> Value {
    one(args, |n| {
        if n == 0.0 {
            return Value::Number(1.0);
        }
        let sign = n.signum();
        Value::Number(sign * (away_to_step(n.abs() - 1.0, 2.0) + 1.0))
    })
}

/// Rounds away from zero to the next multiple of `step`.
fn away_to_step(n: f64, step: f64) -> f64 {
    if n == 0.0 {
        return 0.0;
    }
    let sign = n.signum();
    sign * (n.abs() / step).ceil() * step
}

/// `MROUND(number, multiple)` - to the nearest multiple, halves away from zero.
pub fn mround(args: &[Arg]) -> Value {
    two_numbers(args, |n, step| {
        if step == 0.0 {
            return Value::Number(0.0);
        }
        if n.signum() != step.signum() && n != 0.0 {
            // Excel refuses a number and a multiple of opposite signs.
            return Value::Error(CellError::Num);
        }
        Value::Number((n / step).round() * step)
    })
}

/// `QUOTIENT(numerator, denominator)` - the integer part of the division.
pub fn quotient(args: &[Arg]) -> Value {
    two_numbers(args, |a, b| {
        if b == 0.0 {
            Value::Error(CellError::Div0)
        } else {
            Value::Number((a / b).trunc())
        }
    })
}

/// `GCD(number1, ...)` - of the whole parts, as Excel takes them.
pub fn gcd(args: &[Arg]) -> Value {
    whole_numbers(args, |ns| ns.into_iter().fold(0u64, binary_gcd))
}

/// `LCM(number1, ...)`
pub fn lcm(args: &[Arg]) -> Value {
    whole_numbers(args, |ns| {
        ns.into_iter().fold(1u64, |acc, n| {
            if n == 0 || acc == 0 {
                return 0;
            }
            acc / binary_gcd(acc, n) * n
        })
    })
}

/// Shared body of `GCD` and `LCM`: both refuse negatives and truncate.
fn whole_numbers(args: &[Arg], body: impl Fn(Vec<u64>) -> u64) -> Value {
    let numbers = match aggregate_numbers(args) {
        Ok(ns) => ns,
        Err(e) => return Value::Error(e),
    };
    let mut whole = Vec::with_capacity(numbers.len());
    for n in numbers {
        if n < 0.0 || !n.is_finite() {
            return Value::Error(CellError::Num);
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "checked non-negative and finite; a number past u64 is \
                      past what Excel's own 15 digits can say anyway"
        )]
        let truncated = n.trunc() as u64;
        whole.push(truncated);
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "Excel stores every number as f64 regardless"
    )]
    let result = body(whole) as f64;
    Value::Number(result)
}

/// Greatest common divisor, Stein's algorithm without the shifts: the numbers
/// here are small and the plain remainder loop is the one that reads.
fn binary_gcd(a: u64, b: u64) -> u64 {
    let (mut a, mut b) = (a, b);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// `FACT(number)` - of the whole part, up to Excel's limit of 170.
pub fn fact(args: &[Arg]) -> Value {
    one(args, |n| {
        if n < 0.0 || n.trunc() > 170.0 {
            return Value::Error(CellError::Num);
        }
        let mut total = 1.0f64;
        let mut i = 2.0;
        while i <= n.trunc() {
            total *= i;
            i += 1.0;
        }
        Value::Number(total)
    })
}

/// `FACTDOUBLE(number)` - every second factor: `7!!` is 7*5*3*1.
pub fn factdouble(args: &[Arg]) -> Value {
    one(args, |n| {
        if n < -1.0 {
            return Value::Error(CellError::Num);
        }
        let n = n.trunc();
        let mut total = 1.0f64;
        let mut i = n;
        while i > 1.0 {
            total *= i;
            i -= 2.0;
        }
        if total.is_finite() {
            Value::Number(total)
        } else {
            Value::Error(CellError::Num)
        }
    })
}

/// `COMBIN(n, k)` - how many ways to choose k of n, order not counting.
pub fn combin(args: &[Arg]) -> Value {
    two_numbers(args, |n, k| {
        let (n, k) = (n.trunc(), k.trunc());
        if n < 0.0 || k < 0.0 || k > n {
            return Value::Error(CellError::Num);
        }
        Value::Number(choose(n, k))
    })
}

/// `COMBINA(n, k)` - the same with repetition allowed.
pub fn combina(args: &[Arg]) -> Value {
    two_numbers(args, |n, k| {
        let (n, k) = (n.trunc(), k.trunc());
        if n < 0.0 || k < 0.0 || (n == 0.0 && k > 0.0) {
            return Value::Error(CellError::Num);
        }
        if k == 0.0 {
            return Value::Number(1.0);
        }
        Value::Number(choose(n + k - 1.0, k))
    })
}

/// `PERMUT(n, k)` - the same as `COMBIN` with order counting.
pub fn permut(args: &[Arg]) -> Value {
    two_numbers(args, |n, k| {
        let (n, k) = (n.trunc(), k.trunc());
        if n < 0.0 || k < 0.0 || k > n {
            return Value::Error(CellError::Num);
        }
        let mut total = 1.0f64;
        let mut i = 0.0;
        while i < k {
            total *= n - i;
            i += 1.0;
        }
        Value::Number(total)
    })
}

/// Binomial coefficient, multiplied term by term so the intermediate values
/// stay small: `COMBIN(1000,500)` overflows if the factorials are built first.
fn choose(n: f64, k: f64) -> f64 {
    let k = k.min(n - k);
    let mut total = 1.0f64;
    let mut i = 0.0;
    while i < k {
        total = total * (n - i) / (i + 1.0);
        i += 1.0;
    }
    total.round()
}

/// `SUMSQ(number1, ...)`
pub fn sumsq(args: &[Arg]) -> Value {
    match aggregate_numbers(args) {
        Ok(ns) => Value::Number(ns.iter().fold(0.0, |acc, n| acc + n * n)),
        Err(e) => Value::Error(e),
    }
}

/// `SUMX2MY2(array_x, array_y)` - the sum of `x^2 - y^2` over the pairs.
pub fn sumx2my2(args: &[Arg]) -> Value {
    paired(args, |x, y| x * x - y * y)
}

/// `SUMX2PY2(array_x, array_y)`
pub fn sumx2py2(args: &[Arg]) -> Value {
    paired(args, |x, y| x * x + y * y)
}

/// `SUMXMY2(array_x, array_y)` - the sum of the squared differences.
pub fn sumxmy2(args: &[Arg]) -> Value {
    paired(args, |x, y| (x - y) * (x - y))
}

/// Shared body of the three paired sums.
///
/// A pair where either side is not a number is skipped rather than counted as
/// zero, which is what Excel does and what tells these apart from `SUMPRODUCT`.
fn paired(args: &[Arg], body: impl Fn(f64, f64) -> f64) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let [x, y] = args else {
        return Value::Error(CellError::Value);
    };
    let (xs, ys) = (super::cells(x), super::cells(y));
    if xs.len() != ys.len() {
        return Value::Error(CellError::Na);
    }
    let mut total = 0.0;
    for (a, b) in xs.iter().zip(ys) {
        if let (Value::Number(a), Value::Number(b)) = (a, b) {
            total += body(*a, *b);
        }
    }
    Value::Number(total)
}

/// `SERIESSUM(x, n, m, coefficients)` - the sum of `c * x^(n + i*m)`.
pub fn seriessum(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let [x, n, m, coefficients] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(x), Ok(n), Ok(m)) = (x.number(), n.number(), m.number()) else {
        return Value::Error(CellError::Value);
    };
    let mut total = 0.0;
    for (i, c) in super::cells(coefficients).iter().enumerate() {
        let Value::Number(c) = c else {
            return Value::Error(CellError::Value);
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "an index into one argument's cells is far inside f64"
        )]
        let step = i as f64;
        total += c * x.powf(n + step * m);
    }
    Value::Number(total)
}

/// `SQRTPI(number)` - the square root of `number * pi`.
pub fn sqrtpi(args: &[Arg]) -> Value {
    one(args, |n| {
        if n < 0.0 {
            Value::Error(CellError::Num)
        } else {
            Value::Number((n * core::f64::consts::PI).sqrt())
        }
    })
}

/// `MULTINOMIAL(number1, ...)` - `(sum)! / (a! * b! * ...)`.
pub fn multinomial(args: &[Arg]) -> Value {
    let numbers = match aggregate_numbers(args) {
        Ok(ns) => ns,
        Err(e) => return Value::Error(e),
    };
    let mut total = 1.0f64;
    let mut running = 0.0f64;
    for n in numbers {
        if n < 0.0 {
            return Value::Error(CellError::Num);
        }
        let n = n.trunc();
        running += n;
        // Built up as a product of binomial coefficients rather than a ratio of
        // factorials, which overflows for arguments Excel still answers for.
        total *= choose(running, n);
    }
    Value::Number(total)
}

/// `MUNIT(size)` - the identity matrix of that size.
pub fn munit(args: &[Arg]) -> Value {
    one(args, |n| {
        let n = n.trunc();
        if !(1.0..=1000.0).contains(&n) {
            return Value::Error(CellError::Value);
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "bounded to 1..=1000 on the line above"
        )]
        let size = n as usize;
        Value::array(
            (0..size)
                .map(|r| {
                    (0..size)
                        .map(|c| Value::Number(f64::from(u8::from(r == c))))
                        .collect()
                })
                .collect(),
        )
    })
}

/// `MMULT(a, b)` - matrix multiplication, so the width of one has to be the
/// height of the other.
pub fn mmult(args: &[Arg]) -> Value {
    let [a, b] = args else {
        return Value::Error(CellError::Value);
    };
    let (Some(a), Some(b)) = (matrix(a), matrix(b)) else {
        return Value::Error(CellError::Value);
    };
    let inner = a.first().map_or(0, Vec::len);
    if inner != b.len() || inner == 0 {
        return Value::Error(CellError::Value);
    }
    let width = b.first().map_or(0, Vec::len);
    let product = a
        .iter()
        .map(|row| {
            (0..width)
                .map(|c| {
                    let total: f64 = (0..inner).map(|k| row[k] * b[k][c]).sum();
                    Value::Number(total)
                })
                .collect()
        })
        .collect();
    Value::array(product)
}

/// `MDETERM(array)` - the determinant, by the same elimination that inverts.
pub fn mdeterm(args: &[Arg]) -> Value {
    let [arg] = args else {
        return Value::Error(CellError::Value);
    };
    let Some(square) = square_matrix(arg) else {
        return Value::Error(CellError::Value);
    };
    Value::Number(tidy(determinant(square)))
}

/// `MINVERSE(array)`
pub fn minverse(args: &[Arg]) -> Value {
    let [arg] = args else {
        return Value::Error(CellError::Value);
    };
    let Some(square) = square_matrix(arg) else {
        return Value::Error(CellError::Value);
    };
    match crate::formula::functions::regression::invert_matrix(&square) {
        Some(inverse) => Value::array(
            inverse
                .into_iter()
                .map(|row| row.into_iter().map(|n| Value::Number(tidy(n))).collect())
                .collect(),
        ),
        // A matrix with no inverse is a numeric failure, not a wrong argument.
        None => Value::Error(CellError::Num),
    }
}

/// Fifteen significant digits, which is what Excel shows and what turns the
/// elimination's last-bit noise back into the whole number it came from: the
/// inverse of `{1,2;3,4}` is `{-2,1;1.5,-0.5}`, not `-1.9999999999999996`.
fn tidy(n: f64) -> f64 {
    if !n.is_finite() {
        return n;
    }
    format!("{n:.14e}").parse().unwrap_or(n)
}

/// The determinant by Gaussian elimination with partial pivoting.
fn determinant(mut work: Vec<Vec<f64>>) -> f64 {
    let n = work.len();
    let mut result = 1.0;
    for column in 0..n {
        let Some(pivot) =
            (column..n).max_by(|a, b| work[*a][column].abs().total_cmp(&work[*b][column].abs()))
        else {
            return 0.0;
        };
        if work[pivot][column].abs() < 1.0e-300 {
            return 0.0;
        }
        if pivot != column {
            work.swap(column, pivot);
            // Every swap of two rows flips the sign.
            result = -result;
        }
        result *= work[column][column];
        let scale = work[column][column];
        for row in column + 1..n {
            let factor = work[row][column] / scale;
            if factor == 0.0 {
                continue;
            }
            // The pivot row is read while another is written, so the two
            // have to be borrowed apart.
            let (before, after) = work.split_at_mut(row);
            let (target, pivot) = (&mut after[0], &before[column]);
            for (value, above) in target[column..].iter_mut().zip(&pivot[column..]) {
                *value -= factor * above;
            }
        }
    }
    result
}

/// An argument as a rectangle of numbers.
fn matrix(arg: &Arg) -> Option<Vec<Vec<f64>>> {
    let Value::Array(rows) = &arg.value else {
        return arg.number().ok().map(|n| vec![vec![n]]);
    };
    let width = rows.first()?.len();
    rows.iter()
        .map(|row| {
            (row.len() == width)
                .then(|| {
                    row.iter()
                        .map(|v| match v.scalar() {
                            Value::Number(n) => Some(*n),
                            _ => None,
                        })
                        .collect::<Option<Vec<f64>>>()
                })
                .flatten()
        })
        .collect()
}

/// The same, insisting it is square.
fn square_matrix(arg: &Arg) -> Option<Vec<Vec<f64>>> {
    let rows = matrix(arg)?;
    let width = rows.first()?.len();
    (width == rows.len() && width > 0).then_some(rows)
}

/// `SEQUENCE(rows, [columns], [start], [step])`
pub fn sequence(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let number = |at: usize, default: f64| {
        args.get(at)
            .filter(|a| !a.missing())
            .map_or(Ok(default), Arg::number)
    };
    if args.is_empty() || args.len() > 4 {
        return Value::Error(CellError::Value);
    }
    let (Ok(rows), Ok(columns), Ok(start), Ok(step)) = (
        number(0, 1.0),
        number(1, 1.0),
        number(2, 1.0),
        number(3, 1.0),
    ) else {
        return Value::Error(CellError::Value);
    };
    let size = |n: f64| -> Option<usize> {
        let n = n.trunc();
        (1.0..=1.0e6).contains(&n).then(|| {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "bounded to 1..=1e6 on the line above"
            )]
            let size = n as usize;
            size
        })
    };
    let (Some(rows), Some(columns)) = (size(rows), size(columns)) else {
        return Value::Error(CellError::Value);
    };
    if rows.saturating_mul(columns) > 1_000_000 {
        return Value::Error(CellError::Num);
    }
    Value::array(
        (0..rows)
            .map(|r| {
                (0..columns)
                    .map(|c| {
                        #[expect(
                            clippy::cast_precision_loss,
                            reason = "a count bounded by a million"
                        )]
                        let step_count = (r * columns + c) as f64;
                        Value::Number(start + step_count * step)
                    })
                    .collect()
            })
            .collect(),
    )
}

/// `ROMAN(number, [form])` - the classic form only, which is form 0.
pub fn roman(args: &[Arg]) -> Value {
    const NUMERALS: [(u32, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];

    let number = match args {
        [n] | [n, _] => n.number(),
        _ => return Value::Error(CellError::Value),
    };
    let Ok(number) = number else {
        return Value::Error(CellError::Value);
    };
    let number = number.trunc();
    if !(0.0..=3999.0).contains(&number) {
        return Value::Error(CellError::Value);
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bounded to 0..=3999 on the line above"
    )]
    let mut left = number as u32;
    let mut out = String::new();
    for (value, numeral) in NUMERALS {
        while left >= value {
            out.push_str(numeral);
            left -= value;
        }
    }
    Value::Text(out)
}

/// `ARABIC(text)` - the number a Roman numeral spells.
pub fn arabic(args: &[Arg]) -> Value {
    let [arg] = args else {
        return Value::Error(CellError::Value);
    };
    let text = match arg.text() {
        Ok(text) => text.trim().to_uppercase(),
        Err(e) => return Value::Error(e),
    };
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, text.as_str()),
    };
    let value = |c: char| match c {
        'I' => Some(1),
        'V' => Some(5),
        'X' => Some(10),
        'L' => Some(50),
        'C' => Some(100),
        'D' => Some(500),
        'M' => Some(1000),
        _ => None,
    };
    let mut total = 0i32;
    let mut previous = 0i32;
    // Read from the right: a numeral smaller than the one after it subtracts.
    for c in digits.chars().rev() {
        let Some(n) = value(c) else {
            return Value::Error(CellError::Value);
        };
        if n < previous {
            total -= n;
        } else {
            total += n;
            previous = n;
        }
    }
    Value::Number(sign * f64::from(total))
}

/// `BASE(number, radix, [min_length])`
pub fn base(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (number, radix, length) = match args {
        [n, r] => (n.number(), r.number(), Ok(0.0)),
        [n, r, l] => (n.number(), r.number(), l.number()),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(number), Ok(radix), Ok(length)) = (number, radix, length) else {
        return Value::Error(CellError::Value);
    };
    let radix = radix.trunc();
    if !(2.0..=36.0).contains(&radix) || number < 0.0 || number >= 2f64.powi(53) {
        return Value::Error(CellError::Num);
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "range-checked on the line above"
    )]
    let (mut left, radix) = (number.trunc() as u64, radix as u64);
    let mut digits = Vec::new();
    while left > 0 {
        let digit = usize::try_from(left % radix).unwrap_or(0);
        digits.push(b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"[digit]);
        left /= radix;
    }
    if digits.is_empty() {
        digits.push(b'0');
    }
    digits.reverse();
    let text = String::from_utf8(digits).unwrap_or_default();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a padding width Excel caps at 255"
    )]
    let width = length.trunc().clamp(0.0, 255.0) as usize;
    Value::Text(format!("{text:0>width$}"))
}

/// `DECIMAL(text, radix)` - the inverse of `BASE`.
pub fn decimal(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let [text, radix] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(text), Ok(radix)) = (text.text(), radix.number()) else {
        return Value::Error(CellError::Value);
    };
    let radix = radix.trunc();
    if !(2.0..=36.0).contains(&radix) {
        return Value::Error(CellError::Num);
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bounded to 2..=36 on the line above"
    )]
    let radix = radix as u32;
    match u64::from_str_radix(text.trim(), radix) {
        #[expect(
            clippy::cast_precision_loss,
            reason = "Excel stores every number as f64 regardless"
        )]
        Ok(n) => Value::Number(n as f64),
        Err(_) => Value::Error(CellError::Num),
    }
}

/// `CEILING.MATH(number, [significance], [mode])`, and the precise forms.
///
/// Where plain `CEILING` refuses a positive number with a negative step, these
/// take the size of the step and ignore its sign. `mode` decides which way a
/// negative number goes: away from zero when it is set.
pub fn ceiling_math(args: &[Arg]) -> Value {
    directed(args, true)
}

/// `FLOOR.MATH(number, [significance], [mode])`
pub fn floor_math(args: &[Arg]) -> Value {
    directed(args, false)
}

/// `CEILING.PRECISE(number, [significance])`, and `ISO.CEILING`: always
/// towards positive infinity, whatever the signs.
pub fn ceiling_precise(args: &[Arg]) -> Value {
    toward_infinity(args, true)
}

/// `FLOOR.PRECISE(number, [significance])`
pub fn floor_precise(args: &[Arg]) -> Value {
    toward_infinity(args, false)
}

/// Shared body of the `.MATH` pair.
fn directed(args: &[Arg], up: bool) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let number = |at: usize, default: f64| {
        args.get(at)
            .filter(|a| !a.missing())
            .map_or(Ok(default), Arg::number)
    };
    if args.is_empty() || args.len() > 3 {
        return Value::Error(CellError::Value);
    }
    let (Ok(n), Ok(step), Ok(mode)) = (number(0, 0.0), number(1, 1.0), number(2, 0.0)) else {
        return Value::Error(CellError::Value);
    };
    let step = step.abs();
    if step == 0.0 {
        return Value::Number(0.0);
    }
    // With the mode set, a negative number rounds away from zero rather than
    // towards it; the two swap which of ceil and floor does the work.
    let away = n < 0.0 && mode != 0.0;
    // The mode flips which of the two does the work for a negative number.
    let toward_larger = up ^ away;
    let rounded = if toward_larger {
        (n / step).ceil()
    } else {
        (n / step).floor()
    };
    Value::Number(rounded * step)
}

/// Shared body of the `.PRECISE` family.
fn toward_infinity(args: &[Arg], up: bool) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (n, step) = match args {
        [n] => (n.number(), Ok(1.0)),
        [n, s] => (n.number(), s.number()),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(n), Ok(step)) = (n, step) else {
        return Value::Error(CellError::Value);
    };
    let step = step.abs();
    if step == 0.0 {
        return Value::Number(0.0);
    }
    let rounded = if up {
        (n / step).ceil()
    } else {
        (n / step).floor()
    };
    Value::Number(rounded * step)
}

/// `SUBTOTAL(function, ref1, ...)` - one of eleven aggregates by number.
///
/// A code above 100 skips rows the user hid, which this cannot see from the
/// values alone, so both halves behave the same here. What it does honour is
/// the rule that gives the function its name: a `SUBTOTAL` inside a range
/// another `SUBTOTAL` covers is not counted twice.
pub fn subtotal(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    let Some((code, rest)) = args.split_first() else {
        return Value::Error(CellError::Value);
    };
    let Ok(code) = engine.eval_expr(origin, code).number() else {
        return Value::Error(CellError::Value);
    };
    let code = code.trunc();
    let code = if code > 100.0 { code - 100.0 } else { code };
    aggregate_by_code(engine, origin, code, rest, 0.0)
}

/// `AGGREGATE(function, options, ref1, ...)` - the same list, plus options
/// saying what to leave out.
pub fn aggregate(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    let [code, options, rest @ ..] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(code), Ok(options)) = (
        engine.eval_expr(origin, code).number(),
        engine.eval_expr(origin, options).number(),
    ) else {
        return Value::Error(CellError::Value);
    };
    aggregate_by_code(engine, origin, code.trunc(), rest, options.trunc())
}

/// Shared body of `SUBTOTAL` and `AGGREGATE`.
fn aggregate_by_code(
    engine: &mut Engine<'_>,
    origin: Origin,
    code: f64,
    args: &[Expr],
    options: f64,
) -> Value {
    // Both are small codes rather than measurements, so they belong in an
    // integer; anything outside the tables below is refused further down.
    let code_of = |n: f64| -> i64 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "clamped to a range far inside i64 on this line"
        )]
        let code = n.trunc().clamp(-1000.0, 1000.0) as i64;
        code
    };
    // Options 2, 3, 6 and 7 say to pass over errors rather than report them.
    let skip_errors = matches!(code_of(options), 2 | 3 | 6 | 7);
    // Options 0 to 3 leave out nested SUBTOTAL and AGGREGATE cells, 4 to 7
    // count them; SUBTOTAL comes here with 0.
    let skip_totals = matches!(code_of(options), 0..=3);
    let mut values: Vec<Arg> = Vec::with_capacity(args.len());
    for arg in args {
        let mut value = engine.eval_expr(origin, arg);
        if skip_totals {
            value = engine.without_totals(origin, arg, value);
        }
        if skip_errors {
            values.push(Arg {
                value: without_errors(value),
                reference: true,
            });
        } else {
            values.push(Arg {
                value,
                reference: matches!(arg, Expr::Range { .. }),
            });
        }
    }
    let body: fn(&[Arg]) -> Value = match code_of(code) {
        1 => super::stats::average,
        2 => super::stats::count,
        3 => super::stats::counta,
        4 => super::stats::max,
        5 => super::stats::min,
        6 => product,
        7 => super::stats::stdev,
        8 => super::stats::stdevp,
        9 => sum,
        10 => super::stats::var,
        11 => super::stats::varp,
        12 => super::stats::median,
        13 => super::stats::mode,
        14 => super::stats::large,
        15 => super::stats::small,
        16 => super::stats::percentile,
        17 => super::stats::quartile,
        _ => return Value::Error(CellError::Value),
    };
    body(&values)
}

/// A value with its errors taken out, for the options that ignore them.
fn without_errors(value: Value) -> Value {
    match value {
        Value::Array(rows) => Value::array(
            std::sync::Arc::unwrap_or_clone(rows)
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|v| {
                            if matches!(v, Value::Error(_)) {
                                Value::Blank
                            } else {
                                v
                            }
                        })
                        .collect()
                })
                .collect(),
        ),
        Value::Error(_) => Value::Blank,
        other => other,
    }
}

/// `RAND()` - a number in `0..1`.
///
/// The generator is a small permutation of a counter seeded from the clock:
/// good enough for a spreadsheet, and nothing here is cryptography. Excel's
/// own `RAND` is not reproducible either, so no test can pin its value.
pub fn rand(args: &[Arg]) -> Value {
    if !args.is_empty() {
        return Value::Error(CellError::Value);
    }
    Value::Number(next_random())
}

/// `RANDBETWEEN(low, high)` - a whole number in the range, both ends included.
pub fn randbetween(args: &[Arg]) -> Value {
    let [low, high] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(low), Ok(high)) = (low.number(), high.number()) else {
        return Value::Error(CellError::Value);
    };
    let (low, high) = (low.ceil(), high.floor());
    if low > high {
        return Value::Error(CellError::Num);
    }
    Value::Number(low + (next_random() * (high - low + 1.0)).floor().min(high - low))
}

/// `RANDARRAY([rows], [columns], [min], [max], [whole])`
pub fn randarray(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    if args.len() > 5 {
        return Value::Error(CellError::Value);
    }
    let number = |at: usize, default: f64| {
        args.get(at)
            .filter(|a| !a.missing())
            .map_or(Ok(default), Arg::number)
    };
    let (Ok(rows), Ok(columns), Ok(low), Ok(high)) = (
        number(0, 1.0),
        number(1, 1.0),
        number(2, 0.0),
        number(3, 1.0),
    ) else {
        return Value::Error(CellError::Value);
    };
    let whole = args
        .get(4)
        .and_then(|a| a.value.boolean().ok())
        .unwrap_or(false);
    if low > high {
        return Value::Error(CellError::Value);
    }
    let size = |n: f64| -> Option<usize> {
        let n = n.trunc();
        (1.0..=1.0e6).contains(&n).then(|| {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "bounded to 1..=1e6 on the line above"
            )]
            let size = n as usize;
            size
        })
    };
    let (Some(rows), Some(columns)) = (size(rows), size(columns)) else {
        return Value::Error(CellError::Value);
    };
    if rows.saturating_mul(columns) > crate::formula::eval::MAX_RANGE_CELLS {
        return Value::Error(CellError::Num);
    }
    Value::array(
        (0..rows)
            .map(|_| {
                (0..columns)
                    .map(|_| {
                        let raw = low + next_random() * (high - low);
                        Value::Number(if whole { raw.floor() } else { raw })
                    })
                    .collect()
            })
            .collect(),
    )
}

/// The next number of a small pseudorandom sequence.
///
/// A counter through a mixing function: it has no state to share between
/// threads beyond the counter itself, and it needs no dependency.
fn next_random() -> f64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    // Seeded once from the clock so two runs differ, then advanced by the
    // odd constant that makes the low bits of a counter cycle through all of
    // them (SplitMix64).
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "any bits of the clock will do as a seed"
    )]
    let seed = (crate::shared::unix_seconds().fract() * 1e9) as u64;
    let mut z = COUNTER
        .fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed)
        .wrapping_add(seed);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // The top 53 bits are the ones an f64 can hold exactly.
    #[expect(
        clippy::cast_precision_loss,
        reason = "53 bits is exactly what f64's mantissa holds"
    )]
    let fraction = (z >> 11) as f64 / (1u64 << 53) as f64;
    fraction
}
