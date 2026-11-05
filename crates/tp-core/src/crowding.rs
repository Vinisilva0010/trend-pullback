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

/// Leverage levels assumed for new positions, with the share of money at each.
pub const LEVERAGE_MIX: [(f64, f64); 3] = [(10.0, 0.40), (25.0, 0.35), (50.0, 0.25)];
/// Width of one price bin for the liquidation map, as a share of the price.
pub const ZONE_BIN: f64 = 0.005;
/// Bins joined into one zone.
pub const ZONE_BINS: usize = 3;
/// How far from the price (share) zones are searched.
pub const ZONE_RANGE: f64 = 0.20;

/// One hour of futures data for the liquidation estimate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OiHour {
    /// Hourly high, low and close of the price.
    pub high: f64,
    pub low: f64,
    pub close: f64,
    /// Open interest in USD at the end of the hour.
    pub oi_usd: f64,
    /// Share of accounts that are long (0 to 1).
    pub long_share: f64,
}

/// A price range where many estimated liquidations sit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zone {
    pub low: f64,
    pub high: f64,
    /// Estimated position value (USD) liquidated inside the range.
    pub usd: f64,
}

/// Estimated liquidation zones below the price (longs) and above it (shorts).
///
/// Model: when open interest grows during an hour, the new money is split
/// into longs and shorts by that hour's long share and opened at the hour's
/// close with the [`LEVERAGE_MIX`]; each piece gets the price at which that
/// leverage is wiped out. When open interest falls, every open piece shrinks
/// in proportion. A piece disappears once a later hour trades through its
/// liquidation price. What is left is grouped into price bins, and the
/// heaviest run of [`ZONE_BINS`] bins on each side within [`ZONE_RANGE`] of
/// the price is returned. `scale` multiplies all amounts (for example to go
/// from one exchange to the whole market). This is an estimate: exchanges do
/// not publish real positions.
#[must_use]
pub fn liquidation_zones(hours: &[OiHour], scale: f64) -> (Option<Zone>, Option<Zone>) {
    // (liquidation price, usd, is_long)
    let mut pieces: Vec<(f64, f64, bool)> = Vec::new();
    for w in hours.windows(2) {
        let (prev, h) = (w[0], w[1]);
        pieces.retain(|(liq, _, long)| if *long { h.low > *liq } else { h.high < *liq });
        let change = h.oi_usd - prev.oi_usd;
        if change > 0.0 {
            for (lev, share) in LEVERAGE_MIX {
                let money = change * share;
                pieces.push((h.close * (1.0 - 1.0 / lev), money * h.long_share, true));
                pieces.push((
                    h.close * (1.0 + 1.0 / lev),
                    money * (1.0 - h.long_share),
                    false,
                ));
            }
        } else if prev.oi_usd > 0.0 {
            let keep = (h.oi_usd / prev.oi_usd).max(0.0);
            for p in &mut pieces {
                p.1 *= keep;
            }
        }
    }
    let Some(price) = hours.last().map(|h| h.close).filter(|p| *p > 0.0) else {
        return (None, None);
    };
    let bin = price * ZONE_BIN;
    let side = |long: bool| -> Option<Zone> {
        let mut bins: std::collections::BTreeMap<i64, f64> = std::collections::BTreeMap::new();
        for (liq, usd, is_long) in &pieces {
            let inside = if long {
                *liq < price && *liq >= price * (1.0 - ZONE_RANGE)
            } else {
                *liq > price && *liq <= price * (1.0 + ZONE_RANGE)
            };
            if *is_long == long && inside {
                let k = ((liq - price) / bin).floor();
                // Bins are small integers: the range is limited to ±20%.
                #[allow(clippy::cast_possible_truncation)]
                let k = k as i64;
                *bins.entry(k).or_insert(0.0) += usd * scale;
            }
        }
        let keys: Vec<i64> = bins.keys().copied().collect();
        let width = i64::try_from(ZONE_BINS).unwrap_or(3);
        keys.iter()
            .map(|&k| {
                let usd: f64 = (k..k + width)
                    .map(|j| bins.get(&j).copied().unwrap_or(0.0))
                    .sum();
                (k, usd)
            })
            .filter(|(_, usd)| *usd > 0.0)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(k, usd)| {
                #[allow(clippy::cast_precision_loss)]
                let low = price + (k as f64) * bin;
                Zone {
                    low,
                    high: low + bin * crate::indicators::as_f64(ZONE_BINS),
                    usd,
                }
            })
    };
    (side(true), side(false))
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

    fn hour(close: f64, oi: f64, long_share: f64) -> OiHour {
        OiHour {
            high: close * 1.001,
            low: close * 0.999,
            close,
            oi_usd: oi,
            long_share,
        }
    }

    #[test]
    fn new_money_maps_to_liquidation_prices_by_leverage() {
        let hours = [hour(100.0, 1000.0, 0.5), hour(100.0, 2000.0, 0.75)];
        let (longs, shorts) = liquidation_zones(&hours, 1.0);
        let longs = longs.unwrap();
        let shorts = shorts.unwrap();
        // 1000 new: 750 long and 250 short; 10x liquidates longs at 90 with 300 usd,
        // the heaviest run below; shorts at 110 with 100 usd.
        assert!(longs.low <= 90.0 && longs.high > 90.0, "{longs:?}");
        assert!((longs.usd - 300.0).abs() < 1e-9);
        assert!(shorts.low <= 110.0 && shorts.high > 110.0, "{shorts:?}");
        assert!((shorts.usd - 100.0).abs() < 1e-9);
    }

    #[test]
    fn falling_open_interest_shrinks_and_price_moves_wipe_out() {
        let mut hours = vec![hour(100.0, 1000.0, 0.5), hour(100.0, 2000.0, 0.5)];
        hours.push(hour(100.0, 1000.0, 0.5));
        let (longs, _) = liquidation_zones(&hours, 2.0);
        // Half was closed, then everything doubled by the scale: 10x longs keep 200.
        assert!((longs.unwrap().usd - 200.0).abs() < 1e-9);
        // A drop to 95 wipes out the 25x and 50x longs (96 and 98) but not 10x (90).
        hours.push(OiHour {
            high: 100.0,
            low: 95.0,
            close: 99.0,
            oi_usd: 1000.0,
            long_share: 0.5,
        });
        let (longs, _) = liquidation_zones(&hours, 1.0);
        let z = longs.unwrap();
        assert!(z.low <= 90.0 && z.high > 90.0, "{z:?}");
    }
}
