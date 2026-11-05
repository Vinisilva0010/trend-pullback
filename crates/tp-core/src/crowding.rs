//! Funding rate of perpetual futures against its own past year.
//!
//! Funding is what longs pay shorts (or the reverse when negative) to keep the
//! perpetual near spot. A day's funding is ranked against the previous year
//! of days that had funding, using only data available on that day.

use std::collections::BTreeMap;

use chrono::NaiveDate;

use crate::indicators::as_f64;

/// Past funding days a day is ranked against.
pub const FUNDING_WINDOW: usize = 365;
/// Past funding days needed before a rank is given.
pub const FUNDING_MIN_DAYS: usize = 180;
/// Rank at or above which funding counts as extremely high.
pub const HIGH_RANK: f64 = 0.9;
/// Rank at or below which funding counts as extremely low.
pub const LOW_RANK: f64 = 0.1;

/// For each date, the mid-rank of its funding among the previous
/// [`FUNDING_WINDOW`] funding days: the share with lower funding plus half the
/// share with equal funding, so the many days at the base rate land in the
/// middle instead of looking extreme (None without funding that day or with
/// fewer than [`FUNDING_MIN_DAYS`] earlier funding days).
#[must_use]
pub fn funding_ranks(dates: &[NaiveDate], funding: &BTreeMap<NaiveDate, f64>) -> Vec<Option<f64>> {
    let days: Vec<(NaiveDate, f64)> = funding.iter().map(|(d, v)| (*d, *v)).collect();
    dates
        .iter()
        .map(|date| {
            let pos = days.partition_point(|(d, _)| d < date);
            let (d, value) = *days.get(pos)?;
            if d != *date || pos < FUNDING_MIN_DAYS {
                return None;
            }
            let window = &days[pos.saturating_sub(FUNDING_WINDOW)..pos];
            let lower = window.iter().filter(|(_, v)| *v < value).count();
            let equal = window
                .iter()
                .filter(|(_, v)| (*v - value).abs() <= f64::EPSILON)
                .count();
            Some((as_f64(lower) + 0.5 * as_f64(equal)) / as_f64(window.len()))
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::unwrap_used)]
mod tests {
    use super::*;
    use chrono::Days;

    fn day(i: u64) -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 1, 1)
            .unwrap()
            .checked_add_days(Days::new(i))
            .unwrap()
    }

    #[test]
    fn ranks_use_only_earlier_days() {
        let funding: BTreeMap<NaiveDate, f64> = (0..400)
            .map(|i| (day(i), f64::from(u32::try_from(i % 10).unwrap())))
            .collect();
        let dates: Vec<NaiveDate> = (0..401).map(day).collect();
        let ranks = funding_ranks(&dates, &funding);
        assert!(ranks[100].is_none());
        // Day 399 has value 9; days 34 to 398 hold 36 nines and 329 lower values.
        let r = ranks[399].unwrap();
        assert!((r - (329.0 + 18.0) / 365.0).abs() < 1e-12);
        // Day 390 has value 0: nothing lower, 36 equal zeros (days 40 to 390).
        assert!((ranks[390].unwrap() - 18.0 / 365.0).abs() < 1e-12);
        // A constant series ranks every day in the middle.
        let flat: BTreeMap<NaiveDate, f64> = (0..400).map(|i| (day(i), 0.0001)).collect();
        assert!((funding_ranks(&dates, &flat)[399].unwrap() - 0.5).abs() < 1e-12);
        // No funding on day 400.
        assert!(ranks[400].is_none());
    }
}
