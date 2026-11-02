//! Calendar months used to address monthly archive files.

use std::fmt;
use std::str::FromStr;

/// A calendar month between 2000-01 and 2100-12.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct YearMonth {
    year: i32,
    month: u32,
}

/// Error for text or numbers that are not a valid month.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid month {0:?}, expected YYYY-MM between 2000-01 and 2100-12")]
pub struct InvalidMonth(pub String);

impl YearMonth {
    /// Builds a month.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMonth`] when the year is outside 2000..=2100 or the
    /// month is outside 1..=12.
    pub fn new(year: i32, month: u32) -> Result<Self, InvalidMonth> {
        if (2000..=2100).contains(&year) && (1..=12).contains(&month) {
            Ok(Self { year, month })
        } else {
            Err(InvalidMonth(format!("{year}-{month:02}")))
        }
    }

    #[must_use]
    pub fn year(self) -> i32 {
        self.year
    }

    #[must_use]
    pub fn month(self) -> u32 {
        self.month
    }

    /// The month right after this one.
    #[must_use]
    pub fn following(self) -> Self {
        if self.month == 12 {
            Self {
                year: self.year + 1,
                month: 1,
            }
        } else {
            Self {
                year: self.year,
                month: self.month + 1,
            }
        }
    }

    /// All months from `start` to `end`, both included. Empty when `start` is
    /// after `end`.
    #[must_use]
    pub fn range_inclusive(start: Self, end: Self) -> Vec<Self> {
        let mut months = Vec::new();
        let mut current = start;
        while current <= end {
            months.push(current);
            current = current.following();
        }
        months
    }
}

impl fmt::Display for YearMonth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

impl FromStr for YearMonth {
    type Err = InvalidMonth;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = || InvalidMonth(s.to_owned());
        let (year, month) = s.split_once('-').ok_or_else(invalid)?;
        let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
        if year.len() != 4 || month.len() != 2 || !digits(year) || !digits(month) {
            return Err(invalid());
        }
        let year = year.parse::<i32>().map_err(|_| invalid())?;
        let month = month.parse::<u32>().map_err(|_| invalid())?;
        Self::new(year, month).map_err(|_| invalid())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ym(year: i32, month: u32) -> YearMonth {
        YearMonth::new(year, month).unwrap()
    }

    #[test]
    fn parses_and_displays() {
        let month: YearMonth = "2019-01".parse().unwrap();
        assert_eq!(month, ym(2019, 1));
        assert_eq!(month.to_string(), "2019-01");
    }

    #[test]
    fn rejects_invalid_text() {
        for text in [
            "2019-13", "2019-00", "2019-1", "19-01", "2019/01", "abcd-ef", "2019-+1", "1999-12", "",
        ] {
            assert!(text.parse::<YearMonth>().is_err(), "{text}");
        }
    }

    #[test]
    fn following_crosses_year() {
        assert_eq!(ym(2024, 12).following(), ym(2025, 1));
        assert_eq!(ym(2024, 3).following(), ym(2024, 4));
    }

    #[test]
    fn inclusive_range() {
        let months = YearMonth::range_inclusive(ym(2019, 1), ym(2026, 10));
        assert_eq!(months.len(), 94);
        assert_eq!(months.first(), Some(&ym(2019, 1)));
        assert_eq!(months.last(), Some(&ym(2026, 10)));
        assert!(YearMonth::range_inclusive(ym(2020, 2), ym(2020, 1)).is_empty());
    }
}
