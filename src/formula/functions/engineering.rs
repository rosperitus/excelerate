//! Engineering.
//!
//! Four families that share nothing but the category: converting between number
//! bases, working on the bits of a whole number, the error function and its
//! step-shaped relatives, and complex numbers written as the text `"3+4i"`.

use super::{Arg, first_error};
use crate::error::CellError;
use crate::formula::value::Value;
use crate::shared::special::{erf, erfc};

/// How many digits Excel allows a based number, and the sign bit it uses.
///
/// Ten digits, the top one meaning negative: `1111111111` in binary is -1, not
/// 1023. The same two's-complement rule holds for octal and hexadecimal.
const DIGITS: u32 = 10;

/// `BIN2DEC(text)`
pub fn bin2dec(args: &[Arg]) -> Value {
    from_base(args, 2)
}

/// `OCT2DEC(text)`
pub fn oct2dec(args: &[Arg]) -> Value {
    from_base(args, 8)
}

/// `HEX2DEC(text)`
pub fn hex2dec(args: &[Arg]) -> Value {
    from_base(args, 16)
}

/// `DEC2BIN(number, [places])`
pub fn dec2bin(args: &[Arg]) -> Value {
    to_base(args, 2)
}

/// `DEC2OCT(number, [places])`
pub fn dec2oct(args: &[Arg]) -> Value {
    to_base(args, 8)
}

/// `DEC2HEX(number, [places])`
pub fn dec2hex(args: &[Arg]) -> Value {
    to_base(args, 16)
}

/// `BIN2OCT(text, [places])`
pub fn bin2oct(args: &[Arg]) -> Value {
    between_bases(args, 2, 8)
}

/// `BIN2HEX(text, [places])`
pub fn bin2hex(args: &[Arg]) -> Value {
    between_bases(args, 2, 16)
}

/// `OCT2BIN(text, [places])`
pub fn oct2bin(args: &[Arg]) -> Value {
    between_bases(args, 8, 2)
}

/// `OCT2HEX(text, [places])`
pub fn oct2hex(args: &[Arg]) -> Value {
    between_bases(args, 8, 16)
}

/// `HEX2BIN(text, [places])`
pub fn hex2bin(args: &[Arg]) -> Value {
    between_bases(args, 16, 2)
}

/// `HEX2OCT(text, [places])`
pub fn hex2oct(args: &[Arg]) -> Value {
    between_bases(args, 16, 8)
}

/// Reads a based number into a decimal one.
fn from_base(args: &[Arg], base: u32) -> Value {
    let [arg] = args else {
        return Value::Error(CellError::Value);
    };
    match read_digits(arg, base) {
        Some(n) => Value::Number(n),
        None => Value::Error(CellError::Num),
    }
}

/// Writes a decimal number in another base.
fn to_base(args: &[Arg], base: u32) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (number, places) = match args {
        [n] => (n.number(), None),
        [n, p] => (n.number(), Some(p.number())),
        _ => return Value::Error(CellError::Value),
    };
    let Ok(number) = number else {
        return Value::Error(CellError::Value);
    };
    write_digits(number.trunc(), base, places)
}

/// Reads in one base and writes in another.
fn between_bases(args: &[Arg], from: u32, to: u32) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (text, places) = match args {
        [t] => (t, None),
        [t, p] => (t, Some(p.number())),
        _ => return Value::Error(CellError::Value),
    };
    match read_digits(text, from) {
        Some(n) => write_digits(n, to, places),
        None => Value::Error(CellError::Num),
    }
}

/// The value of a based number, negative when its top digit is set.
fn read_digits(arg: &Arg, base: u32) -> Option<f64> {
    let text = match arg.value.scalar() {
        Value::Text(text) => text.clone(),
        // A number given where digits were expected is read as its own text.
        Value::Number(n) if n.fract() == 0.0 && *n >= 0.0 => format!("{}", n.trunc()),
        _ => return None,
    };
    let text = text.trim();
    if text.is_empty() || text.len() > DIGITS as usize {
        return None;
    }
    let raw = i64::from_str_radix(text, base).ok()?;
    // The top digit is the sign, so half the range counts backwards.
    let span = i64::from(base).checked_pow(DIGITS)?;
    Some(if raw >= span / 2 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a ten-digit number is far inside what f64 counts exactly"
        )]
        let n = (raw - span) as f64;
        n
    } else {
        #[expect(clippy::cast_precision_loss, reason = "as above")]
        let n = raw as f64;
        n
    })
}

