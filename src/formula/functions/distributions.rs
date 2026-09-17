//! The statistical distributions.
//!
//! Each one is a thin shell over [`crate::shared::special`]: the normal is the
//! error function, the chi-squared and the Poisson are the incomplete gamma,
//! and the t, the F and the binomial are the incomplete beta. What is left here
//! is Excel's own part - which argument order it uses, where it wants the
//! density rather than the cumulative total, and which combinations it refuses.
//!
//! Every function has a `cumulative` flag or a fixed choice, and the `.RT`
//! forms are the right tail: `1 - cumulative`, computed directly where the
//! subtraction would lose the answer.

use super::{Arg, first_error};
use crate::error::CellError;
use crate::formula::value::Value;
use crate::shared::special::{beta_i, erf, gamma, gamma_p, gamma_q, invert, ln_gamma};

/// The largest argument the inverses search up to.
///
/// Past this the distributions are flat to the last bit `f64` has, so a bound
/// costs nothing and stops a search that would otherwise never narrow.
const SEARCH_LIMIT: f64 = 1.0e10;

/// `NORM.DIST(x, mean, deviation, cumulative)`, and `NORMDIST`.
pub fn norm_dist(args: &[Arg]) -> Value {
    let Some([x, mean, deviation, cumulative]) = four(args) else {
        return Value::Error(CellError::Value);
    };
    if deviation <= 0.0 {
        return Value::Error(CellError::Num);
    }
    let z = (x - mean) / deviation;
    Value::Number(if is_set(cumulative) {
        normal_cdf(z)
    } else {
        (-z * z / 2.0).exp() / (deviation * (2.0 * core::f64::consts::PI).sqrt())
    })
}

/// `NORM.S.DIST(z, [cumulative])`, and `NORMSDIST`, which is always cumulative.
pub fn norm_s_dist(args: &[Arg]) -> Value {
    let (z, cumulative) = match args {
        [z] => (z.number(), Ok(1.0)),
        [z, c] => (z.number(), c.number()),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(z), Ok(cumulative)) = (z, cumulative) else {
        return read_error(args);
    };
    Value::Number(if is_set(cumulative) {
        normal_cdf(z)
    } else {
        (-z * z / 2.0).exp() / (2.0 * core::f64::consts::PI).sqrt()
    })
}

/// `NORM.INV(probability, mean, deviation)`, and `NORMINV`.
pub fn norm_inv(args: &[Arg]) -> Value {
    let Some([p, mean, deviation]) = three(args) else {
        return Value::Error(CellError::Value);
    };
    if deviation <= 0.0 || !(0.0..1.0).contains(&p) || p == 0.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number(mean + deviation * standard_inverse(p))
}

/// `NORM.S.INV(probability)`, and `NORMSINV`.
pub fn norm_s_inv(args: &[Arg]) -> Value {
    super::one(args, |p| {
        if p <= 0.0 || p >= 1.0 {
            return Value::Error(CellError::Num);
        }
        Value::Number(standard_inverse(p))
    })
}

/// `GAUSS(z)` - the area between the mean and z, so half a normal less.
pub fn gauss(args: &[Arg]) -> Value {
    super::one(args, |z| Value::Number(normal_cdf(z) - 0.5))
}

/// `CONFIDENCE(alpha, deviation, size)`, and `CONFIDENCE.NORM`.
pub fn confidence(args: &[Arg]) -> Value {
    let Some([alpha, deviation, size]) = three(args) else {
        return Value::Error(CellError::Value);
    };
    if !(0.0..1.0).contains(&alpha) || alpha == 0.0 || deviation <= 0.0 || size < 1.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number(-standard_inverse(alpha / 2.0) * deviation / size.trunc().sqrt())
}

/// `LOGNORM.DIST(x, mean, deviation, [cumulative])`, and `LOGNORMDIST`.
pub fn lognorm_dist(args: &[Arg]) -> Value {
    let (x, mean, deviation, cumulative) = match args {
        [x, m, d] => (x.number(), m.number(), d.number(), Ok(1.0)),
        [x, m, d, c] => (x.number(), m.number(), d.number(), c.number()),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(x), Ok(mean), Ok(deviation), Ok(cumulative)) = (x, mean, deviation, cumulative) else {
        return read_error(args);
    };
    if x <= 0.0 || deviation <= 0.0 {
        return Value::Error(CellError::Num);
    }
    let z = (x.ln() - mean) / deviation;
    Value::Number(if is_set(cumulative) {
        normal_cdf(z)
    } else {
        (-z * z / 2.0).exp() / (x * deviation * (2.0 * core::f64::consts::PI).sqrt())
    })
}

