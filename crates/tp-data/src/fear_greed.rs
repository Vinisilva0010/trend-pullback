//! The Crypto Fear & Greed Index of alternative.me: one value a day, from 0
//! (extreme fear) to 100 (extreme greed), with history since February 2018.

use std::time::Duration;

use chrono::{DateTime, NaiveDate};
use serde_json::Value;

const URL: &str = "https://api.alternative.me/fng/";
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) trend-pullback/0.1";

/// Errors while fetching the index.
#[derive(Debug, thiserror::Error)]
pub enum FearGreedError {
    #[error("network error while fetching the Fear & Greed Index: {0}")]
    Fetch(String),
    #[error("the Fear & Greed Index answered with HTTP status {0}")]
    Status(u16),
    #[error("unexpected Fear & Greed Index format: {0}")]
    Format(String),
}

/// One day of the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub date: NaiveDate,
    pub value: u8,
    /// The source's words: "Extreme Fear", "Fear", "Neutral", "Greed",
    /// "Extreme Greed".
    pub class: String,
}

/// The answer of the API, oldest day first.
///
/// # Errors
///
/// Returns [`FearGreedError::Format`] when the body is not the expected JSON.
pub fn parse(body: &str) -> Result<Vec<Reading>, FearGreedError> {
    let v: Value = serde_json::from_str(body).map_err(|e| FearGreedError::Format(e.to_string()))?;
    let rows = v["data"]
        .as_array()
        .ok_or_else(|| FearGreedError::Format("no data array".to_owned()))?;
    let mut out: Vec<Reading> = rows
        .iter()
        .filter_map(|r| {
            let value: u8 = r["value"]
                .as_str()?
                .trim()
                .parse()
                .ok()
                .filter(|v| *v <= 100)?;
            let ts: i64 = r["timestamp"].as_str()?.trim().parse().ok()?;
            Some(Reading {
                date: DateTime::from_timestamp(ts, 0)?.date_naive(),
                value,
                class: r["value_classification"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            })
        })
        .collect();
    if out.is_empty() {
        return Err(FearGreedError::Format("no readings".to_owned()));
    }
    out.sort_by_key(|r| r.date);
    out.dedup_by_key(|r| r.date);
    Ok(out)
}

/// The last `days` days of the index (`0` for the whole history).
///
/// # Errors
///
/// Returns an error on a network failure, a non-success answer or an
/// unexpected body.
pub async fn fetch(days: u32) -> Result<Vec<Reading>, FearGreedError> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(|e| FearGreedError::Fetch(e.without_url().to_string()))?;
    let response = client
        .get(format!("{URL}?limit={days}&format=json"))
        .send()
        .await
        .map_err(|e| FearGreedError::Fetch(e.without_url().to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(FearGreedError::Status(status.as_u16()));
    }
    let body = response
        .text()
        .await
        .map_err(|e| FearGreedError::Fetch(e.without_url().to_string()))?;
    parse(&body)
}

/// The source's class in Portuguese.
#[must_use]
pub fn class_pt(class: &str) -> &'static str {
    match class.to_ascii_lowercase().as_str() {
        "extreme fear" => "Medo extremo",
        "fear" => "Medo",
        "neutral" => "Neutro",
        "greed" => "Ganância",
        "extreme greed" => "Ganância extrema",
        _ => "Sem classe",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn readings_come_back_oldest_first() {
        let body = r#"{"name":"Fear and Greed Index","data":[
            {"value":"20","value_classification":"Extreme Fear","timestamp":"1794787200","time_until_update":"100"},
            {"value":"28","value_classification":"Fear","timestamp":"1794700800"},
            {"value":"x","value_classification":"Fear","timestamp":"1794614400"}
        ],"metadata":{"error":null}}"#;
        let r = parse(body).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].value, 28);
        assert_eq!(r[1].value, 20);
        assert_eq!(r[1].date, NaiveDate::from_ymd_opt(2026, 11, 16).unwrap());
        assert_eq!(class_pt(&r[1].class), "Medo extremo");
        assert!(parse(r#"{"data":[]}"#).is_err());
    }
}
