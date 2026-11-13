//! The IPO calendar of Nasdaq's public site (`api.nasdaq.com/api/ipo/
//! calendar?date=YYYY-MM`): IPOs with a date set and IPOs already priced, on
//! every US exchange. Not an official API: rows that do not parse are left
//! out and a failure is reported, never guessed around.

use std::time::Duration;

use chrono::NaiveDate;
use serde_json::Value;

const URL: &str = "https://api.nasdaq.com/api/ipo/calendar";
/// The site refuses clients without a browser-like user agent.
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) trend-pullback/0.1";

/// Errors while fetching the calendar.
#[derive(Debug, thiserror::Error)]
pub enum IpoError {
    #[error("month must be 1 to 12")]
    Month,
    #[error("network error while fetching the IPO calendar: {0}")]
    Fetch(String),
    #[error("the IPO calendar answered with HTTP status {0}")]
    Status(u16),
    #[error("unexpected IPO calendar format: {0}")]
    Format(String),
}

/// An IPO with an expected date and a price range.
#[derive(Debug, Clone, PartialEq)]
pub struct Upcoming {
    pub symbol: String,
    pub company: String,
    pub exchange: String,
    pub price_low: f64,
    pub price_high: f64,
    pub shares: f64,
    pub date: NaiveDate,
    /// Offer amount in dollars as the calendar states it.
    pub amount: f64,
}

/// An IPO that priced.
#[derive(Debug, Clone, PartialEq)]
pub struct Priced {
    pub symbol: String,
    pub company: String,
    pub exchange: String,
    pub price: f64,
    pub shares: f64,
    pub date: NaiveDate,
    pub amount: f64,
}

/// One month of the calendar.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Calendar {
    pub upcoming: Vec<Upcoming>,
    pub priced: Vec<Priced>,
}

/// `"$74,002,500,000"`, `"825,000,000"` or `"10.00"` as a number.
fn number(text: &str) -> Option<f64> {
    let clean: String = text
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    clean
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
}

/// `"70.00-78.00"` or a single price.
fn range(text: &str) -> Option<(f64, f64)> {
    match text.split_once('-') {
        Some((a, b)) => Some((number(a)?, number(b)?)),
        None => number(text).map(|p| (p, p)),
    }
}

fn date(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text.trim(), "%m/%d/%Y").ok()
}

/// A ticker of letters, digits, `.` or `-` (what Yahoo also takes).
fn symbol(text: &str) -> Option<String> {
    let s = text.trim();
    (!s.is_empty()
        && s.len() <= 12
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'))
    .then(|| s.to_owned())
}

fn text<'a>(row: &'a Value, key: &str) -> &'a str {
    row.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Reads one calendar response.
///
/// # Errors
///
/// Returns [`IpoError::Format`] when the response has no `data` object.
pub fn parse_calendar(body: &str) -> Result<Calendar, IpoError> {
    let v: Value = serde_json::from_str(body).map_err(|e| IpoError::Format(e.to_string()))?;
    let data = v
        .get("data")
        .filter(|d| d.is_object())
        .ok_or_else(|| IpoError::Format("no data object".to_owned()))?;
    let rows = |pointer: &str| {
        data.pointer(pointer)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let upcoming = rows("/upcoming/upcomingTable/rows")
        .iter()
        .filter_map(|r| {
            let (price_low, price_high) = range(text(r, "proposedSharePrice"))?;
            Some(Upcoming {
                symbol: symbol(text(r, "proposedTickerSymbol"))?,
                company: text(r, "companyName").trim().to_owned(),
                exchange: text(r, "proposedExchange").trim().to_owned(),
                price_low,
                price_high,
                shares: number(text(r, "sharesOffered"))?,
                date: date(text(r, "expectedPriceDate"))?,
                amount: number(text(r, "dollarValueOfSharesOffered"))?,
            })
        })
        .collect();
    let priced = rows("/priced/rows")
        .iter()
        .filter_map(|r| {
            Some(Priced {
                symbol: symbol(text(r, "proposedTickerSymbol"))?,
                company: text(r, "companyName").trim().to_owned(),
                exchange: text(r, "proposedExchange").trim().to_owned(),
                price: number(text(r, "proposedSharePrice"))?,
                shares: number(text(r, "sharesOffered"))?,
                date: date(text(r, "pricedDate"))?,
                amount: number(text(r, "dollarValueOfSharesOffered"))?,
            })
        })
        .collect();
    Ok(Calendar { upcoming, priced })
}

/// Fetches one month of the calendar.
///
/// # Errors
///
/// Returns [`IpoError`] for a bad month, a network failure, a non-success
/// answer or an unexpected body.
pub async fn fetch_calendar(year: i32, month: u32) -> Result<Calendar, IpoError> {
    if !(1..=12).contains(&month) {
        return Err(IpoError::Month);
    }
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(|e| IpoError::Fetch(e.without_url().to_string()))?;
    let response = client
        .get(format!("{URL}?date={year:04}-{month:02}"))
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| IpoError::Fetch(e.without_url().to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(IpoError::Status(status.as_u16()));
    }
    let body = response
        .text()
        .await
        .map_err(|e| IpoError::Fetch(e.without_url().to_string()))?;
    parse_calendar(&body)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    // The shape of a real answer (November 2026), shortened.
    const SAMPLE: &str = r#"{"data":{"priced":{"asOf":null,"rows":[{"dealID":"1390143-118358","proposedTickerSymbol":"NARA","companyName":"Narragansett Bancorp, Inc.","proposedExchange":"NASDAQ Capital","proposedSharePrice":"10.00","sharesOffered":"9,098,800","pricedDate":"11/02/2026","dollarValueOfSharesOffered":"$90,988,000","dealStatus":"Priced"}]},"upcoming":{"upcomingTable":{"asOf":null,"rows":[{"dealID":"1407642-119728","proposedTickerSymbol":"ANT","companyName":"ANTHROPIC, PBC","proposedExchange":"NASDAQ Global Select","proposedSharePrice":"70.00-78.00","sharesOffered":"825,000,000","expectedPriceDate":"11/19/2026","dollarValueOfSharesOffered":"$74,002,500,000"},{"dealID":"x","proposedTickerSymbol":"","companyName":"No ticker yet","proposedSharePrice":"5.00","sharesOffered":"1","expectedPriceDate":"11/20/2026","dollarValueOfSharesOffered":"$5"}]}}}}"#;

    #[test]
    fn reads_upcoming_and_priced_rows() {
        let c = parse_calendar(SAMPLE).unwrap();
        assert_eq!(c.upcoming.len(), 1, "a row without a ticker is left out");
        let u = &c.upcoming[0];
        assert_eq!(u.symbol, "ANT");
        assert_eq!((u.price_low, u.price_high), (70.0, 78.0));
        assert_eq!(u.shares, 825_000_000.0);
        assert_eq!(u.amount, 74_002_500_000.0);
        assert_eq!(u.date, NaiveDate::from_ymd_opt(2026, 11, 19).unwrap());
        let p = &c.priced[0];
        assert_eq!(
            (p.symbol.as_str(), p.price, p.amount),
            ("NARA", 10.0, 90_988_000.0)
        );
        assert_eq!(p.date, NaiveDate::from_ymd_opt(2026, 11, 2).unwrap());
    }

    #[test]
    fn an_empty_month_and_a_broken_body() {
        assert_eq!(
            parse_calendar(r#"{"data":{"priced":{"rows":null}}}"#).unwrap(),
            Calendar::default()
        );
        assert!(parse_calendar(r#"{"data":null}"#).is_err());
        assert!(parse_calendar("<html>").is_err());
    }
}
