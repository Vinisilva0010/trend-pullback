//! Daily and weekly candles for stocks, indexes, ETFs and currencies from
//! Yahoo Finance's public chart endpoint (no account or key). It is not an
//! official API: errors are reported per symbol and never stop a run.

use std::time::Duration;

use serde_json::Value;
use tp_core::intraday::Bar;

const URL: &str = "https://query1.finance.yahoo.com/v8/finance/chart";
/// Yahoo answers 429 to clients without a browser-like user agent.
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) trend-pullback/0.1";

/// Errors while fetching or reading a chart.
#[derive(Debug, thiserror::Error)]
pub enum YahooError {
    #[error("invalid symbol")]
    Symbol,
    #[error("request failed: {0}")]
    Fetch(String),
    #[error("HTTP status {0}")]
    Status(u16),
    #[error("unexpected response: {0}")]
    Format(String),
}

/// Candle size to ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interval {
    Day,
    Week,
}

/// Letters, digits and `^ . = -` only (e.g. `ITUB4.SA`, `^BVSP`, `USDBRL=X`).
#[must_use]
pub fn is_valid_symbol(symbol: &str) -> bool {
    (1..=20).contains(&symbol.len())
        && symbol
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'^' | b'.' | b'=' | b'-'))
}

fn number(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|x| x.is_finite())
}

/// Reads a chart response into candles, oldest first. Rows with a missing
/// price (holidays, halts) are skipped.
///
/// # Errors
///
/// Returns [`YahooError::Format`] when the response is not a chart.
pub fn parse_chart(text: &str) -> Result<Vec<Bar>, YahooError> {
    let v: Value = serde_json::from_str(text).map_err(|e| YahooError::Format(e.to_string()))?;
    let result = v.pointer("/chart/result/0").ok_or_else(|| {
        let why = v
            .pointer("/chart/error/description")
            .and_then(Value::as_str)
            .unwrap_or("missing chart.result");
        YahooError::Format(why.to_owned())
    })?;
    let times = result
        .get("timestamp")
        .and_then(Value::as_array)
        .ok_or_else(|| YahooError::Format("missing timestamp".to_owned()))?;
    let quote = result
        .pointer("/indicators/quote/0")
        .ok_or_else(|| YahooError::Format("missing quote".to_owned()))?;
    let column = |name: &str| quote.get(name).and_then(Value::as_array);
    let (Some(open), Some(high), Some(low), Some(close)) = (
        column("open"),
        column("high"),
        column("low"),
        column("close"),
    ) else {
        return Err(YahooError::Format("missing price columns".to_owned()));
    };
    let volume = column("volume");
    let mut bars: Vec<Bar> = times
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let bar = Bar {
                open_time_ms: t.as_i64()? * 1000,
                open: number(open.get(i))?,
                high: number(high.get(i))?,
                low: number(low.get(i))?,
                close: number(close.get(i))?,
                volume: volume.and_then(|v| number(v.get(i))).unwrap_or(0.0),
            };
            (bar.low <= bar.high && bar.low > 0.0).then_some(bar)
        })
        .collect();
    bars.sort_by_key(|b| b.open_time_ms);
    bars.dedup_by_key(|b| b.open_time_ms);
    Ok(bars)
}

/// Drops the last candle while it may still be forming: a daily candle of
/// today before 22:00 UTC (after the close of B3 and New York), or a weekly
/// candle whose week has not ended.
#[must_use]
pub fn closed_only(mut bars: Vec<Bar>, interval: Interval, now_ms: i64) -> Vec<Bar> {
    const DAY: i64 = 24 * 3600 * 1000;
    if let Some(last) = bars.last() {
        let open = match interval {
            Interval::Day => {
                let day_start = last.open_time_ms.div_euclid(DAY) * DAY;
                now_ms < day_start + 22 * 3600 * 1000
            }
            Interval::Week => now_ms < last.open_time_ms + 7 * DAY - 2 * 3600 * 1000,
        };
        if open {
            bars.pop();
        }
    }
    bars
}

/// Today's and the last few sessions' 15-minute candles, with the last
/// price and how many minutes the exchange's data is delayed.
#[derive(Debug, Clone)]
pub struct Intraday {
    pub bars: Vec<Bar>,
    pub price: Option<f64>,
    pub delay_min: i64,
}