/// `LOGNORM.INV(probability, mean, deviation)`, and `LOGINV`.
pub fn lognorm_inv(args: &[Arg]) -> Value {
    let Some([p, mean, deviation]) = three(args) else {
        return Value::Error(CellError::Value);
    };
    if !(0.0..1.0).contains(&p) || p == 0.0 || deviation <= 0.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number((mean + deviation * standard_inverse(p)).exp())
}

/// `GAMMA(x)` - the gamma function itself, which Excel exposes on its own.
pub fn gamma_fn(args: &[Arg]) -> Value {
    super::one(args, |x| {
        // The poles: gamma is undefined at zero and the negative whole numbers.
        if x <= 0.0 && x.fract() == 0.0 {
            return Value::Error(CellError::Num);
        }
        let result = gamma(x);
        if result.is_finite() {
            Value::Number(result)
        } else {
            Value::Error(CellError::Num)
        }
    })
}

/// `GAMMALN(x)`, and `GAMMALN.PRECISE`.
pub fn gammaln(args: &[Arg]) -> Value {
    super::one(args, |x| {
        if x <= 0.0 {
            return Value::Error(CellError::Num);
        }
        Value::Number(ln_gamma(x))
    })
}

/// `GAMMA.DIST(x, alpha, beta, cumulative)`, and `GAMMADIST`.
pub fn gamma_dist(args: &[Arg]) -> Value {
    let Some([x, alpha, beta, cumulative]) = four(args) else {
        return Value::Error(CellError::Value);
    };
    if x < 0.0 || alpha <= 0.0 || beta <= 0.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number(if is_set(cumulative) {
        gamma_p(alpha, x / beta)
    } else {
        (x / beta).powf(alpha - 1.0) * (-x / beta).exp() / (beta * gamma(alpha))
    })
}

/// `GAMMA.INV(probability, alpha, beta)`, and `GAMMAINV`.
pub fn gamma_inv(args: &[Arg]) -> Value {
    let Some([p, alpha, beta]) = three(args) else {
        return Value::Error(CellError::Value);
    };
    if !(0.0..1.0).contains(&p) || alpha <= 0.0 || beta <= 0.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number(beta * invert(p, 0.0, SEARCH_LIMIT, |x| gamma_p(alpha, x)))
}

/// `CHISQ.DIST(x, degrees, cumulative)` - the left tail.
pub fn chisq_dist(args: &[Arg]) -> Value {
    let Some([x, degrees, cumulative]) = three(args) else {
        return Value::Error(CellError::Value);
    };
    let degrees = degrees.trunc();
    if x < 0.0 || degrees < 1.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number(if is_set(cumulative) {
        gamma_p(degrees / 2.0, x / 2.0)
    } else {
        let half = degrees / 2.0;
        (x / 2.0).powf(half - 1.0) * (-x / 2.0).exp() / (2.0 * gamma(half))
    })
}

/// `CHISQ.DIST.RT(x, degrees)`, and `CHIDIST` - the right tail, which is the
/// one the older name always meant.
pub fn chisq_dist_rt(args: &[Arg]) -> Value {
    let [x, degrees] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(x), Ok(degrees)) = (x.number(), degrees.number()) else {
        return read_error(args);
    };
    let degrees = degrees.trunc();
    if x < 0.0 || degrees < 1.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number(gamma_q(degrees / 2.0, x / 2.0))
}

/// `CHISQ.INV(probability, degrees)` - the left tail inverted.
pub fn chisq_inv(args: &[Arg]) -> Value {
    chisq_inverse(args, false)
}

/// `CHISQ.INV.RT(probability, degrees)`, and `CHIINV`.
pub fn chisq_inv_rt(args: &[Arg]) -> Value {
    chisq_inverse(args, true)
}

/// Shared body of the two chi-squared inverses.
fn chisq_inverse(args: &[Arg], right_tail: bool) -> Value {
    let [p, degrees] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(p), Ok(degrees)) = (p.number(), degrees.number()) else {
        return read_error(args);
    };
    let degrees = degrees.trunc();
    if !(0.0..=1.0).contains(&p) || degrees < 1.0 {
        return Value::Error(CellError::Num);
    }
    let wanted = if right_tail { 1.0 - p } else { p };
    Value::Number(2.0 * invert(wanted, 0.0, SEARCH_LIMIT, |x| gamma_p(degrees / 2.0, x)))
}

/// `T.DIST(x, degrees, cumulative)` - the left tail.
pub fn t_dist(args: &[Arg]) -> Value {
    let Some([x, degrees, cumulative]) = three(args) else {
        return Value::Error(CellError::Value);
    };
    let degrees = degrees.trunc();
    if degrees < 1.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number(if is_set(cumulative) {
        student_cdf(x, degrees)
    } else {
        let scale = (-ln_gamma(degrees / 2.0) + ln_gamma(f64::midpoint(degrees, 1.0))).exp()
            / (degrees * core::f64::consts::PI).sqrt();
        scale * (1.0 + x * x / degrees).powf(-f64::midpoint(degrees, 1.0))
    })
}

