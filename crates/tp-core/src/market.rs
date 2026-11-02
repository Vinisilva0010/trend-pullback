//! Core market data types.

use chrono::NaiveDate;

/// Raw fields used to build a [`Candle`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CandleFields {
    /// Trading day (UTC) the candle covers.
    pub date: NaiveDate,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    /// Volume in the base asset.
    pub volume: f64,
    /// Volume in the quote asset (for example USDT or USD).
    pub quote_volume: f64,
}

/// Reasons a set of candle fields is rejected.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum InvalidCandle {
    #[error("{date}: prices must be finite and greater than zero")]
    Price { date: NaiveDate },
    #[error("{date}: volumes must be finite and not negative")]
    Volume { date: NaiveDate },
    #[error("{date}: high and low do not contain open and close")]
    Range { date: NaiveDate },
}

/// One validated daily candle.
///
/// A `Candle` can only be built through [`Candle::try_new`], so every instance
/// has positive finite prices, non-negative volumes and a consistent range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candle {
    fields: CandleFields,
}

impl Candle {
    /// Validates the fields and builds a candle.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidCandle`] when a price is not finite or not positive,
    /// when a volume is not finite or negative, or when high and low do not
    /// contain open and close.
    pub fn try_new(f: CandleFields) -> Result<Self, InvalidCandle> {
        let prices = [f.open, f.high, f.low, f.close];
        if prices.iter().any(|p| !p.is_finite() || *p <= 0.0) {
            return Err(InvalidCandle::Price { date: f.date });
        }
        let volumes = [f.volume, f.quote_volume];
        if volumes.iter().any(|v| !v.is_finite() || *v < 0.0) {
            return Err(InvalidCandle::Volume { date: f.date });
        }
        if f.low > f.open.min(f.close) || f.high < f.open.max(f.close) {
            return Err(InvalidCandle::Range { date: f.date });
        }
        Ok(Self { fields: f })
    }

    #[must_use]
    pub fn date(&self) -> NaiveDate {
        self.fields.date
    }

    #[must_use]
    pub fn open(&self) -> f64 {
        self.fields.open
    }

    #[must_use]
    pub fn high(&self) -> f64 {
        self.fields.high
    }

    #[must_use]
    pub fn low(&self) -> f64 {
        self.fields.low
    }

    #[must_use]
    pub fn close(&self) -> f64 {
        self.fields.close
    }

    #[must_use]
    pub fn volume(&self) -> f64 {
        self.fields.volume
    }

    #[must_use]
    pub fn quote_volume(&self) -> f64 {
        self.fields.quote_volume
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn fields() -> CandleFields {
        CandleFields {
            date: NaiveDate::from_ymd_opt(2024, 12, 1).unwrap(),
            open: 100.0,
            high: 110.0,
            low: 95.0,
            close: 105.0,
            volume: 10.0,
            quote_volume: 1_000.0,
        }
    }

    #[test]
    fn accepts_valid_candle() {
        let candle = Candle::try_new(fields()).unwrap();
        assert_eq!(candle.date(), fields().date);
        assert_eq!(candle.close(), 105.0);
        assert_eq!(candle.quote_volume(), 1_000.0);
    }

    #[test]
    fn accepts_zero_volume() {
        let mut f = fields();
        f.volume = 0.0;
        f.quote_volume = 0.0;
        assert!(Candle::try_new(f).is_ok());
    }

    #[test]
    fn rejects_bad_prices() {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut f = fields();
            f.open = bad;
            assert!(matches!(
                Candle::try_new(f),
                Err(InvalidCandle::Price { .. })
            ));
        }
    }

    #[test]
    fn rejects_bad_volumes() {
        for bad in [-0.1, f64::NAN, f64::INFINITY] {
            let mut f = fields();
            f.quote_volume = bad;
            assert!(matches!(
                Candle::try_new(f),
                Err(InvalidCandle::Volume { .. })
            ));
        }
    }

    #[test]
    fn rejects_high_below_close() {
        let mut f = fields();
        f.high = 104.0;
        assert!(matches!(
            Candle::try_new(f),
            Err(InvalidCandle::Range { .. })
        ));
    }

    #[test]
    fn rejects_low_above_open() {
        let mut f = fields();
        f.low = 101.0;
        assert!(matches!(
            Candle::try_new(f),
            Err(InvalidCandle::Range { .. })
        ));
    }
}
