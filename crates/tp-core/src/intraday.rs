//! Intraday bars (open time in UTC milliseconds) and their aggregation.

/// One hour in milliseconds.
pub const HOUR_MS: i64 = 3_600_000;

/// One intraday bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    pub open_time_ms: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    /// Traded volume in the base asset (0 when unknown).
    pub volume: f64,
}

/// Groups hourly bars into bars of `hours` hours aligned to UTC multiples of
/// that size; groups that are not complete are dropped.
#[must_use]
pub fn aggregate(bars: &[Bar], hours: i64) -> Vec<Bar> {
    let size = hours * HOUR_MS;
    let expected = usize::try_from(hours).unwrap_or(0);
    let mut out = Vec::new();
    let mut group: Vec<Bar> = Vec::new();
    let flush = |group: &mut Vec<Bar>, out: &mut Vec<Bar>| {
        if expected > 0 && group.len() == expected {
            out.push(Bar {
                open_time_ms: group[0].open_time_ms,
                open: group[0].open,
                high: group.iter().map(|b| b.high).fold(f64::MIN, f64::max),
                low: group.iter().map(|b| b.low).fold(f64::MAX, f64::min),
                close: group[group.len() - 1].close,
                volume: group.iter().map(|b| b.volume).sum(),
            });
        }
        group.clear();
    };
    for &bar in bars {
        let new_window = group
            .first()
            .is_some_and(|f| f.open_time_ms.div_euclid(size) != bar.open_time_ms.div_euclid(size));
        if new_window {
            flush(&mut group, &mut out);
        }
        group.push(bar);
    }
    flush(&mut group, &mut out);
    out
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn hour(i: i64, price: f64) -> Bar {
        Bar {
            open_time_ms: 1_704_067_200_000 + i * HOUR_MS,
            open: price,
            high: price + 1.0,
            low: price - 1.0,
            close: price,
            volume: 1.0,
        }
    }

    #[test]
    fn four_hour_bars_by_hand() {
        let bars: Vec<Bar> = (0..10)
            .map(|i| hour(i, 100.0 + f64::from(i32::try_from(i).unwrap())))
            .collect();
        let four = aggregate(&bars, 4);
        assert_eq!(four.len(), 2);
        assert_eq!(four[0].open, 100.0);
        assert_eq!(four[0].close, 103.0);
        assert_eq!(four[0].high, 104.0);
        assert_eq!(four[0].low, 99.0);
        assert_eq!(four[1].open_time_ms, bars[4].open_time_ms);
    }

    #[test]
    fn incomplete_groups_are_dropped() {
        let mut bars: Vec<Bar> = (0..4).map(|i| hour(i, 100.0)).collect();
        bars.remove(2);
        assert!(aggregate(&bars, 4).is_empty());
    }
}