/// A number as digits in a base, padded to `places` if one is given.
fn write_digits(number: f64, base: u32, places: Option<Result<f64, CellError>>) -> Value {
    let places = match places {
        Some(Ok(p)) => Some(p.trunc()),
        Some(Err(e)) => return Value::Error(e),
        None => None,
    };
    let span = f64::from(base).powi(DIGITS.cast_signed());
    if !number.is_finite() || number.abs() > span / 2.0 {
        return Value::Error(CellError::Num);
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "bounded by half the ten-digit span just above"
    )]
    let whole = number as i64;
    let raw = if whole < 0 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the span of ten digits in base 16 fits an i64"
        )]
        let span = span as i64;
        span + whole
    } else {
        whole
    };
    let digits = to_radix(raw, base);
    if whole < 0 {
        // A negative number always fills the width, so a places argument
        // cannot make it shorter and is simply ignored.
        return Value::Text(digits);
    }
    match places {
        None => Value::Text(digits),
        Some(places) if places < 1.0 || places > f64::from(DIGITS) => Value::Error(CellError::Num),
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "range-checked on the arm above"
        )]
        Some(places) if (places as usize) < digits.len() => Value::Error(CellError::Num),
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "as above"
        )]
        Some(places) => Value::Text(format!("{digits:0>width$}", width = places as usize)),
    }
}

/// A non-negative number as digits in a base, upper case for hexadecimal.
fn to_radix(mut n: i64, base: u32) -> String {
    if n == 0 {
        return "0".to_owned();
    }
    let base = i64::from(base);
    let mut out = Vec::new();
    while n > 0 {
        let digit = usize::try_from(n % base).unwrap_or(0);
        out.push(b"0123456789ABCDEF"[digit]);
        n /= base;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// `BITAND(a, b)`
pub fn bitand(args: &[Arg]) -> Value {
    bitwise(args, |a, b| a & b)
}

/// `BITOR(a, b)`
pub fn bitor(args: &[Arg]) -> Value {
    bitwise(args, |a, b| a | b)
}

/// `BITXOR(a, b)`
pub fn bitxor(args: &[Arg]) -> Value {
    bitwise(args, |a, b| a ^ b)
}

/// `BITLSHIFT(number, places)` - a negative count shifts the other way.
pub fn bitlshift(args: &[Arg]) -> Value {
    shift(args, true)
}

/// `BITRSHIFT(number, places)`
pub fn bitrshift(args: &[Arg]) -> Value {
    shift(args, false)
}

/// The largest number Excel's bit functions work on: 2^48 - 1.
const BIT_LIMIT: f64 = 281_474_976_710_655.0;

/// Shared body of the three bitwise pairs.
fn bitwise(args: &[Arg], body: fn(u64, u64) -> u64) -> Value {
    let [a, b] = args else {
        return Value::Error(CellError::Value);
    };
    let (Some(a), Some(b)) = (whole_bits(a), whole_bits(b)) else {
        return Value::Error(CellError::Num);
    };
    #[expect(
        clippy::cast_precision_loss,
        reason = "the result is under 2^48, which f64 counts exactly"
    )]
    let out = body(a, b) as f64;
    Value::Number(out)
}

/// Shared body of the two shifts.
fn shift(args: &[Arg], left: bool) -> Value {
    let [number, places] = args else {
        return Value::Error(CellError::Value);
    };
    let (Some(number), Ok(places)) = (whole_bits(number), places.number()) else {
        return Value::Error(CellError::Num);
    };
    let places = places.trunc();
    if places.abs() > 53.0 {
        return Value::Error(CellError::Num);
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "bounded to 53 on the line above"
    )]
    let places = places as i32;
    // A shift the other way is the other shift, which is what Excel means by
    // a negative count.
    let towards_left = if places < 0 { !left } else { left };
    let by = places.unsigned_abs();
    let shifted = if towards_left {
        number.checked_shl(by).unwrap_or(0)
    } else {
        number.checked_shr(by).unwrap_or(0)
    };
    #[expect(
        clippy::cast_precision_loss,
        reason = "checked against the limit below"
    )]
    let out = shifted as f64;
    if out > BIT_LIMIT {
        return Value::Error(CellError::Num);
    }
    Value::Number(out)
}

/// An argument as the whole non-negative number the bit functions need.
fn whole_bits(arg: &Arg) -> Option<u64> {
    let n = arg.number().ok()?;
    if n.fract() != 0.0 || !(0.0..=BIT_LIMIT).contains(&n) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "checked whole, non-negative and under 2^48 just above"
    )]
    let whole = n as u64;
    Some(whole)
}

/// `DELTA(a, [b])` - one when the two are equal, and nothing otherwise.
pub fn delta(args: &[Arg]) -> Value {
    let (a, b) = match args {
        [a] => (a.number(), Ok(0.0)),
        [a, b] => (a.number(), b.number()),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(a), Ok(b)) = (a, b) else {
        return Value::Error(CellError::Value);
    };
    // The same number exactly, which is what DELTA asks about.
    Value::Number(f64::from(u8::from(
        a.total_cmp(&b) == core::cmp::Ordering::Equal,
    )))
}