/// `T.DIST.RT(x, degrees)` - the right tail.
pub fn t_dist_rt(args: &[Arg]) -> Value {
    student_tail(args, 1.0)
}

/// `T.DIST.2T(x, degrees)` - both ends together.
pub fn t_dist_2t(args: &[Arg]) -> Value {
    student_tail(args, 2.0)
}

/// `TDIST(x, degrees, tails)` - the older name, which asks for the number of
/// tails rather than having it in the name.
pub fn tdist(args: &[Arg]) -> Value {
    let Some([x, degrees, tails]) = three(args) else {
        return Value::Error(CellError::Value);
    };
    // One tail or two, and nothing else.
    let tails = tails.trunc();
    if !(1.0..=2.0).contains(&tails) || tails.fract() != 0.0 {
        return Value::Error(CellError::Num);
    }
    let with_two = [
        Arg {
            value: Value::Number(x),
            reference: false,
        },
        Arg {
            value: Value::Number(degrees),
            reference: false,
        },
    ];
    student_tail(&with_two, tails)
}

/// Shared body of the one- and two-tailed t distributions.
fn student_tail(args: &[Arg], tails: f64) -> Value {
    let [x, degrees] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(x), Ok(degrees)) = (x.number(), degrees.number()) else {
        return read_error(args);
    };
    let degrees = degrees.trunc();
    if degrees < 1.0 || x < 0.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number(tails * (1.0 - student_cdf(x, degrees)))
}

/// `T.INV(probability, degrees)` - the left tail inverted.
pub fn t_inv(args: &[Arg]) -> Value {
    t_inverse(args, false)
}

/// `T.INV.2T(probability, degrees)`, and `TINV`: the two-tailed inverse, whose
/// answer is the positive end of the interval.
pub fn t_inv_2t(args: &[Arg]) -> Value {
    t_inverse(args, true)
}

/// Shared body of the two t inverses.
fn t_inverse(args: &[Arg], two_tailed: bool) -> Value {
    let [p, degrees] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(p), Ok(degrees)) = (p.number(), degrees.number()) else {
        return read_error(args);
    };
    let degrees = degrees.trunc();
    if degrees < 1.0 || p <= 0.0 || p > 1.0 {
        return Value::Error(CellError::Num);
    }
    // Two tails share the probability, so the positive end cuts off half of it.
    let wanted = if two_tailed { 1.0 - p / 2.0 } else { p };
    // The whole probability in two tails leaves the middle, which is zero.
    if two_tailed && p >= 1.0 {
        return Value::Number(0.0);
    }
    Value::Number(invert(wanted, -SEARCH_LIMIT, SEARCH_LIMIT, |x| {
        student_cdf(x, degrees)
    }))
}

/// `F.DIST(x, d1, d2, cumulative)` - the left tail.
pub fn f_dist(args: &[Arg]) -> Value {
    let Some([x, d1, d2, cumulative]) = four(args) else {
        return Value::Error(CellError::Value);
    };
    let (d1, d2) = (d1.trunc(), d2.trunc());
    if x < 0.0 || d1 < 1.0 || d2 < 1.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number(if is_set(cumulative) {
        fisher_cdf(x, d1, d2)
    } else {
        let top = (d1 * x).powf(d1) * d2.powf(d2) / (d1 * x + d2).powf(d1 + d2);
        top.sqrt() / (x * beta_function(d1 / 2.0, d2 / 2.0))
    })
}

/// `F.DIST.RT(x, d1, d2)`, and `FDIST` - the right tail.
pub fn f_dist_rt(args: &[Arg]) -> Value {
    let Some([x, d1, d2]) = three(args) else {
        return Value::Error(CellError::Value);
    };
    let (d1, d2) = (d1.trunc(), d2.trunc());
    if x < 0.0 || d1 < 1.0 || d2 < 1.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number(1.0 - fisher_cdf(x, d1, d2))
}

/// `F.INV(probability, d1, d2)` - the left tail inverted.
pub fn f_inv(args: &[Arg]) -> Value {
    f_inverse(args, false)
}

/// `F.INV.RT(probability, d1, d2)`, and `FINV`.
pub fn f_inv_rt(args: &[Arg]) -> Value {
    f_inverse(args, true)
}

