//! Futures positioning from Bybit and OKX public APIs (no account or key):
//! open interest and the share of accounts or top-trader money that is long.
//! Binance's own endpoints live in `binance::rest`.

use serde_json::Value;

use crate::binance::download::Http;

/// Bybit public API.
pub const BYBIT_URL: &str = "https://api.bybit.com";
/// OKX public API.
pub const OKX_URL: &str = "https://www.okx.com";

/// Errors while fetching or parsing positioning data.
#[derive(Debug, thiserror::Error)]
pub enum PositioningError {
    #[error("{0}: request failed: {1}")]
    Fetch(&'static str, String),
    #[error("{0}: unexpected response: {1}")]
    Format(&'static str, String),
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
    .filter(|x: &f64| x.is_finite())
}

fn time(v: &Value) -> Option<i64> {
    match v {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

/// Sorts (time, value) rows oldest first and checks they are usable.
fn ordered(
    mut rows: Vec<(i64, f64)>,
    source: &'static str,
) -> Result<Vec<(i64, f64)>, PositioningError> {
    if rows.is_empty() {
        return Err(PositioningError::Format(source, "empty list".to_owned()));
    }
    rows.sort_by_key(|(t, _)| *t);
    rows.dedup_by_key(|(t, _)| *t);
    Ok(rows)
}

/// Parses Bybit `/v5/market/account-ratio` into (time ms, long share).
///
/// # Errors
///
/// Returns [`PositioningError::Format`] when the response is not the expected shape.
pub fn parse_bybit_ratio(text: &str) -> Result<Vec<(i64, f64)>, PositioningError> {
    const SRC: &str = "Bybit long/short";
    let v: Value =
        serde_json::from_str(text).map_err(|e| PositioningError::Format(SRC, e.to_string()))?;
    let list = v
        .pointer("/result/list")
        .and_then(Value::as_array)
        .ok_or_else(|| PositioningError::Format(SRC, "missing result.list".to_owned()))?;
    let rows = list
        .iter()
        .map(|r| {
            let t = r.get("timestamp").and_then(time)?;
            let buy = r.get("buyRatio").and_then(number)?;
            (0.0..=1.0).contains(&buy).then_some((t, buy))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| PositioningError::Format(SRC, "bad row".to_owned()))?;
    ordered(rows, SRC)
}

/// Parses Bybit `/v5/market/open-interest` into (time ms, open interest in the base asset).
///
/// # Errors
///
/// Returns [`PositioningError::Format`] when the response is not the expected shape.
pub fn parse_bybit_open_interest(text: &str) -> Result<Vec<(i64, f64)>, PositioningError> {
    const SRC: &str = "Bybit open interest";
    let v: Value =
        serde_json::from_str(text).map_err(|e| PositioningError::Format(SRC, e.to_string()))?;
    let list = v
        .pointer("/result/list")
        .and_then(Value::as_array)
        .ok_or_else(|| PositioningError::Format(SRC, "missing result.list".to_owned()))?;
    let rows = list
        .iter()
        .map(|r| {
            let t = r.get("timestamp").and_then(time)?;
            let oi = r.get("openInterest").and_then(number)?;
            (oi >= 0.0).then_some((t, oi))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| PositioningError::Format(SRC, "bad row".to_owned()))?;
    ordered(rows, SRC)
}

/// Parses an OKX rubik ratio response (`data` rows of `[ts, longs/shorts]`)
/// into (time ms, long share).
///
/// # Errors
///
/// Returns [`PositioningError::Format`] when the response is not the expected shape.
pub fn parse_okx_ratio(text: &str) -> Result<Vec<(i64, f64)>, PositioningError> {
    const SRC: &str = "OKX long/short";
    let v: Value =
        serde_json::from_str(text).map_err(|e| PositioningError::Format(SRC, e.to_string()))?;
    let data = v
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| PositioningError::Format(SRC, "missing data".to_owned()))?;
    let rows = data
        .iter()
        .map(|r| {
            let row = r.as_array()?;
            let t = time(row.first()?)?;
            let ratio = number(row.get(1)?)?;
            (ratio >= 0.0).then_some((t, ratio / (1.0 + ratio)))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| PositioningError::Format(SRC, "bad row".to_owned()))?;
    ordered(rows, SRC)
}

/// Parses OKX `/api/v5/rubik/stat/contracts/open-interest-history`
/// (`data` rows of `[ts, oi, oiCcy, oiUsd]`) into (time ms, open interest in USD).
///
/// # Errors
///
/// Returns [`PositioningError::Format`] when the response is not the expected shape.
pub fn parse_okx_open_interest(text: &str) -> Result<Vec<(i64, f64)>, PositioningError> {
    const SRC: &str = "OKX open interest";
    let v: Value =
        serde_json::from_str(text).map_err(|e| PositioningError::Format(SRC, e.to_string()))?;
    let data = v
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| PositioningError::Format(SRC, "missing data".to_owned()))?;
    let rows = data
        .iter()
        .map(|r| {
            let row = r.as_array()?;
            let t = time(row.first()?)?;
            let usd = number(row.get(3)?)?;
            (usd >= 0.0).then_some((t, usd))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| PositioningError::Format(SRC, "bad row".to_owned()))?;
    ordered(rows, SRC)
}

async fn get(source: &'static str, url: &str) -> Result<String, PositioningError> {
    let http = Http::new(2).map_err(|e| PositioningError::Fetch(source, e.to_string()))?;
    let body = http
        .get_bytes(url)
        .await
        .map_err(|e| PositioningError::Fetch(source, e.to_string()))?;
    String::from_utf8(body).map_err(|_| PositioningError::Format(source, "not UTF-8".to_owned()))
}

/// Asset part of a USDT pair (`BTCUSDT` -> `BTC`), letters and digits only.
fn base(symbol: &str) -> Option<&str> {
    let b = symbol.strip_suffix("USDT")?;
    (!b.is_empty() && b.chars().all(|c| c.is_ascii_alphanumeric())).then_some(b)
}

/// Bybit hourly long share of accounts for `symbol` (e.g. `BTCUSDT`), last `hours` hours.
///
/// # Errors
///
/// Returns [`PositioningError`] on a bad symbol, a failed request or an unexpected response.
pub async fn fetch_bybit_ratio(
    symbol: &str,
    hours: u32,
) -> Result<Vec<(i64, f64)>, PositioningError> {
    base(symbol).ok_or_else(|| PositioningError::Format("Bybit", "bad symbol".to_owned()))?;
    let url = format!(
        "{BYBIT_URL}/v5/market/account-ratio?category=linear&symbol={symbol}&period=1h&limit={}",
        hours.clamp(1, 500)
    );
    parse_bybit_ratio(&get("Bybit long/short", &url).await?)
}

/// Bybit latest hourly open interest for `symbol`, in the base asset.
///
/// # Errors
///
/// Returns [`PositioningError`] on a bad symbol, a failed request or an unexpected response.
pub async fn fetch_bybit_open_interest(symbol: &str) -> Result<Vec<(i64, f64)>, PositioningError> {
    base(symbol).ok_or_else(|| PositioningError::Format("Bybit", "bad symbol".to_owned()))?;
    let url = format!(
        "{BYBIT_URL}/v5/market/open-interest?category=linear&symbol={symbol}&intervalTime=1h&limit=2"
    );
    parse_bybit_open_interest(&get("Bybit open interest", &url).await?)
}

/// Which OKX ratio to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OkxRatio {
    /// Long share of all accounts.
    Accounts,
    /// Long share of the position value of top traders.
    TopPositions,
}

/// OKX hourly long share for the `symbol` perpetual (e.g. `BTCUSDT` -> `BTC-USDT-SWAP`).
///
/// # Errors
///
/// Returns [`PositioningError`] on a bad symbol, a failed request or an unexpected response.
pub async fn fetch_okx_ratio(
    symbol: &str,
    kind: OkxRatio,
    hours: u32,
) -> Result<Vec<(i64, f64)>, PositioningError> {
    let b = base(symbol).ok_or_else(|| PositioningError::Format("OKX", "bad symbol".to_owned()))?;
    let path = match kind {
        OkxRatio::Accounts => "long-short-account-ratio-contract",
        OkxRatio::TopPositions => "long-short-position-ratio-contract-top-trader",
    };
    let url = format!(
        "{OKX_URL}/api/v5/rubik/stat/contracts/{path}?instId={b}-USDT-SWAP&period=1H&limit={}",
        hours.clamp(1, 100)
    );
    parse_okx_ratio(&get("OKX long/short", &url).await?)
}

/// OKX latest hourly open interest for the `symbol` perpetual, in USD.
///
/// # Errors
///
/// Returns [`PositioningError`] on a bad symbol, a failed request or an unexpected response.
pub async fn fetch_okx_open_interest(symbol: &str) -> Result<Vec<(i64, f64)>, PositioningError> {
    let b = base(symbol).ok_or_else(|| PositioningError::Format("OKX", "bad symbol".to_owned()))?;
    let url = format!(
        "{OKX_URL}/api/v5/rubik/stat/contracts/open-interest-history?instId={b}-USDT-SWAP&period=1H&limit=2"
    );
    parse_okx_open_interest(&get("OKX open interest", &url).await?)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn bybit_rows_come_newest_first_and_are_sorted() {
        let text = r#"{"retCode":0,"result":{"list":[{"symbol":"BTCUSDT","buyRatio":"0.49","sellRatio":"0.51","timestamp":"1696262400000"},{"symbol":"BTCUSDT","buyRatio":"0.4927","sellRatio":"0.5073","timestamp":"1696258800000"}]}}"#;
        let rows = parse_bybit_ratio(text).unwrap();
        assert_eq!(
            rows,
            vec![(1_696_258_800_000, 0.4927), (1_696_262_400_000, 0.49)]
        );
        let oi = r#"{"retCode":0,"result":{"symbol":"BTCUSDT","list":[{"openInterest":"55000.5","timestamp":"1696262400000"}]}}"#;
        assert_eq!(
            parse_bybit_open_interest(oi).unwrap(),
            vec![(1_696_262_400_000, 55000.5)]
        );
        assert!(parse_bybit_ratio(r#"{"retCode":10001,"retMsg":"error"}"#).is_err());
    }

    #[test]
    fn okx_ratio_becomes_a_long_share() {
        let text = r#"{"code":"0","msg":"","data":[["1701421200000","3"],["1701417600000","1"]]}"#;
        let rows = parse_okx_ratio(text).unwrap();
        assert_eq!(
            rows,
            vec![(1_701_417_600_000, 0.5), (1_701_421_200_000, 0.75)]
        );
        let oi = r#"{"code":"0","msg":"","data":[["1701417600000","1000","10","800000000"]]}"#;
        assert_eq!(
            parse_okx_open_interest(oi).unwrap(),
            vec![(1_701_417_600_000, 800_000_000.0)]
        );
        assert!(parse_okx_ratio(r#"{"code":"0","data":[]}"#).is_err());
    }

    #[test]
    fn symbols_are_checked() {
        assert_eq!(base("SOLUSDT"), Some("SOL"));
        assert_eq!(base("SOL/USDT"), None);
        assert_eq!(base("USDT"), None);
    }
}