/// `GESTEP(number, [step])` - one when the number reaches the step.
pub fn gestep(args: &[Arg]) -> Value {
    let (number, step) = match args {
        [n] => (n.number(), Ok(0.0)),
        [n, s] => (n.number(), s.number()),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(number), Ok(step)) = (number, step) else {
        return Value::Error(CellError::Value);
    };
    Value::Number(f64::from(u8::from(number >= step)))
}

/// `ERF(lower, [upper])`, and `ERF.PRECISE`, which takes only the one bound.
pub fn erf_fn(args: &[Arg]) -> Value {
    let (lower, upper) = match args {
        [a] => (a.number(), None),
        [a, b] => (a.number(), Some(b.number())),
        _ => return Value::Error(CellError::Value),
    };
    let Ok(lower) = lower else {
        return Value::Error(CellError::Value);
    };
    match upper {
        // Between two bounds it is the difference of the two integrals.
        Some(Ok(upper)) => Value::Number(erf(upper) - erf(lower)),
        Some(Err(e)) => Value::Error(e),
        None => Value::Number(erf(lower)),
    }
}

/// `ERFC(x)`, and `ERFC.PRECISE`.
pub fn erfc_fn(args: &[Arg]) -> Value {
    super::one(args, |x| Value::Number(erfc(x)))
}

/// A complex number, as Excel writes it: `"3+4i"`, `"-2.5j"`, `"7"`.
#[derive(Clone, Copy)]
struct Complex {
    real: f64,
    imaginary: f64,
    /// Which letter the text used, since Excel keeps it in the answer.
    suffix: char,
}

impl Complex {
    /// Reads one out of a value.
    fn parse(value: &Value) -> Option<Self> {
        let text = match value.scalar() {
            Value::Number(n) => {
                return Some(Self {
                    real: *n,
                    imaginary: 0.0,
                    suffix: 'i',
                });
            }
            Value::Text(text) => text.trim().to_owned(),
            _ => return None,
        };
        if text.is_empty() {
            return None;
        }
        let suffix = if text.ends_with('j') { 'j' } else { 'i' };
        if !text.ends_with(['i', 'j']) {
            // No suffix at all, so the whole of it is the real part.
            return text.parse().ok().map(|real| Self {
                real,
                imaginary: 0.0,
                suffix: 'i',
            });
        }
        let body = &text[..text.len() - 1];
        // The split is at the last sign that is not an exponent's.
        let mut split = None;
        for (i, c) in body.char_indices().skip(1) {
            if matches!(c, '+' | '-') && !body[..i].ends_with(['e', 'E']) {
                split = Some(i);
            }
        }
        let (real, imaginary) = match split {
            Some(at) => (&body[..at], &body[at..]),
            None => ("", body),
        };
        let imaginary = match imaginary {
            "" | "+" => 1.0,
            "-" => -1.0,
            other => other.parse().ok()?,
        };
        let real = if real.is_empty() {
            0.0
        } else {
            real.parse().ok()?
        };
        Some(Self {
            real,
            imaginary,
            suffix,
        })
    }

    /// Writes one back out the way Excel does.
    fn show(self) -> Value {
        let round = |n: f64| {
            // Fifteen significant digits, which is what Excel shows and what
            // turns the polar form's last-bit noise back into the whole number
            // it came from: (2+3i)^2 is -5+12i, not -4.999999999999999+12i.
            let text = format!("{n:.14e}");
            text.parse::<f64>().unwrap_or(n)
        };
        let (real, imaginary) = (round(self.real), round(self.imaginary));
        if imaginary == 0.0 {
            return Value::Text(shortest(real));
        }
        // One times i is written as just the letter, and minus one as -i.
        let same = |a: f64, b: f64| a.total_cmp(&b) == core::cmp::Ordering::Equal;
        let part = if same(imaginary, 1.0) {
            String::new()
        } else if same(imaginary, -1.0) {
            "-".to_owned()
        } else {
            shortest(imaginary)
        };
        if real == 0.0 {
            return Value::Text(format!("{part}{}", self.suffix));
        }
        let sign = if imaginary < 0.0 || part.starts_with('-') {
            ""
        } else {
            "+"
        };
        Value::Text(format!("{}{sign}{part}{}", shortest(real), self.suffix))
    }

    /// How far it is from the origin.
    fn modulus(self) -> f64 {
        self.real.hypot(self.imaginary)
    }
}

/// A number without a trailing `.0`, which is how Excel writes these.
fn shortest(n: f64) -> String {
    format!("{n}")
}