/// Shared body of the two F inverses.
fn f_inverse(args: &[Arg], right_tail: bool) -> Value {
    let Some([p, d1, d2]) = three(args) else {
        return Value::Error(CellError::Value);
    };
    let (d1, d2) = (d1.trunc(), d2.trunc());
    if !(0.0..=1.0).contains(&p) || d1 < 1.0 || d2 < 1.0 {
        return Value::Error(CellError::Num);
    }
    let wanted = if right_tail { 1.0 - p } else { p };
    Value::Number(invert(wanted, 0.0, SEARCH_LIMIT, |x| fisher_cdf(x, d1, d2)))
}

/// `BETA.DIST(x, alpha, beta, cumulative, [low], [high])`.
pub fn beta_dist(args: &[Arg]) -> Value {
    beta_shape(args, true)
}

/// `BETADIST(x, alpha, beta, [low], [high])` - the older name, which is always
/// cumulative and so has the bounds where the newer one has its flag.
pub fn betadist(args: &[Arg]) -> Value {
    beta_shape(args, false)
}

/// Shared body of the two beta distributions.
fn beta_shape(args: &[Arg], has_flag: bool) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    // The newer name has a cumulative flag where the older one has its first
    // bound, so the tail of the argument list means different things.
    let (head, tail) = args.split_at(3.min(args.len()));
    let [first, alpha, beta] = head else {
        return Value::Error(CellError::Value);
    };
    let (cumulative, bounds) = match (has_flag, tail) {
        (true, [flag, rest @ ..]) => (flag.number().unwrap_or(1.0), rest),
        _ => (1.0, tail),
    };
    if bounds.len() > 2 {
        return Value::Error(CellError::Value);
    }
    let (Ok(x), Ok(alpha), Ok(beta)) = (first.number(), alpha.number(), beta.number()) else {
        return Value::Error(CellError::Value);
    };
    let (Ok(low), Ok(high)) = (
        bounds.first().map_or(Ok(0.0), Arg::number),
        bounds.get(1).map_or(Ok(1.0), Arg::number),
    ) else {
        return Value::Error(CellError::Value);
    };
    if alpha <= 0.0 || beta <= 0.0 || high <= low || x < low || x > high {
        return Value::Error(CellError::Num);
    }
    let scaled = (x - low) / (high - low);
    Value::Number(if is_set(cumulative) {
        beta_i(alpha, beta, scaled)
    } else {
        scaled.powf(alpha - 1.0) * (1.0 - scaled).powf(beta - 1.0)
            / (beta_function(alpha, beta) * (high - low))
    })
}

/// `BETA.INV(probability, alpha, beta, [low], [high])`, and `BETAINV`.
pub fn beta_inv(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (head, bounds) = args.split_at(3.min(args.len()));
    let ([p, alpha, beta], true) = (head, bounds.len() <= 2) else {
        return Value::Error(CellError::Value);
    };
    let (low, high) = (
        bounds.first().map_or(Ok(0.0), Arg::number),
        bounds.get(1).map_or(Ok(1.0), Arg::number),
    );
    let (Ok(p), Ok(alpha), Ok(beta), Ok(low), Ok(high)) =
        (p.number(), alpha.number(), beta.number(), low, high)
    else {
        return Value::Error(CellError::Value);
    };
    if !(0.0..=1.0).contains(&p) || alpha <= 0.0 || beta <= 0.0 || high <= low {
        return Value::Error(CellError::Num);
    }
    let scaled = invert(p, 0.0, 1.0, |x| beta_i(alpha, beta, x));
    Value::Number(low + scaled * (high - low))
}

/// `EXPON.DIST(x, lambda, cumulative)`, and `EXPONDIST`.
pub fn expon_dist(args: &[Arg]) -> Value {
    let Some([x, lambda, cumulative]) = three(args) else {
        return Value::Error(CellError::Value);
    };
    if x < 0.0 || lambda <= 0.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number(if is_set(cumulative) {
        1.0 - (-lambda * x).exp()
    } else {
        lambda * (-lambda * x).exp()
    })
}

/// `WEIBULL.DIST(x, alpha, beta, cumulative)`, and `WEIBULL`.
pub fn weibull_dist(args: &[Arg]) -> Value {
    let Some([x, alpha, beta, cumulative]) = four(args) else {
        return Value::Error(CellError::Value);
    };
    if x < 0.0 || alpha <= 0.0 || beta <= 0.0 {
        return Value::Error(CellError::Num);
    }
    let scaled = (x / beta).powf(alpha);
    Value::Number(if is_set(cumulative) {
        1.0 - (-scaled).exp()
    } else {
        alpha / beta.powf(alpha) * x.powf(alpha - 1.0) * (-scaled).exp()
    })
}

