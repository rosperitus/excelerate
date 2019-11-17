//! The special functions the statistical distributions are built from.
//!
//! Every distribution Excel has reduces to one of four things: the logarithm of
//! the gamma function, the regularised incomplete gamma function `P(a,x)` and
//! its complement `Q(a,x)`, and the regularised incomplete beta function
//! `I_x(a,b)`. The error function is `P(½, x²)`, so it comes free with them.
//!
//! These are the textbook algorithms — Lanczos for the gamma, a series where it
//! converges and a continued fraction where the series does not — because that
//! is what every other implementation uses and what the reference values were
//! computed with. A crate would have done, but the ones offering these bring
//! linear algebra and random number generators along with them, which is a
//! steep price for four functions.
//!
//! Inverses are found by bisection. It is slower than Newton's method and it
//! cannot diverge, which matters more here: these are called with whatever
//! numbers a spreadsheet holds.

/// How close an inverse has to get before it stops.
const PRECISION: f64 = 1.0e-12;

/// How many halvings the inverses take at most; 200 is far past what `f64`
/// can distinguish, so the loop ends on precision rather than on this.
const MAX_STEPS: usize = 200;

/// The natural logarithm of the gamma function, by the Lanczos approximation.
///
/// The logarithm rather than the function itself: `gamma(171)` is already near
/// the top of `f64`, while its logarithm stays small for anything a
/// distribution asks about.
#[must_use]
pub fn ln_gamma(x: f64) -> f64 {
    /// Lanczos coefficients for g = 7, which is good to about fifteen digits.
    const COEFFICIENTS: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];

    if x < 0.5 {
        // The reflection formula, for the half-line the series does not cover.
        let pi = core::f64::consts::PI;
        return (pi / (pi * x).sin()).ln() - ln_gamma(1.0 - x);
    }
    let x = x - 1.0;
    let mut series = COEFFICIENTS[0];
    for (i, c) in COEFFICIENTS.iter().enumerate().skip(1) {
        #[expect(
            clippy::cast_precision_loss,
            reason = "an index into a nine-element table"
        )]
        let i = i as f64;
        series += c / (x + i);
    }
    let t = x + 7.5;
    (2.0 * core::f64::consts::PI).sqrt().ln() + (x + 0.5) * t.ln() - t + series.ln()
}

/// The gamma function itself, for the arguments where it fits in `f64`.
#[must_use]
pub fn gamma(x: f64) -> f64 {
    if x < 0.5 {
        let pi = core::f64::consts::PI;
        return pi / ((pi * x).sin() * gamma(1.0 - x));
    }
    ln_gamma(x).exp()
}

/// The regularised incomplete gamma function `P(a,x)`.
///
/// This is the cumulative distribution of the gamma family: the chi-squared,
/// the Poisson and the error function are all it under other names.
#[must_use]
pub fn gamma_p(a: f64, x: f64) -> f64 {
    if x < 0.0 || a <= 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return 0.0;
    }
    // The series converges quickly below the peak and slowly above it; the
    // continued fraction is the other way round.
    if x < a + 1.0 {
        series_p(a, x)
    } else {
        1.0 - fraction_q(a, x)
    }
}

/// The complement `Q(a,x) = 1 - P(a,x)`, computed directly.
///
/// Directly, because for a large `x` the complement is the small quantity and
/// subtracting it from one would lose every digit it has.
#[must_use]
pub fn gamma_q(a: f64, x: f64) -> f64 {
    if x < 0.0 || a <= 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return 1.0;
    }
    if x < a + 1.0 {
        1.0 - series_p(a, x)
    } else {
        fraction_q(a, x)
    }
}

/// `P(a,x)` by its series, which is the good one for small `x`.
fn series_p(a: f64, x: f64) -> f64 {
    let mut term = 1.0 / a;
    let mut sum = term;
    let mut n = a;
    for _ in 0..MAX_STEPS {
        n += 1.0;
        term *= x / n;
        sum += term;
        if term.abs() < sum.abs() * PRECISION {
            break;
        }
    }
    sum * (-x + a * x.ln() - ln_gamma(a)).exp()
}