/// `COMPLEX(real, imaginary, [suffix])`
pub fn complex(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (real, imaginary, suffix) = match args {
        [r, i] => (r.number(), i.number(), None),
        [r, i, s] => (r.number(), i.number(), Some(s.text())),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(real), Ok(imaginary)) = (real, imaginary) else {
        return Value::Error(CellError::Value);
    };
    let suffix = match suffix {
        None => 'i',
        Some(Ok(text)) if text == "i" || text == "j" => text.chars().next().unwrap_or('i'),
        _ => return Value::Error(CellError::Value),
    };
    Complex {
        real,
        imaginary,
        suffix,
    }
    .show()
}

/// `IMREAL(number)`
pub fn imreal(args: &[Arg]) -> Value {
    one_complex(args, |z| Value::Number(z.real))
}

/// `IMAGINARY(number)`
pub fn imaginary(args: &[Arg]) -> Value {
    one_complex(args, |z| Value::Number(z.imaginary))
}

/// `IMABS(number)`
pub fn imabs(args: &[Arg]) -> Value {
    one_complex(args, |z| Value::Number(z.modulus()))
}

/// `IMARGUMENT(number)` - the angle it makes, in radians.
pub fn imargument(args: &[Arg]) -> Value {
    one_complex(args, |z| {
        if z.real == 0.0 && z.imaginary == 0.0 {
            return Value::Error(CellError::Div0);
        }
        Value::Number(z.imaginary.atan2(z.real))
    })
}

/// `IMCONJUGATE(number)`
pub fn imconjugate(args: &[Arg]) -> Value {
    one_complex(args, |z| {
        Complex {
            imaginary: -z.imaginary,
            ..z
        }
        .show()
    })
}

/// `IMSUM(number1, ...)`
pub fn imsum(args: &[Arg]) -> Value {
    fold_complex(args, |a, b| Complex {
        real: a.real + b.real,
        imaginary: a.imaginary + b.imaginary,
        suffix: a.suffix,
    })
}

/// `IMSUB(a, b)`
pub fn imsub(args: &[Arg]) -> Value {
    two_complex(args, |a, b| {
        Some(Complex {
            real: a.real - b.real,
            imaginary: a.imaginary - b.imaginary,
            suffix: a.suffix,
        })
    })
}

/// `IMPRODUCT(number1, ...)`
pub fn improduct(args: &[Arg]) -> Value {
    fold_complex(args, |a, b| Complex {
        real: a.real * b.real - a.imaginary * b.imaginary,
        imaginary: a.real * b.imaginary + a.imaginary * b.real,
        suffix: a.suffix,
    })
}

/// `IMDIV(a, b)`
pub fn imdiv(args: &[Arg]) -> Value {
    two_complex(args, |a, b| {
        let divisor = b.real * b.real + b.imaginary * b.imaginary;
        if divisor == 0.0 {
            return None;
        }
        Some(Complex {
            real: (a.real * b.real + a.imaginary * b.imaginary) / divisor,
            imaginary: (a.imaginary * b.real - a.real * b.imaginary) / divisor,
            suffix: a.suffix,
        })
    })
}

/// `IMEXP(number)`
pub fn imexp(args: &[Arg]) -> Value {
    map_complex(args, |z| {
        let scale = z.real.exp();
        Some(Complex {
            real: scale * z.imaginary.cos(),
            imaginary: scale * z.imaginary.sin(),
            suffix: z.suffix,
        })
    })
}

/// `IMLN(number)`
pub fn imln(args: &[Arg]) -> Value {
    map_complex(args, |z| {
        let modulus = z.modulus();
        (modulus > 0.0).then(|| Complex {
            real: modulus.ln(),
            imaginary: z.imaginary.atan2(z.real),
            suffix: z.suffix,
        })
    })
}

/// `IMLOG10(number)`
pub fn imlog10(args: &[Arg]) -> Value {
    scaled_log(args, core::f64::consts::LN_10)
}

/// `IMLOG2(number)`
pub fn imlog2(args: &[Arg]) -> Value {
    scaled_log(args, core::f64::consts::LN_2)
}

/// The natural logarithm divided through, which is every other base.
fn scaled_log(args: &[Arg], divisor: f64) -> Value {
    map_complex(args, |z| {
        let modulus = z.modulus();
        (modulus > 0.0).then(|| Complex {
            real: modulus.ln() / divisor,
            imaginary: z.imaginary.atan2(z.real) / divisor,
            suffix: z.suffix,
        })
    })
}

