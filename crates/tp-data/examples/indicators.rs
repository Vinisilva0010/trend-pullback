//! Prints indicators for the last three days of a processed series, to compare
//! against a charting platform (daily candles, UTC).
//!
//! Usage: `cargo run -p tp-data --example indicators -- data/processed/binance/spot/1d/BTCUSDT.csv`

use std::{env, error::Error, fs};

use tp_core::indicators::{atr, by_segment, close_values, ema, highest, rate_of_change, sma};
use tp_data::history::from_csv;

fn show(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_owned(), |v| format!("{v:.2}"))
}

fn main() -> Result<(), Box<dyn Error>> {
    let path = env::args()
        .nth(1)
        .ok_or("usage: indicators <processed.csv>")?;
    let candles = from_csv(&path, &fs::read_to_string(&path)?)?;
    let ema20 = by_segment(&candles, &|s| ema(&close_values(s), 20));
    let sma50 = by_segment(&candles, &|s| sma(&close_values(s), 50));
    let sma200 = by_segment(&candles, &|s| sma(&close_values(s), 200));
    let atr14 = by_segment(&candles, &|s| atr(s, 14));
    let roc90 = by_segment(&candles, &|s| rate_of_change(&close_values(s), 90));
    let high10 = by_segment(&candles, &|s| {
        let highs: Vec<f64> = s.iter().map(tp_core::market::Candle::high).collect();
        highest(&highs, 10)
    });
    let start = candles.len().saturating_sub(3);
    for (i, c) in candles.iter().enumerate().skip(start) {
        let (high, ago) = high10[i].map_or((None, None), |(h, a)| (Some(h), Some(a)));
        println!(
            "{} close {:.2} | EMA20 {} | SMA50 {} | SMA200 {} | ATR14 {} | ROC90 {} | max10 {} ({} dias atras)",
            c.date(),
            c.close(),
            show(ema20[i]),
            show(sma50[i]),
            show(sma200[i]),
            show(atr14[i]),
            roc90[i].map_or_else(|| "-".to_owned(), |v| format!("{:.1}%", v * 100.0)),
            show(high),
            ago.map_or_else(|| "-".to_owned(), |a| a.to_string())
        );
    }
    Ok(())
}
