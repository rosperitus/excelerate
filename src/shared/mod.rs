//! Shared helper subsystems.

pub mod biff_functions;
pub mod codepage;
pub mod date;
pub mod date_parse;
pub mod odf_formula;
pub mod palette;
pub mod special;

/// Seconds since the Unix epoch, as the clock of whatever platform this runs
/// on reports them.
///
/// `SystemTime::now` panics on `wasm32-unknown-unknown` - that target has no
/// clock of its own - so there the host's `Date.now()` answers instead. Every
/// caller (`TODAY`, `NOW`, `RAND`, a date written without a year) only needs
/// the wall clock, not monotonicity.
#[must_use]
pub fn unix_seconds() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() / 1000.0
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64())
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