/// `IMPOWER(number, power)`
pub fn impower(args: &[Arg]) -> Value {
    let [number, power] = args else {
        return Value::Error(CellError::Value);
    };
    let (Some(z), Ok(power)) = (Complex::parse(&number.value), power.number()) else {
        return Value::Error(CellError::Value);
    };
    let modulus = z.modulus();
    if modulus == 0.0 {
        return Value::Error(CellError::Num);
    }
    // In polar form a power is a scaling of the modulus and a turn of the
    // angle, which is the whole of de Moivre's rule.
    let angle = z.imaginary.atan2(z.real) * power;
    let scale = modulus.powf(power);
    Complex {
        real: scale * angle.cos(),
        imaginary: scale * angle.sin(),
        suffix: z.suffix,
    }
    .show()
}

/// `IMSQRT(number)`
pub fn imsqrt(args: &[Arg]) -> Value {
    map_complex(args, |z| {
        let modulus = z.modulus();
        let angle = z.imaginary.atan2(z.real) / 2.0;
        let scale = modulus.sqrt();
        Some(Complex {
            real: scale * angle.cos(),
            imaginary: scale * angle.sin(),
            suffix: z.suffix,
        })
    })
}

/// `IMSIN(number)`
pub fn imsin(args: &[Arg]) -> Value {
    map_complex(args, |z| {
        Some(Complex {
            real: z.real.sin() * z.imaginary.cosh(),
            imaginary: z.real.cos() * z.imaginary.sinh(),
            suffix: z.suffix,
        })
    })
}

/// `IMCOS(number)`
pub fn imcos(args: &[Arg]) -> Value {
    map_complex(args, |z| {
        Some(Complex {
            real: z.real.cos() * z.imaginary.cosh(),
            imaginary: -z.real.sin() * z.imaginary.sinh(),
            suffix: z.suffix,
        })
    })
}

/// `IMSINH(number)`
pub fn imsinh(args: &[Arg]) -> Value {
    map_complex(args, |z| {
        Some(Complex {
            real: z.real.sinh() * z.imaginary.cos(),
            imaginary: z.real.cosh() * z.imaginary.sin(),
            suffix: z.suffix,
        })
    })
}

/// `IMCOSH(number)`
pub fn imcosh(args: &[Arg]) -> Value {
    map_complex(args, |z| {
        Some(Complex {
            real: z.real.cosh() * z.imaginary.cos(),
            imaginary: z.real.sinh() * z.imaginary.sin(),
            suffix: z.suffix,
        })
    })
}

/// `IMTAN(number)` - the sine over the cosine, and the rest follow from it.
pub fn imtan(args: &[Arg]) -> Value {
    ratio_of(args, imsin, imcos)
}

/// `IMCOT(number)`
pub fn imcot(args: &[Arg]) -> Value {
    ratio_of(args, imcos, imsin)
}

/// `IMSEC(number)` - one over the cosine.
pub fn imsec(args: &[Arg]) -> Value {
    reciprocal_of(args, imcos)
}

/// `IMCSC(number)`
pub fn imcsc(args: &[Arg]) -> Value {
    reciprocal_of(args, imsin)
}

/// `IMSECH(number)`
pub fn imsech(args: &[Arg]) -> Value {
    reciprocal_of(args, imcosh)
}

/// `IMCSCH(number)`
pub fn imcsch(args: &[Arg]) -> Value {
    reciprocal_of(args, imsinh)
}

/// One complex function divided by another.
fn ratio_of(args: &[Arg], top: fn(&[Arg]) -> Value, bottom: fn(&[Arg]) -> Value) -> Value {
    let (numerator, denominator) = (top(args), bottom(args));
    if let Some(e) = numerator.error().or_else(|| denominator.error()) {
        return Value::Error(e);
    }
    imdiv(&[value_arg(numerator), value_arg(denominator)])
}

/// One over a complex function.
fn reciprocal_of(args: &[Arg], bottom: fn(&[Arg]) -> Value) -> Value {
    let denominator = bottom(args);
    if let Some(e) = denominator.error() {
        return Value::Error(e);
    }
    imdiv(&[value_arg(Value::Number(1.0)), value_arg(denominator)])
}

/// A value as an argument, for the functions built out of other ones.
fn value_arg(value: Value) -> Arg {
    Arg {
        value,
        reference: false,
    }
}

/// Applies a body to one complex argument.
fn one_complex(args: &[Arg], body: impl Fn(Complex) -> Value) -> Value {
    let [arg] = args else {
        return Value::Error(CellError::Value);
    };
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    match Complex::parse(&arg.value) {
        Some(z) => body(z),
        None => Value::Error(CellError::Num),
    }
}

/// The same where the answer is another complex number.
fn map_complex(args: &[Arg], body: impl Fn(Complex) -> Option<Complex>) -> Value {
    one_complex(args, |z| {
        body(z).map_or(Value::Error(CellError::Num), Complex::show)
    })
}

