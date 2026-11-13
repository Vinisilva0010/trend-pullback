//! The cycle of an asset on daily candles: how deep its falls from a top
//! went, how far the bounces inside a fall rose before it kept falling, and
//! how often a bounce that passed a given share of the fall turned into a
//! rise above the top instead.
//!
//! Tops and bottoms come from a zigzag: a top is confirmed when the price
//! falls `threshold` from it, a bottom when it rises `threshold` from it.

use crate::intraday::Bar;

/// A confirmed turning point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Swing {
    pub index: usize,
    pub price: f64,
    pub top: bool,
}

/// Confirmed turning points and the extreme still forming at the end
/// (`(index, price, is_top)`), if the direction is known.
#[must_use]
pub fn zigzag(bars: &[Bar], threshold: f64) -> (Vec<Swing>, Option<(usize, f64, bool)>) {
    let mut swings = Vec::new();
    let Some(first) = bars.first() else {
        return (swings, None);
    };
    let (mut hi, mut hi_i, mut lo, mut lo_i) = (first.high, 0, first.low, 0);
    let mut rising: Option<bool> = None;
    for (i, b) in bars.iter().enumerate().skip(1) {
        match rising {
            None => {
                if b.high > hi {
                    hi = b.high;
                    hi_i = i;
                }
                if b.low < lo {
                    lo = b.low;
                    lo_i = i;
                }
                if lo_i < hi_i && hi >= lo * (1.0 + threshold) {
                    swings.push(Swing {
                        index: lo_i,
                        price: lo,
                        top: false,
                    });
                    rising = Some(true);
                } else if hi_i < lo_i && lo <= hi * (1.0 - threshold) {
                    swings.push(Swing {
                        index: hi_i,
                        price: hi,
                        top: true,
                    });
                    rising = Some(false);
                }
            }
            Some(true) => {
                if b.high > hi {
                    hi = b.high;
                    hi_i = i;
                } else if b.low <= hi * (1.0 - threshold) {
                    swings.push(Swing {
                        index: hi_i,
                        price: hi,
                        top: true,
                    });
                    rising = Some(false);
                    lo = b.low;
                    lo_i = i;
                }
            }
            Some(false) => {
                if b.low < lo {
                    lo = b.low;
                    lo_i = i;
                } else if b.high >= lo * (1.0 + threshold) {
                    swings.push(Swing {
                        index: lo_i,
                        price: lo,
                        top: false,
                    });
                    rising = Some(true);
                    hi = b.high;
                    hi_i = i;
                }
            }
        }
    }
    let forming = rising.map(|up| {
        if up {
            (hi_i, hi, true)
        } else {
            (lo_i, lo, false)
        }
    });
    (swings, forming)
}

/// Depth of every fall from a confirmed top to the next confirmed bottom.
#[must_use]
pub fn falls(swings: &[Swing]) -> Vec<f64> {
    swings
        .windows(2)
        .filter(|w| w[0].top && !w[1].top)
        .map(|w| 1.0 - w[1].price / w[0].price)
        .collect()
}

/// A bounce from a confirmed bottom after a fall: the highest share of the
/// fall it took back before an outcome, and whether it went above the top
/// (`true`) or below the bottom (`false`) first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounce {
    pub retrace: f64,
    pub turned: bool,
}

/// Bounces after every fall with an outcome. A candle that crosses both
/// the top and the bottom counts as falling.
#[must_use]
pub fn bounces(bars: &[Bar], swings: &[Swing]) -> Vec<Bounce> {
    let mut out = Vec::new();
    for w in swings.windows(2) {
        let (top, bottom) = (w[0], w[1]);
        if !top.top || bottom.top || top.price <= bottom.price {
            continue;
        }
        let span = top.price - bottom.price;
        let mut best: f64 = 0.0;
        for b in &bars[bottom.index + 1..] {
            if b.low < bottom.price {
                out.push(Bounce {
                    retrace: best,
                    turned: false,
                });
                break;
            }
            best = best.max((b.high - bottom.price) / span);
            if b.high > top.price {
                out.push(Bounce {
                    retrace: best,
                    turned: true,
                });
                break;
            }
        }
    }
    out
}