/// `POISSON.DIST(x, mean, cumulative)`, and `POISSON`.
pub fn poisson_dist(args: &[Arg]) -> Value {
    let Some([x, mean, cumulative]) = three(args) else {
        return Value::Error(CellError::Value);
    };
    let x = x.trunc();
    if x < 0.0 || mean < 0.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number(if is_set(cumulative) {
        // The cumulative Poisson is the incomplete gamma from the other side.
        gamma_q(x + 1.0, mean)
    } else {
        (-mean + x * mean.ln() - ln_gamma(x + 1.0)).exp()
    })
}

/// `BINOM.DIST(successes, trials, probability, cumulative)`, and `BINOMDIST`.
pub fn binom_dist(args: &[Arg]) -> Value {
    let Some([successes, trials, p, cumulative]) = four(args) else {
        return Value::Error(CellError::Value);
    };
    let (successes, trials) = (successes.trunc(), trials.trunc());
    if successes < 0.0 || successes > trials || !(0.0..=1.0).contains(&p) {
        return Value::Error(CellError::Num);
    }
    Value::Number(if is_set(cumulative) {
        if successes >= trials {
            1.0
        } else {
            beta_i(trials - successes, successes + 1.0, 1.0 - p)
        }
    } else {
        binomial_mass(successes, trials, p)
    })
}

/// `NEGBINOM.DIST(failures, successes, probability, [cumulative])`, and
/// `NEGBINOMDIST`, which has no cumulative form.
pub fn negbinom_dist(args: &[Arg]) -> Value {
    let (failures, successes, p, cumulative) = match args {
        [f, s, p] => (f.number(), s.number(), p.number(), Ok(0.0)),
        [f, s, p, c] => (f.number(), s.number(), p.number(), c.number()),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(failures), Ok(successes), Ok(p), Ok(cumulative)) = (failures, successes, p, cumulative)
    else {
        return read_error(args);
    };
    let (failures, successes) = (failures.trunc(), successes.trunc());
    if failures < 0.0 || successes < 1.0 || !(0.0..=1.0).contains(&p) {
        return Value::Error(CellError::Num);
    }
    if is_set(cumulative) {
        return Value::Number(beta_i(successes, failures + 1.0, p));
    }
    let log = ln_gamma(failures + successes) - ln_gamma(successes) - ln_gamma(failures + 1.0)
        + successes * p.ln()
        + failures * (1.0 - p).ln();
    Value::Number(log.exp())
}

/// `HYPGEOM.DIST(x, draws, successes, population, [cumulative])`, and
/// `HYPGEOMDIST`.
pub fn hypgeom_dist(args: &[Arg]) -> Value {
    let (x, draws, successes, population, cumulative) = match args {
        [found, sample, marked, total] => (
            found.number(),
            sample.number(),
            marked.number(),
            total.number(),
            Ok(0.0),
        ),
        [found, sample, marked, total, flag] => (
            found.number(),
            sample.number(),
            marked.number(),
            total.number(),
            flag.number(),
        ),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(x), Ok(draws), Ok(successes), Ok(population), Ok(cumulative)) =
        (x, draws, successes, population, cumulative)
    else {
        return read_error(args);
    };
    let (x, draws, successes, population) = (
        x.trunc(),
        draws.trunc(),
        successes.trunc(),
        population.trunc(),
    );
    if x < 0.0
        || x > draws
        || x > successes
        || draws > population
        || successes > population
        || draws < 0.0
        || successes < 0.0
    {
        return Value::Error(CellError::Num);
    }
    let mass = |k: f64| {
        (log_choose(successes, k) + log_choose(population - successes, draws - k)
            - log_choose(population, draws))
        .exp()
    };
    if !is_set(cumulative) {
        return Value::Number(mass(x));
    }
    // ponytail: a term per count up to x, with a ceiling instead of a closed
    // form; a hypergeometric distribution function (a 3F2 series) lifts it
    // if a workbook ever needs counts in the tens of millions.
    if x > 1.0e7 {
        return Value::Error(CellError::Num);
    }
    let mut total = 0.0;
    let mut k = 0.0;
    while k <= x {
        if k <= successes && draws - k <= population - successes {
            total += mass(k);
        }
        k += 1.0;
    }
    Value::Number(total)
}