/// Applies a body to two complex arguments.
fn two_complex(args: &[Arg], body: impl Fn(Complex, Complex) -> Option<Complex>) -> Value {
    let [a, b] = args else {
        return Value::Error(CellError::Value);
    };
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (Some(a), Some(b)) = (Complex::parse(&a.value), Complex::parse(&b.value)) else {
        return Value::Error(CellError::Num);
    };
    body(a, b).map_or(Value::Error(CellError::Num), Complex::show)
}

/// Folds a list of complex arguments together.
fn fold_complex(args: &[Arg], body: impl Fn(Complex, Complex) -> Complex) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let mut total: Option<Complex> = None;
    for arg in args {
        let mut flat = Vec::new();
        arg.value.flatten(&mut flat);
        for value in flat {
            let Some(z) = Complex::parse(value) else {
                return Value::Error(CellError::Num);
            };
            total = Some(match total {
                Some(current) => body(current, z),
                None => z,
            });
        }
    }
    total.map_or(Value::Error(CellError::Value), Complex::show)
}

/// `BESSELJ(x, n)` - the Bessel function of the first kind, order `n`.
///
/// By its power series, which converges everywhere and needs no table of
/// fitted constants.
pub fn besselj(args: &[Arg]) -> Value {
    bessel(args, |x, n| Some(series(x, n, -1.0)))
}

/// `BESSELI(x, n)` - the modified first kind, the same series without the
/// alternating sign.
pub fn besseli(args: &[Arg]) -> Value {
    bessel(args, |x, n| Some(series(x, n, 1.0)))
}

/// `BESSELY(x, n)` - the second kind, defined only for a positive argument.
///
/// Computed from its series rather than from a rational approximation: a page
/// of fitted constants is exactly where a typo produces a plausible wrong
/// answer with nothing to check it against. The orders above one come from the
/// recurrence, which is stable upwards for this function.
pub fn bessely(args: &[Arg]) -> Value {
    bessel(args, |x, n| {
        if x <= 0.0 {
            return None;
        }
        if x >= Y_ASYMPTOTIC_FROM {
            return Some(y_asymptotic(x, n));
        }
        let (mut previous, mut current) = (y0(x), y1(x));
        for k in 1..n {
            let next = 2.0 * f64::from(k) / x * current - previous;
            previous = current;
            current = next;
        }
        Some(if n == 0 { previous } else { current })
    })
}

/// `BESSELK(x, n)` - the modified second kind, likewise positive-only.
pub fn besselk(args: &[Arg]) -> Value {
    bessel(args, |x, n| {
        if x <= 0.0 {
            return None;
        }
        if x >= K_ASYMPTOTIC_FROM {
            return Some(k_asymptotic(x, n));
        }
        let (mut previous, mut current) = (k0(x), k1(x));
        for k in 1..n {
            let next = 2.0 * f64::from(k) / x * current + previous;
            previous = current;
            current = next;
        }
        Some(if n == 0 { previous } else { current })
    })
}

/// Where each power series stops being worth summing and the asymptotic
/// expansion takes over.
///
/// Both series subtract two large quantities to leave a small one, and each
/// loses precision at its own rate: `K0` at `x = 12` cancels seven orders of
/// magnitude, `Y0` still holds there and only gives way around eighteen. The
/// thresholds are where the two methods were measured to agree best - past
/// them the series is the worse of the two, before them the expansion is.
const Y_ASYMPTOTIC_FROM: f64 = 18.0;

/// The same for the modified second kind, which cancels sooner.
const K_ASYMPTOTIC_FROM: f64 = 11.0;

/// The Hankel expansion's terms for order `n` at `x`, as far as they keep
/// shrinking: an asymptotic series is only worth summing while it does.
///
/// `t[k]` is the k-th term of the standard expansion in `1/(8x)`, whose
/// numerators run over `4n² - (2j-1)²`.
fn hankel_terms(x: f64, n: u32) -> Vec<f64> {
    let mu = 4.0 * f64::from(n) * f64::from(n);
    let mut terms: Vec<f64> = vec![1.0];
    let mut term = 1.0;
    for k in 1..30u32 {
        let j = f64::from(2 * k - 1);
        term *= (mu - j * j) / (f64::from(k) * 8.0 * x);
        // The expansion diverges eventually; stop where it turns.
        if term.abs() > terms[terms.len() - 1].abs() {
            break;
        }
        terms.push(term);
        if term.abs() < 1.0e-18 {
            break;
        }
    }
    terms
}