/// `Q(a,x)` by its continued fraction, evaluated with the modified Lentz
/// method — the one that never divides by a zero that a plain evaluation would.
#[expect(
    clippy::many_single_char_names,
    reason = "b, c, d and h are the names the modified Lentz method is \
              published under; renaming them would only make this harder to \
              check against the source it came from"
)]
fn fraction_q(a: f64, x: f64) -> f64 {
    /// Smallest number the method lets a denominator be.
    const TINY: f64 = 1.0e-300;

    let mut b = x + 1.0 - a;
    let mut c = 1.0 / TINY;
    let mut d = 1.0 / b;
    let mut h = d;
    for i in 1..MAX_STEPS {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a step count bounded by MAX_STEPS"
        )]
        let i = i as f64;
        let an = -i * (i - a);
        b += 2.0;
        d = an * d + b;
        if d.abs() < TINY {
            d = TINY;
        }
        c = b + an / c;
        if c.abs() < TINY {
            c = TINY;
        }
        d = 1.0 / d;
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() < PRECISION {
            break;
        }
    }
    h * (-x + a * x.ln() - ln_gamma(a)).exp()
}

/// The error function.
#[must_use]
pub fn erf(x: f64) -> f64 {
    if x < 0.0 {
        return -erf(-x);
    }
    gamma_p(0.5, x * x)
}

/// The complementary error function, `1 - erf(x)`.
#[must_use]
pub fn erfc(x: f64) -> f64 {
    if x < 0.0 {
        return 1.0 + erf(-x);
    }
    gamma_q(0.5, x * x)
}

/// The regularised incomplete beta function `I_x(a,b)`.
///
/// The cumulative distribution of the beta family, which the t, the F and the
/// binomial all reduce to.
#[must_use]
pub fn beta_i(a: f64, b: f64, x: f64) -> f64 {
    if a <= 0.0 || b <= 0.0 || !(0.0..=1.0).contains(&x) {
        return f64::NAN;
    }
    // At either end the answer is the end itself.
    if x <= 0.0 || x >= 1.0 {
        return x;
    }
    let front =
        (ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln()).exp();
    // The fraction converges for x below the mean; above it, the symmetry
    // `I_x(a,b) = 1 - I_(1-x)(b,a)` puts the argument back in that half.
    if x < (a + 1.0) / (a + b + 2.0) {
        front * beta_fraction(a, b, x) / a
    } else {
        1.0 - front * beta_fraction(b, a, 1.0 - x) / b
    }
}

/// The continued fraction of the incomplete beta, again by modified Lentz.
#[expect(
    clippy::many_single_char_names,
    reason = "b, c, d and h are the names the modified Lentz method is \
              published under; renaming them would only make this harder to \
              check against the source it came from"
)]
fn beta_fraction(a: f64, b: f64, x: f64) -> f64 {
    /// Smallest number the method lets a denominator be.
    const TINY: f64 = 1.0e-300;

    let (sum, plus, minus) = (a + b, a + 1.0, a - 1.0);
    let mut c = 1.0;
    let mut d = 1.0 - sum * x / plus;
    if d.abs() < TINY {
        d = TINY;
    }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..MAX_STEPS {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a step count bounded by MAX_STEPS"
        )]
        let m = m as f64;
        let m2 = 2.0 * m;
        // Each step of the fraction has two halves, the even and the odd.
        for numerator in [
            m * (b - m) * x / ((minus + m2) * (a + m2)),
            -(a + m) * (sum + m) * x / ((a + m2) * (plus + m2)),
        ] {
            d = 1.0 + numerator * d;
            if d.abs() < TINY {
                d = TINY;
            }
            c = 1.0 + numerator / c;
            if c.abs() < TINY {
                c = TINY;
            }
            d = 1.0 / d;
            h *= d * c;
        }
        // The convergence test belongs after the pair, not inside it.
        let delta = d * c;
        if (delta - 1.0).abs() < PRECISION {
            break;
        }
    }
    h
}