/// `BINOM.INV(trials, probability, alpha)`, and `CRITBINOM`: the smallest
/// number of successes whose cumulative probability reaches alpha.
pub fn binom_inv(args: &[Arg]) -> Value {
    let Some([trials, p, alpha]) = three(args) else {
        return Value::Error(CellError::Value);
    };
    let trials = trials.trunc();
    if trials < 0.0 || !(0.0..=1.0).contains(&p) || !(0.0..=1.0).contains(&alpha) {
        return Value::Error(CellError::Num);
    }
    if trials > 1000.0 && p > 0.0 && p < 1.0 {
        // A step per success counted is millions of steps for a large number
        // of trials; the distribution function is monotone, so halving the
        // range finds the same smallest count in sixty.
        let cumulative = |k: f64| {
            if k >= trials {
                1.0
            } else {
                beta_i(trials - k, k + 1.0, 1.0 - p)
            }
        };
        let (mut low, mut high) = (0.0_f64, trials);
        while low < high {
            let middle = f64::midpoint(low, high).floor();
            if cumulative(middle) >= alpha {
                high = middle;
            } else {
                low = middle + 1.0;
            }
        }
        return Value::Number(low);
    }
    let mut total = 0.0;
    let mut k = 0.0;
    while k <= trials {
        total += binomial_mass(k, trials, p);
        if total >= alpha {
            return Value::Number(k);
        }
        k += 1.0;
    }
    Value::Number(trials)
}

/// The probability of exactly this many successes.
fn binomial_mass(successes: f64, trials: f64, p: f64) -> f64 {
    // A certainty either way puts all the mass on one outcome.
    if p <= 0.0 {
        return f64::from(u8::from(successes <= 0.0));
    }
    if p >= 1.0 {
        return f64::from(u8::from(successes >= trials));
    }
    (log_choose(trials, successes) + successes * p.ln() + (trials - successes) * (1.0 - p).ln())
        .exp()
}

/// The logarithm of a binomial coefficient, which keeps the big ones finite.
fn log_choose(n: f64, k: f64) -> f64 {
    ln_gamma(n + 1.0) - ln_gamma(k + 1.0) - ln_gamma(n - k + 1.0)
}

/// The beta function, from the gamma function it is a ratio of.
fn beta_function(a: f64, b: f64) -> f64 {
    (ln_gamma(a) + ln_gamma(b) - ln_gamma(a + b)).exp()
}

/// The cumulative standard normal distribution.
fn normal_cdf(z: f64) -> f64 {
    f64::midpoint(1.0, erf(z / core::f64::consts::SQRT_2))
}

/// Its inverse, found by bisection over the range a normal ever reaches.
pub(crate) fn standard_inverse(p: f64) -> f64 {
    // Eight and a half deviations out, a normal is within one part in 1e17 of
    // its limit, which is past what f64 tells apart.
    invert(p, -8.5, 8.5, normal_cdf)
}

/// The cumulative t distribution, from the incomplete beta.
fn student_cdf(x: f64, degrees: f64) -> f64 {
    let tail = 0.5 * beta_i(degrees / 2.0, 0.5, degrees / (degrees + x * x));
    if x > 0.0 { 1.0 - tail } else { tail }
}

/// The cumulative F distribution, likewise.
fn fisher_cdf(x: f64, d1: f64, d2: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    beta_i(d1 / 2.0, d2 / 2.0, d1 * x / (d1 * x + d2))
}

/// Exactly three numeric arguments.
fn three(args: &[Arg]) -> Option<[f64; 3]> {
    numbers(args)
}

/// Exactly four numeric arguments.
fn four(args: &[Arg]) -> Option<[f64; 4]> {
    numbers(args)
}

/// Exactly `N` numeric arguments, or nothing.
fn numbers<const N: usize>(args: &[Arg]) -> Option<[f64; N]> {
    if first_error(args).is_some() || args.len() != N {
        return None;
    }
    let mut out = [0.0f64; N];
    for (slot, arg) in out.iter_mut().zip(args) {
        // A logical argument is a flag here, and Excel takes it as one.
        *slot = match arg.value.scalar() {
            Value::Bool(b) => f64::from(u8::from(*b)),
            other => other.number().ok()?,
        };
    }
    Some(out)
}

/// Whether a `cumulative` flag is on. Excel takes anything but zero as yes.
fn is_set(flag: f64) -> bool {
    flag != 0.0
}

/// The error an argument carried, or `#VALUE!` when it merely was not a number.
fn read_error(args: &[Arg]) -> Value {
    first_error(args).map_or(Value::Error(CellError::Value), Value::Error)
}