/// `Yn(x)` for a large argument, from the Hankel expansion.
fn y_asymptotic(x: f64, n: u32) -> f64 {
    let terms = hankel_terms(x, n);
    // The even terms carry the sine, the odd ones the cosine.
    let (mut p, mut q) = (0.0, 0.0);
    for (k, term) in terms.iter().enumerate() {
        let sign = if (k / 2).is_multiple_of(2) { 1.0 } else { -1.0 };
        if k.is_multiple_of(2) {
            p += sign * term;
        } else {
            q += sign * term;
        }
    }
    let chi = x - (f64::from(n) / 2.0 + 0.25) * std::f64::consts::PI;
    (2.0 / (std::f64::consts::PI * x)).sqrt() * (p * chi.sin() + q * chi.cos())
}

/// `Kn(x)` for a large argument: the same expansion with every sign positive,
/// which is what makes it exponentially small rather than oscillating.
fn k_asymptotic(x: f64, n: u32) -> f64 {
    let total: f64 = hankel_terms(x, n).iter().sum();
    (std::f64::consts::PI / (2.0 * x)).sqrt() * (-x).exp() * total
}

/// Euler's constant, which every second-kind series carries.
const EULER: f64 = 0.577_215_664_901_532_9;

/// The harmonic numbers `1 + 1/2 + ... + 1/k`, which is the part of these
/// series that the first kinds do not have.
fn harmonic(k: u32) -> f64 {
    (1..=k).map(|i| 1.0 / f64::from(i)).sum()
}

/// `Y0(x)` from its series: the logarithmic term times `J0`, plus a sum whose
/// coefficients are the harmonic numbers.
fn y0(x: f64) -> f64 {
    let half = x / 2.0;
    let square = half * half;
    let mut sum = 0.0;
    let mut term = 1.0;
    for k in 1..200u32 {
        term *= -square / (f64::from(k) * f64::from(k));
        let contribution = term * harmonic(k);
        sum += contribution;
        if contribution.abs() < sum.abs() * 1.0e-17 && k > 3 {
            break;
        }
    }
    2.0 / std::f64::consts::PI * ((half.ln() + EULER) * series(x, 0, -1.0) - sum)
}

/// `Y1(x)`, the same shape with the extra `-1/x` the order brings.
fn y1(x: f64) -> f64 {
    let half = x / 2.0;
    let square = half * half;
    // The sum runs over k >= 0 with coefficients H(k) + H(k+1).
    let mut sum = 0.0;
    let mut term = 1.0;
    for k in 0..200u32 {
        if k > 0 {
            term *= -square / (f64::from(k) * f64::from(k + 1));
        }
        let contribution = term * (harmonic(k) + harmonic(k + 1));
        sum += contribution;
        if contribution.abs() < sum.abs() * 1.0e-17 && k > 3 {
            break;
        }
    }
    2.0 / std::f64::consts::PI
        * ((half.ln() + EULER) * series(x, 1, -1.0) - 1.0 / x - half * sum / 2.0)
}

/// `K0(x)`: the modified second kind is the same series without the
/// alternating sign, and with the logarithm entering the other way round.
fn k0(x: f64) -> f64 {
    let half = x / 2.0;
    let square = half * half;
    let mut sum = 0.0;
    let mut term = 1.0;
    for k in 1..200u32 {
        term *= square / (f64::from(k) * f64::from(k));
        let contribution = term * harmonic(k);
        sum += contribution;
        if contribution.abs() < sum.abs() * 1.0e-17 && k > 3 {
            break;
        }
    }
    -(half.ln() + EULER) * series(x, 0, 1.0) + sum
}

/// `K1(x)`.
fn k1(x: f64) -> f64 {
    let half = x / 2.0;
    let square = half * half;
    let mut sum = 0.0;
    let mut term = 1.0;
    for k in 0..200u32 {
        if k > 0 {
            term *= square / (f64::from(k) * f64::from(k + 1));
        }
        let contribution = term * (harmonic(k) + harmonic(k + 1));
        sum += contribution;
        if contribution.abs() < sum.abs() * 1.0e-17 && k > 3 {
            break;
        }
    }
    (half.ln() + EULER) * series(x, 1, 1.0) + 1.0 / x - half * sum / 2.0
}

/// Shared body of the two: a value and a whole non-negative order.
fn bessel(args: &[Arg], body: impl Fn(f64, u32) -> Option<f64>) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let [x, order] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(x), Ok(order)) = (x.number(), order.number()) else {
        return Value::Error(CellError::Value);
    };
    let order = order.trunc();
    if !(0.0..=1000.0).contains(&order) {
        return Value::Error(CellError::Num);
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bounded to 0..=1000 on the line above"
    )]
    let order = order as u32;
    body(x, order).map_or(Value::Error(CellError::Num), |result| {
        if result.is_finite() {
            Value::Number(result)
        } else {
            Value::Error(CellError::Num)
        }
    })
}