/// Finds where a rising function reaches `target`, between `low` and `high`.
///
/// Bisection: it needs only that the function rises, which every cumulative
/// distribution does, and it cannot run away the way Newton's method can when
/// the derivative goes flat in a tail.
#[must_use]
pub fn invert(target: f64, low: f64, high: f64, f: impl Fn(f64) -> f64) -> f64 {
    let (mut low, mut high) = (low, high);
    for _ in 0..MAX_STEPS {
        let middle = f64::midpoint(low, high);
        if f(middle) < target {
            low = middle;
        } else {
            high = middle;
        }
        if (high - low).abs() < PRECISION * middle.abs().max(1.0) {
            break;
        }
    }
    f64::midpoint(low, high)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts two numbers agree to about twelve digits.
    fn close(got: f64, want: f64) {
        assert!(
            (got - want).abs() <= want.abs() * 1e-11 + 1e-13,
            "{got} != {want}"
        );
    }

    #[test]
    fn the_gamma_function_matches_its_known_values() {
        // Gamma of a whole number is a factorial of the one below it.
        close(gamma(1.0), 1.0);
        close(gamma(5.0), 24.0);
        close(gamma(10.0), 362_880.0);
        // Gamma of a half is the square root of pi.
        close(gamma(0.5), core::f64::consts::PI.sqrt());
        close(ln_gamma(1.0), 0.0);
        close(ln_gamma(100.0), 359.134_205_369_575_4);
    }

    #[test]
    fn the_error_function_matches_its_known_values() {
        close(erf(0.0), 0.0);
        close(erf(1.0), 0.842_700_792_949_714_9);
        close(erf(-1.0), -0.842_700_792_949_714_9);
        close(erf(2.0), 0.995_322_265_018_952_7);
        close(erfc(1.0), 0.157_299_207_050_285_13);
        // Far out in the tail, where subtracting from one would lose it all.
        close(erfc(5.0), 1.537_459_794_428_035_6e-12);
    }

    #[test]
    fn the_incomplete_gamma_is_a_distribution() {
        // It rises from nothing to one, and the two halves add up.
        close(gamma_p(1.0, 0.0), 0.0);
        for (a, x) in [(0.5, 0.3), (2.0, 1.0), (5.0, 7.0), (20.0, 25.0)] {
            close(gamma_p(a, x) + gamma_q(a, x), 1.0);
        }
        // For a whole `a` there is a closed form to check against: P(a,x) is
        // one less the first `a` terms of the Poisson sum.
        close(gamma_p(1.0, 1.0), 1.0 - (-1.0f64).exp());
        let x = 2.5f64;
        let poisson = (-x).exp() * (1.0 + x + x * x / 2.0);
        close(gamma_p(3.0, x), 1.0 - poisson);
    }

    #[test]
    fn the_incomplete_beta_is_a_distribution() {
        close(beta_i(2.0, 3.0, 0.0), 0.0);
        close(beta_i(2.0, 3.0, 1.0), 1.0);
        // Symmetry: I_x(a,b) = 1 - I_(1-x)(b,a).
        for (a, b, x) in [(2.0, 3.0, 0.4), (0.5, 0.5, 0.7), (10.0, 1.0, 0.9)] {
            close(beta_i(a, b, x), 1.0 - beta_i(b, a, 1.0 - x));
        }
        // I_x(1,1) is x itself.
        close(beta_i(1.0, 1.0, 0.25), 0.25);
        close(beta_i(2.0, 3.0, 0.5), 0.687_5);
    }

    #[test]
    fn an_inverse_finds_its_way_back() {
        for want in [0.1, 0.5, 0.9, 0.99] {
            let x = invert(want, 0.0, 100.0, |x| gamma_p(3.0, x));
            close(gamma_p(3.0, x), want);
        }
    }
}