/// The value below which `share` of `values` fall (nearest rank).
#[must_use]
pub fn quantile(values: &[f64], share: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    // Smallest rank whose share of the values reaches `share`.
    let n = v.len();
    let rank = (1..=n)
        .find(|r| {
            f64::from(u32::try_from(*r).unwrap_or(u32::MAX))
                >= share * f64::from(u32::try_from(n).unwrap_or(u32::MAX))
        })
        .unwrap_or(n);
    v.get(rank - 1).copied()
}

/// Where the asset is now in its cycle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Now {
    /// Falling from `top` (confirmed), lowest price so far `low`.
    Falling { top: Swing, low: Swing },
    /// Bouncing from a confirmed `bottom`, still below the `top` of the fall.
    Bouncing {
        top: Swing,
        bottom: Swing,
        high: f64,
    },
    /// Above the last top (or no fall yet): highest price so far `high`.
    Rising { high: Swing },
}

/// The current state from the zigzag.
#[must_use]
pub fn now(swings: &[Swing], forming: Option<(usize, f64, bool)>) -> Option<Now> {
    let (index, price, is_top) = forming?;
    let extreme = Swing {
        index,
        price,
        top: is_top,
    };
    if !is_top {
        let top = *swings.last().filter(|s| s.top)?;
        return Some(Now::Falling { top, low: extreme });
    }
    let bottom = swings.last().filter(|s| !s.top).copied();
    let top = swings
        .len()
        .checked_sub(2)
        .map(|i| swings[i])
        .filter(|s| s.top);
    match (top, bottom) {
        (Some(top), Some(bottom)) if price <= top.price => Some(Now::Bouncing {
            top,
            bottom,
            high: price,
        }),
        _ => Some(Now::Rising { high: extreme }),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    fn bar(high: f64, low: f64) -> Bar {
        Bar {
            open_time_ms: 0,
            open: low,
            high,
            low,
            close: high,
            volume: 1.0,
        }
    }

    /// 100 -> 70 (fall of 30%) -> 85 (half back) -> 60 -> 110 (above the top).
    fn path() -> Vec<Bar> {
        vec![
            bar(100.0, 99.0),
            bar(90.0, 80.0),
            bar(75.0, 70.0),
            bar(85.0, 78.0),
            bar(70.0, 60.0),
            bar(80.0, 66.0),
            bar(110.0, 90.0),
            bar(105.0, 100.0),
        ]
    }

    #[test]
    fn the_zigzag_finds_tops_and_bottoms() {
        let (s, forming) = zigzag(&path(), 0.15);
        let prices: Vec<(f64, bool)> = s.iter().map(|x| (x.price, x.top)).collect();
        assert_eq!(
            prices,
            vec![(100.0, true), (70.0, false), (85.0, true), (60.0, false)]
        );
        assert_eq!(forming, Some((6, 110.0, true)));
        let f = falls(&s);
        assert!((f[0] - 0.3).abs() < 1e-12);
        assert!((f[1] - (1.0 - 60.0 / 85.0)).abs() < 1e-12);
    }

    #[test]
    fn bounces_are_measured_against_their_fall() {
        let bars = path();
        let (s, _) = zigzag(&bars, 0.15);
        let b = bounces(&bars, &s);
        // From 70: back to 85 (half of 100-70), then below 70.
        assert_eq!(
            b[0],
            Bounce {
                retrace: 0.5,
                turned: false
            }
        );
        // From 60: above 85 at 110.
        assert!(b[1].turned);
        assert!(b[1].retrace > 1.0);
    }

    #[test]
    fn the_state_now_follows_the_last_swings() {
        let bars = path();
        let (s, f) = zigzag(&bars[..5], 0.15);
        assert!(
            matches!(now(&s, f), Some(Now::Falling { top, low }) if top.price == 85.0 && low.price == 60.0)
        );
        let (s, f) = zigzag(&bars, 0.15);
        assert!(matches!(now(&s, f), Some(Now::Rising { high }) if high.price == 110.0));
        let (s, f) = zigzag(&bars[..6], 0.15);
        assert!(matches!(now(&s, f), Some(Now::Bouncing { bottom, .. }) if bottom.price == 60.0));
    }

    #[test]
    fn quantiles_use_the_nearest_rank() {
        let v = [0.1, 0.2, 0.3, 0.4];
        assert_eq!(quantile(&v, 0.5), Some(0.2));
        assert_eq!(quantile(&v, 0.75), Some(0.3));
        assert_eq!(quantile(&v, 0.9), Some(0.4));
        assert_eq!(quantile(&[], 0.5), None);
    }
}