/// Reads an intraday chart response.
///
/// # Errors
///
/// Returns [`YahooError::Format`] when the response is not a chart.
pub fn parse_intraday(text: &str) -> Result<Intraday, YahooError> {
    let bars = parse_chart(text)?;
    let v: Value = serde_json::from_str(text).map_err(|e| YahooError::Format(e.to_string()))?;
    let meta = v.pointer("/chart/result/0/meta");
    let price = number(meta.and_then(|m| m.get("regularMarketPrice")))
        .or_else(|| bars.last().map(|b| b.close));
    let delay_min = meta
        .and_then(|m| m.get("exchangeDataDelayedBy"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    Ok(Intraday {
        bars,
        price,
        delay_min,
    })
}

async fn get_text(path: &str) -> Result<String, YahooError> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| YahooError::Fetch(e.without_url().to_string()))?;
    let response = client
        .get(format!("{URL}/{path}"))
        .send()
        .await
        .map_err(|e| YahooError::Fetch(e.without_url().to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(YahooError::Status(status.as_u16()));
    }
    response
        .text()
        .await
        .map_err(|e| YahooError::Fetch(e.without_url().to_string()))
}

fn encode(symbol: &str) -> String {
    symbol.replace('^', "%5E").replace('=', "%3D")
}

/// Fetches the last five sessions in 15-minute candles (regular hours).
///
/// # Errors
///
/// Returns [`YahooError`] for a bad symbol, a network failure, a non-success
/// answer or an unexpected body.
pub async fn fetch_intraday(symbol: &str) -> Result<Intraday, YahooError> {
    if !is_valid_symbol(symbol) {
        return Err(YahooError::Symbol);
    }
    parse_intraday(&get_text(&format!("{}?range=5d&interval=15m", encode(symbol))).await?)
}

/// Fetches up to ten years of closed candles.
///
/// # Errors
///
/// Returns [`YahooError`] for a bad symbol, a network failure, a non-success
/// answer or an unexpected body.
pub async fn fetch_chart(
    symbol: &str,
    interval: Interval,
    now_ms: i64,
) -> Result<Vec<Bar>, YahooError> {
    if !is_valid_symbol(symbol) {
        return Err(YahooError::Symbol);
    }
    let step = match interval {
        Interval::Day => "1d",
        Interval::Week => "1wk",
    };
    let text = get_text(&format!("{}?range=10y&interval={step}", encode(symbol))).await?;
    Ok(closed_only(parse_chart(&text)?, interval, now_ms))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"chart":{"result":[{"meta":{"currency":"BRL","symbol":"ITUB4.SA"},"timestamp":[1762520400,1762779600,1762866000],"indicators":{"quote":[{"open":[36.1,36.5,null],"high":[36.9,37.0,null],"low":[35.8,36.2,null],"close":[36.6,36.8,null],"volume":[1000,2000,null]}]}}],"error":null}}"#;

    #[test]
    fn reads_candles_and_skips_empty_rows() {
        let bars = parse_chart(SAMPLE).unwrap();
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].open_time_ms, 1_762_520_400_000);
        assert_eq!(
            (bars[1].open, bars[1].close, bars[1].volume),
            (36.5, 36.8, 2000.0)
        );
    }

    #[test]
    fn reports_yahoo_errors() {
        let text = r#"{"chart":{"result":null,"error":{"code":"Not Found","description":"No data found, symbol may be delisted"}}}"#;
        let e = parse_chart(text).unwrap_err().to_string();
        assert!(e.contains("delisted"), "{e}");
    }

    #[test]
    fn drops_a_candle_that_may_still_be_forming() {
        let bars = parse_chart(SAMPLE).unwrap();
        let last_day = bars[1].open_time_ms;
        // Same day, before 22:00 UTC: dropped. Next day: kept.
        assert_eq!(
            closed_only(bars.clone(), Interval::Day, last_day + 3600 * 1000).len(),
            1
        );
        assert_eq!(
            closed_only(bars, Interval::Day, last_day + 30 * 3600 * 1000).len(),
            2
        );
    }

    #[test]
    fn symbols_are_checked() {
        assert!(is_valid_symbol("^BVSP"));
        assert!(is_valid_symbol("USDBRL=X"));
        assert!(is_valid_symbol("BPAC11.SA"));
        assert!(!is_valid_symbol("ITUB4.SA/../x"));
        assert!(!is_valid_symbol(""));
    }

    #[test]
    fn reads_the_last_price_and_the_delay() {
        let text = SAMPLE.replace(
            r#""symbol":"ITUB4.SA"}"#,
            r#""symbol":"ITUB4.SA","regularMarketPrice":36.95,"exchangeDataDelayedBy":15}"#,
        );
        let d = parse_intraday(&text).unwrap();
        assert_eq!(d.price, Some(36.95));
        assert_eq!(d.delay_min, 15);
        assert_eq!(d.bars.len(), 2);
    }
}
