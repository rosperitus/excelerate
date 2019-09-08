//! Shared helper subsystems.

pub mod date;
pub mod date_parse;
// pub mod odf_formula;
// pub mod special;

/// Seconds since the Unix epoch, as the clock of whatever platform this runs
/// on reports them.
///
/// `SystemTime::now` panics on `wasm32-unknown-unknown` — that target has no
/// clock of its own — so there the host's `Date.now()` answers instead. Every
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