/// The series both first kinds share; `sign` is what alternates one of them.
fn series(x: f64, n: u32, sign: f64) -> f64 {
    use crate::shared::special::ln_gamma;

    let half = x / 2.0;
    let mut total = 0.0;
    let mut term = 1.0;
    for k in 0..200u32 {
        if k > 0 {
            // Each term is the last times x^2/4 over k(k+n), which keeps the
            // factorials out of it.
            term *= sign * half * half / (f64::from(k) * f64::from(k + n));
        }
        total += term;
        if term.abs() < total.abs() * 1.0e-16 && k > 2 {
            break;
        }
    }
    // The leading (x/2)^n / n! is added in logarithms so a large order does
    // not overflow on its way to a small answer.
    let scale = f64::from(n) * half.abs().ln() - ln_gamma(f64::from(n) + 1.0);
    let leading = scale.exp() * if half < 0.0 && n % 2 == 1 { -1.0 } else { 1.0 };
    leading * total
}

/// `CONVERT(number, from, to)` - between units of the same kind.
///
/// The units Excel lists that a spreadsheet actually sees; the exotic tail of
/// its table (light years, parsecs, the several kinds of barrel) is not here.
pub fn convert(args: &[Arg]) -> Value {
    /// Each unit as its kind and how many of the base unit it is.
    const UNITS: [(&str, &str, f64); 34] = [
        // Mass, in grams.
        ("g", "mass", 1.0),
        ("kg", "mass", 1000.0),
        ("mg", "mass", 0.001),
        ("lbm", "mass", 453.592_37),
        ("ozm", "mass", 28.349_523_125),
        ("u", "mass", 1.660_539_066_6e-24),
        // Distance, in metres.
        ("m", "distance", 1.0),
        ("km", "distance", 1000.0),
        ("cm", "distance", 0.01),
        ("mm", "distance", 0.001),
        ("mi", "distance", 1609.344),
        ("in", "distance", 0.0254),
        ("ft", "distance", 0.3048),
        ("yd", "distance", 0.9144),
        ("ang", "distance", 1.0e-10),
        ("Nmi", "distance", 1852.0),
        // Time, in seconds.
        ("sec", "time", 1.0),
        ("s", "time", 1.0),
        ("mn", "time", 60.0),
        ("min", "time", 60.0),
        ("hr", "time", 3600.0),
        ("day", "time", 86_400.0),
        ("yr", "time", 31_557_600.0),
        // Pressure, in pascals.
        ("Pa", "pressure", 1.0),
        ("atm", "pressure", 101_325.0),
        ("mmHg", "pressure", 133.322_387_415),
        // Energy, in joules.
        ("J", "energy", 1.0),
        ("cal", "energy", 4.186_8),
        ("eV", "energy", 1.602_176_634e-19),
        ("Wh", "energy", 3600.0),
        // Power, in watts.
        ("W", "power", 1.0),
        ("HP", "power", 745.699_871_582_27),
        // Area, in square metres.
        ("m2", "area", 1.0),
        ("ft2", "area", 0.092_903_04),
    ];
    /// Temperature is the odd one: a scale and an offset, not a scale alone.
    /// Each is the pair that turns it into kelvin.
    const TEMPERATURES: [(&str, f64, f64); 5] = [
        ("K", 1.0, 0.0),
        ("kel", 1.0, 0.0),
        ("C", 1.0, 273.15),
        ("cel", 1.0, 273.15),
        ("F", 5.0 / 9.0, 273.15 - 32.0 * 5.0 / 9.0),
    ];

    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let [number, from, to] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(number), Ok(from), Ok(to)) = (number.number(), from.text(), to.text()) else {
        return Value::Error(CellError::Value);
    };
    // Temperature first, since it needs the offset the table above has no
    // room for.
    let temperature = |name: &str| {
        TEMPERATURES
            .iter()
            .find(|(unit, _, _)| *unit == name)
            .map(|(_, scale, offset)| (*scale, *offset))
    };
    if let (Some((from_scale, from_offset)), Some((to_scale, to_offset))) =
        (temperature(&from), temperature(&to))
    {
        let kelvin = number * from_scale + from_offset;
        return Value::Number((kelvin - to_offset) / to_scale);
    }
    if temperature(&from).is_some() || temperature(&to).is_some() {
        return Value::Error(CellError::Na);
    }
    let find = |name: &str| UNITS.iter().find(|(unit, _, _)| *unit == name);
    let (Some((_, from_kind, from_scale)), Some((_, to_kind, to_scale))) = (find(&from), find(&to))
    else {
        return Value::Error(CellError::Na);
    };
    // Converting between kinds is not a conversion at all.
    if from_kind != to_kind {
        return Value::Error(CellError::Na);
    }
    Value::Number(number * from_scale / to_scale)
}