/// `CHISQ.TEST(observed, expected)`, and `CHITEST` - how unlikely the observed
/// counts are if the expected ones are the truth.
pub fn chisq_test(args: &[Arg]) -> Value {
    let [observed, expected] = args else {
        return Value::Error(CellError::Value);
    };
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (observed, expected) = (grid_of(observed), grid_of(expected));
    if observed.len() != expected.len() || observed.is_empty() {
        return Value::Error(CellError::Na);
    }
    let (rows, columns) = (shape_of(args.first()), shape_of(args.get(1)));
    if rows != columns {
        return Value::Error(CellError::Na);
    }
    let mut statistic = 0.0;
    for (seen, want) in observed.iter().zip(&expected) {
        if *want == 0.0 {
            return Value::Error(CellError::Div0);
        }
        statistic += (seen - want) * (seen - want) / want;
    }
    // A table of r rows and c columns has (r-1)(c-1) degrees of freedom; a
    // single row or column has one less than its length.
    let (r, c) = rows;
    let cells = r.saturating_mul(c);
    let degrees = if r <= 1 || c <= 1 {
        cells.saturating_sub(1)
    } else {
        (r - 1).saturating_mul(c - 1)
    };
    if degrees < 1 {
        return Value::Error(CellError::Na);
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "a table this crate could build is far inside f64"
    )]
    let degrees = degrees as f64;
    Value::Number(gamma_q(degrees / 2.0, statistic / 2.0))
}

/// `F.TEST(array1, array2)`, and `FTEST` - whether two samples have the same
/// spread, as a two-tailed probability.
pub fn f_test(args: &[Arg]) -> Value {
    let [first, second] = args else {
        return Value::Error(CellError::Value);
    };
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (a, b) = (grid_of(first), grid_of(second));
    if a.len() < 2 || b.len() < 2 {
        return Value::Error(CellError::Div0);
    }
    let (va, vb) = (sample_variance(&a), sample_variance(&b));
    if va == 0.0 || vb == 0.0 {
        return Value::Error(CellError::Div0);
    }
    let (d1, d2) = (count_of(&a) - 1.0, count_of(&b) - 1.0);
    let tail = 1.0 - fisher_cdf(va / vb, d1, d2);
    // The larger tail doubled, so the answer is a probability either way round.
    Value::Number(2.0 * tail.min(1.0 - tail))
}

/// `T.TEST(array1, array2, tails, type)`, and `TTEST`.
///
/// Type 1 pairs the samples, 2 assumes they have the same spread, and 3 does
/// not - the Welch form, whose degrees of freedom are not a whole number.
pub fn t_test(args: &[Arg]) -> Value {
    let [first, second, tails, kind] = args else {
        return Value::Error(CellError::Value);
    };
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (Ok(tails), Ok(kind)) = (tails.number(), kind.number()) else {
        return Value::Error(CellError::Value);
    };
    // Both are codes rather than measurements, so they belong in an integer.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "range-checked on the line below before either is used"
    )]
    let (tails, kind) = (tails.trunc(), kind.trunc() as u32);
    if !(1.0..=2.0).contains(&tails) || !(1..=3).contains(&kind) {
        return Value::Error(CellError::Num);
    }
    let (a, b) = (grid_of(first), grid_of(second));
    if a.len() < 2 || b.len() < 2 {
        return Value::Error(CellError::Div0);
    }
    let (na, nb) = (count_of(&a), count_of(&b));
    let (ma, mb) = (a.iter().sum::<f64>() / na, b.iter().sum::<f64>() / nb);
    let (va, vb) = (sample_variance(&a), sample_variance(&b));

    let (statistic, degrees) = if kind == 1 {
        if a.len() != b.len() {
            return Value::Error(CellError::Na);
        }
        // Paired: the test is a one-sample one on the differences.
        let differences: Vec<f64> = a.iter().zip(&b).map(|(x, y)| x - y).collect();
        let mean = differences.iter().sum::<f64>() / na;
        let variance = sample_variance(&differences);
        if variance == 0.0 {
            return Value::Error(CellError::Div0);
        }
        (mean / (variance / na).sqrt(), na - 1.0)
    } else if kind == 2 {
        let pooled = ((na - 1.0) * va + (nb - 1.0) * vb) / (na + nb - 2.0);
        if pooled == 0.0 {
            return Value::Error(CellError::Div0);
        }
        (
            (ma - mb) / (pooled * (1.0 / na + 1.0 / nb)).sqrt(),
            na + nb - 2.0,
        )
    } else {
        let spread = va / na + vb / nb;
        if spread == 0.0 {
            return Value::Error(CellError::Div0);
        }
        // Welch-Satterthwaite: the degrees of freedom are a weighted blend.
        let degrees =
            spread * spread / ((va / na).powi(2) / (na - 1.0) + (vb / nb).powi(2) / (nb - 1.0));
        ((ma - mb) / spread.sqrt(), degrees)
    };
    let tail = 1.0 - student_cdf(statistic.abs(), degrees);
    Value::Number(tails * tail)
}

