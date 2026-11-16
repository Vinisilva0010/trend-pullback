//! Earnings dates from the Nasdaq earnings calendar (one day per request,
//! the same public API as the IPO calendar).

use std::time::Duration;

use chrono::NaiveDate;
use serde_json::Value;

const URL: &str = "https://api.nasdaq.com/api/calendar/earnings";
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) trend-pullback/0.1";

/// Errors while fetching a day of the calendar.
#[derive(Debug, thiserror::Error)]
pub enum EarningsError {
    #[error("network error while fetching the earnings calendar: {0}")]
    Fetch(String),
    #[error("the earnings calendar answered with HTTP status {0}")]
    Status(u16),
    #[error("unexpected earnings calendar format: {0}")]
    Format(String),
}

/// When in the day the company reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    BeforeOpen,
    AfterClose,
    Unknown,
}

impl When {
    #[must_use]
    pub fn pt(self) -> &'static str {
        match self {
            Self::BeforeOpen => "antes da abertura",
            Self::AfterClose => "depois do fechamento",
            Self::Unknown => "horário não informado",
        }
    }
}

/// A company reporting on a day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Earning {
    pub symbol: String,
    pub date: NaiveDate,
    pub when: When,
}

/// The rows of one day. A day without reports (weekend, holiday) is empty.
///
/// # Errors
///
/// Returns [`EarningsError::Format`] when the body is not the expected JSON.
pub fn parse_day(body: &str, date: NaiveDate) -> Result<Vec<Earning>, EarningsError> {
    let v: Value = serde_json::from_str(body).map_err(|e| EarningsError::Format(e.to_string()))?;
    if !v.get("data").is_some_and(|d| d.is_object() || d.is_null()) {
        return Err(EarningsError::Format("no data object".to_owned()));
    }
    let Some(rows) = v.pointer("/data/rows").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    Ok(rows
        .iter()
        .filter_map(|r| {
            let symbol = r["symbol"].as_str()?.trim().to_uppercase();
            if symbol.is_empty() {
                return None;
            }
            let when = match r["time"].as_str().unwrap_or_default() {
                "time-pre-market" => When::BeforeOpen,
                "time-after-hours" => When::AfterClose,
                _ => When::Unknown,
            };
            Some(Earning { symbol, date, when })
        })
        .collect())
}

/// The companies reporting on `date`.
///
/// # Errors
///
/// Returns an error on a network failure, a non-success answer or an
/// unexpected body.
pub async fn fetch_day(date: NaiveDate) -> Result<Vec<Earning>, EarningsError> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(|e| EarningsError::Fetch(e.without_url().to_string()))?;
    let response = client
        .get(format!("{URL}?date={}", date.format("%Y-%m-%d")))
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| EarningsError::Fetch(e.without_url().to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(EarningsError::Status(status.as_u16()));
    }
    let body = response
        .text()
        .await
        .map_err(|e| EarningsError::Fetch(e.without_url().to_string()))?;
    parse_day(&body, date)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_day_of_reports_is_read() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 17).unwrap();
        let body = r#"{"data":{"asOf":"Tue, Nov 17, 2026","headers":{},"rows":[
            {"lastYearRptDt":"11/20/2025","lastYearEPS":"$0.81","time":"time-after-hours","symbol":"NVDA","name":"NVIDIA Corporation","marketCap":"$4,000,000,000,000","fiscalQuarterEnding":"Oct/2026","epsForecast":"$1.20","noOfEsts":"20"},
            {"time":"time-pre-market","symbol":"wmt","name":"Walmart Inc."},
            {"time":"time-not-supplied","symbol":"XYZ"}
        ]},"message":null,"status":{"rCode":200}}"#;
        let e = parse_day(body, day).unwrap();
        assert_eq!(e.len(), 3);
        assert_eq!(
            e[0],
            Earning {
                symbol: "NVDA".to_owned(),
                date: day,
                when: When::AfterClose
            }
        );
        assert_eq!(e[1].symbol, "WMT");
        assert_eq!(e[1].when.pt(), "antes da abertura");
        assert_eq!(e[2].when, When::Unknown);
        // A weekend: no rows.
        assert!(
            parse_day(r#"{"data":{"rows":null}}"#, day)
                .unwrap()
                .is_empty()
        );
        assert!(
            parse_day(r#"{"data":null,"status":{"rCode":200}}"#, day)
                .unwrap()
                .is_empty()
        );
        assert!(parse_day("<html>", day).is_err());
    }
}