/// `Z.TEST(array, x, [deviation])`, and `ZTEST` - the one-tailed probability
/// that the sample mean is as far above `x` as it is.
pub fn z_test(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (values, x, deviation) = match args {
        [v, x] => (v, x.number(), None),
        [v, x, d] => (v, x.number(), Some(d.number())),
        _ => return Value::Error(CellError::Value),
    };
    let Ok(x) = x else {
        return Value::Error(CellError::Value);
    };
    let ns = grid_of(values);
    if ns.is_empty() {
        return Value::Error(CellError::Na);
    }
    let count = count_of(&ns);
    let mean = ns.iter().sum::<f64>() / count;
    // Without a known deviation the sample's own is used.
    let deviation = match deviation {
        Some(Ok(d)) => d,
        Some(Err(_)) => return Value::Error(CellError::Value),
        None => sample_variance(&ns).sqrt(),
    };
    if deviation <= 0.0 {
        return Value::Error(CellError::Div0);
    }
    Value::Number(1.0 - normal_cdf((mean - x) / (deviation / count.sqrt())))
}

/// The numbers of an argument, ranges flattened, anything else dropped.
fn grid_of(arg: &Arg) -> Vec<f64> {
    let mut flat = Vec::new();
    arg.value.flatten(&mut flat);
    flat.into_iter()
        .filter_map(|v| match v {
            Value::Number(n) => Some(*n),
            _ => None,
        })
        .collect()
}

/// The rows and columns an argument covers, for the chi-squared table.
fn shape_of(arg: Option<&Arg>) -> (usize, usize) {
    let Some(arg) = arg else {
        return (0, 0);
    };
    match &arg.value {
        Value::Array(rows) => (rows.len(), rows.first().map_or(0, Vec::len)),
        _ => (1, 1),
    }
}

/// The sample variance of a list of numbers.
fn sample_variance(ns: &[f64]) -> f64 {
    let count = count_of(ns);
    if count < 2.0 {
        return 0.0;
    }
    let mean = ns.iter().sum::<f64>() / count;
    ns.iter().map(|n| (n - mean) * (n - mean)).sum::<f64>() / (count - 1.0)
}

/// A count as a float, for the arithmetic the tests do.
#[expect(
    clippy::cast_precision_loss,
    reason = "a count that large cannot be reached"
)]
fn count_of<T>(items: &[T]) -> f64 {
    items.len() as f64
}

/// `CONFIDENCE.T(alpha, deviation, size)` - the same interval as
/// `CONFIDENCE.NORM`, but from the t distribution, which is what a small
/// sample needs.
pub fn confidence_t(args: &[Arg]) -> Value {
    let Some([alpha, deviation, size]) = three(args) else {
        return Value::Error(CellError::Value);
    };
    let size = size.trunc();
    if !(0.0..1.0).contains(&alpha) || alpha == 0.0 || deviation <= 0.0 || size < 1.0 {
        return Value::Error(CellError::Num);
    }
    // One observation leaves no degrees of freedom to estimate from.
    let degrees = size - 1.0;
    if degrees < 1.0 {
        return Value::Error(CellError::Div0);
    }
    let t = invert(1.0 - alpha / 2.0, 0.0, SEARCH_LIMIT, |x| {
        student_cdf(x, degrees)
    });
    Value::Number(t * deviation / size.sqrt())
}

/// `BINOM.DIST.RANGE(trials, probability, low, [high])` - the chance of a
/// number of successes anywhere in a range.
pub fn binom_dist_range(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (trials, p, low, high) = match args {
        [t, p, l] => (t.number(), p.number(), l.number(), None),
        [t, p, l, h] => (t.number(), p.number(), l.number(), Some(h.number())),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(trials), Ok(p), Ok(low)) = (trials, p, low) else {
        return Value::Error(CellError::Value);
    };
    let high = match high {
        Some(Ok(h)) => h.trunc(),
        Some(Err(e)) => return Value::Error(e),
        // With one bound the range is that single count.
        None => low.trunc(),
    };
    let (trials, low) = (trials.trunc(), low.trunc());
    if trials < 0.0
        || !(0.0..=1.0).contains(&p)
        || low < 0.0
        || low > trials
        || high > trials
        || high < low
    {
        return Value::Error(CellError::Num);
    }
    if high - low > 1000.0 && p > 0.0 && p < 1.0 {
        // A wide range is the difference of two cumulative values rather
        // than a sum of millions of terms.
        let cumulative = |k: f64| {
            if k < 0.0 {
                0.0
            } else if k >= trials {
                1.0
            } else {
                beta_i(trials - k, k + 1.0, 1.0 - p)
            }
        };
        return Value::Number(cumulative(high) - cumulative(low - 1.0));
    }
    let mut total = 0.0;
    let mut k = low;
    while k <= high {
        total += binomial_mass(k, trials, p);
        k += 1.0;
    }
    Value::Number(total)
}
