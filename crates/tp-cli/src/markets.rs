//! The Mercados tab: Brazilian and US stocks, indexes, ETFs and a few
//! extra coins from `config/watchlist.csv`, with trend and Stormer signals.
//! Dashboard only: nothing here goes to Telegram. `tp market-study` runs the
//! Stormer history test on the same list before any signal is trusted.

use std::path::Path;

use serde_json::{Value, json};
use tp_core::intraday::Bar;
use tp_core::stormer::{Params, Pattern, Side, ema, scan};
use tp_data::yahoo::{Interval, fetch_chart};

use super::{CliResult, held_out_start};
use crate::dashboard;
use crate::stormer_live::{Chart, Role, items_json};
use crate::stormer_study::report;

const DAY_MS: i64 = 24 * 3600 * 1000;
pub const MARKETS_FILE: &str = "dashboard_markets.json";
/// Candles sent to the dashboard per chart.
const DAILY_SENT: usize = 150;
const WEEKLY_SENT: usize = 104;

/// One line of the watchlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub group: String,
    pub symbol: String,
    pub name: String,
}

/// Reads `group,symbol,name` lines; `#` starts a comment.
#[must_use]
pub fn parse_watchlist(text: &str) -> Vec<Item> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut parts = l.splitn(3, ',');
            let group = parts.next()?.trim();
            let symbol = parts.next()?.trim();
            let name = parts.next()?.trim();
            (!group.is_empty() && tp_data::yahoo::is_valid_symbol(symbol) && !name.is_empty()).then(
                || Item {
                    group: group.to_owned(),
                    symbol: symbol.to_owned(),
                    name: name.to_owned(),
                },
            )
        })
        .collect()
}

/// An item with its candles.
struct Loaded {
    item: Item,
    daily: Vec<Bar>,
    weekly: Vec<Bar>,
}

async fn load_all(items: &[Item], now_ms: i64) -> Vec<Loaded> {
    let mut out = Vec::new();
    for item in items {
        let daily = fetch_chart(&item.symbol, Interval::Day, now_ms).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let weekly = fetch_chart(&item.symbol, Interval::Week, now_ms).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        match (daily, weekly) {
            (Ok(daily), Ok(weekly)) if daily.len() > 2 && weekly.len() > 2 => {
                println!(
                    "{} ({}): {} dias, {} semanas",
                    item.name,
                    item.symbol,
                    daily.len(),
                    weekly.len()
                );
                out.push(Loaded {
                    item: item.clone(),
                    daily,
                    weekly,
                });
            }
            (Err(e), _) | (_, Err(e)) => eprintln!("warning: {} ({}): {e}", item.name, item.symbol),
            _ => eprintln!("warning: {} ({}): poucos dados", item.name, item.symbol),
        }
    }
    out
}

fn charts(l: &Loaded) -> Vec<Chart> {
    let (dt, ds) = scan(&l.daily, Params::with_max_hold(30));
    let (wt, ws) = scan(&l.weekly, Params::with_max_hold(12));
    vec![
        Chart {
            tf: "1d",
            step_ms: DAY_MS,
            bars: l.daily.clone(),
            trades: dt,
            signals: ds,
        },
        Chart {
            tf: "1w",
            step_ms: 7 * DAY_MS,
            bars: l.weekly.clone(),
            trades: wt,
            signals: ws,
        },
    ]
}

/// Patterns shown for these assets, from `tp market-study` (each group
/// pooled, 10 years). Candidates passed both periods and beat the base
/// (entering on any candle with the same stop and target); watch ones beat
/// the base without proof. Sells lost everywhere.
fn market_role(
    group: &str,
    tf: &str,
    pattern: Pattern,
    side: Side,
    with_trend: bool,
) -> Option<Role> {
    use Pattern::{GapTrap, OneTwoThree, OneTwoThreeInside, Trap};
    if side != Side::Long {
        return None;
    }
    let candidate = match (group, tf) {
        ("Brasil", "1w") => pattern == Trap,
        ("EUA", "1w") => matches!(pattern, Trap | OneTwoThreeInside),
        ("Cripto", "1d") => pattern == OneTwoThree,
        _ => false,
    };
    if candidate {
        return Some(Role::Candidate);
    }
    let watch = match (group, tf) {
        ("Brasil", "1d") => pattern == GapTrap && with_trend,
        ("EUA", "1d") => pattern == Trap && with_trend,
        _ => false,
    };
    watch.then_some(Role::Watch)
}

/// "alta", "baixa" or "lateral" from the 8 and 80 day averages.
fn trend_word(bars: &[Bar]) -> &'static str {
    let fast = ema(bars, 8);
    let slow = ema(bars, 80);
    let i = bars.len().saturating_sub(1);
    match (
        bars.get(i),
        fast.get(i).copied().flatten(),
        slow.get(i).copied().flatten(),
    ) {
        (Some(b), Some(f), Some(s)) if b.close > f && f > s => "alta",
        (Some(b), Some(f), Some(s)) if b.close < f && f < s => "baixa",
        _ => "lateral",
    }
}

fn change(bars: &[Bar], back: usize) -> Option<f64> {
    let last = bars.last()?;
    let before = bars.get(bars.len().checked_sub(back + 1)?)?;
    (before.close > 0.0).then(|| (last.close / before.close - 1.0) * 100.0)
}

fn candles(bars: &[Bar], keep: usize) -> Vec<[f64; 5]> {
    bars[bars.len().saturating_sub(keep)..]
        .iter()
        .map(|b| {
            #[allow(clippy::cast_precision_loss)] // milliseconds since 1970 fit in f64 exactly
            let t = b.open_time_ms as f64;
            [t, sig6(b.open), sig6(b.high), sig6(b.low), sig6(b.close)]
        })
        .collect()
}

/// Six significant digits: enough for any chart, a third of the JSON size.
fn sig6(v: f64) -> f64 {
    if v == 0.0 || !v.is_finite() {
        return v;
    }
    #[allow(clippy::cast_possible_truncation)] // a price's magnitude is a small integer
    let digits = 5 - v.abs().log10().floor() as i32;
    let scale = 10f64.powi(digits);
    (v * scale).round() / scale
}

fn markets_json(now_ms: i64, loaded: &[Loaded]) -> Value {
    let assets: Vec<Value> = loaded
        .iter()
        .map(|l| {
            let c = charts(l);
            json!({
                "symbol": l.item.symbol,
                "name": l.item.name,
                "group": l.item.group,
                "price": l.daily.last().map(|b| b.close),
                "change_day": change(&l.daily, 1),
                "change_week": change(&l.weekly, 1),
                "trend": trend_word(&l.daily),
                "weekly_trend": trend_word(&l.weekly),
                "daily": candles(&l.daily, DAILY_SENT),
                "weekly": candles(&l.weekly, WEEKLY_SENT),
                "stormer": items_json(&l.item.symbol, &c, &|tf, p, side, trend| {
                    market_role(&l.item.group, tf, p, side, trend)
                }),
            })
        })
        .collect();
    json!({ "version": 1, "generated_ms": now_ms, "assets": assets })
}

fn watchlist() -> Result<Vec<Item>, Box<dyn std::error::Error>> {
    let items = parse_watchlist(&std::fs::read_to_string("config/watchlist.csv")?);
    if items.is_empty() {
        return Err("config/watchlist.csv has no valid lines".into());
    }
    Ok(items)
}

fn now_ms() -> Result<i64, Box<dyn std::error::Error>> {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    Ok(i64::try_from(since.as_millis())?)
}

/// Fetches the watchlist and publishes the Mercados tab.
///
/// # Errors
///
/// Returns an error when the watchlist is missing or the file cannot be saved.
pub async fn cmd_markets(data_dir: &Path, dry_run: bool) -> CliResult {
    let now = now_ms()?;
    let loaded = load_all(&watchlist()?, now).await;
    if loaded.is_empty() {
        return Err("no market data: Yahoo Finance did not answer for any symbol".into());
    }
    let data = markets_json(now, &loaded);
    if dry_run {
        println!(
            "dry run: {} ativos carregados, nada salvo nem enviado",
            loaded.len()
        );
        return Ok(std::process::ExitCode::SUCCESS);
    }
    dashboard::save_and_publish(&data_dir.join("live"), MARKETS_FILE, "markets", &data, &[])
        .await?;
    Ok(std::process::ExitCode::SUCCESS)
}

/// The Stormer history test on the watchlist, each group pooled.
///
/// # Errors
///
/// Returns an error when the watchlist is missing.
pub async fn cmd_market_study() -> CliResult {
    let now = now_ms()?;
    let split_ms = held_out_start()?
        .and_hms_opt(23, 59, 59)
        .ok_or("invalid date")?
        .and_utc()
        .timestamp_millis();
    let loaded = load_all(&watchlist()?, now).await;
    println!();
    println!(
        "Setups do Stormer nas ações, índices e ETFs da lista (até 10 anos). Mesmas regras do estudo de cripto: entrada no rompimento, stop na mínima do padrão, alvo em 2 vezes o risco, custos de 0,2% por trade."
    );
    let mut good = Vec::new();
    let mut groups: Vec<&str> = loaded.iter().map(|l| l.item.group.as_str()).collect();
    groups.dedup();
    for group in groups {
        let mine: Vec<&Loaded> = loaded.iter().filter(|l| l.item.group == group).collect();
        println!();
        println!("{group} ({} ativos juntos)", mine.len());
        for (tf, name, hold) in [
            ("1d", "diário (até 30 dias)", 30),
            ("1w", "semanal (até 12 semanas)", 12),
        ] {
            let sets: Vec<_> = mine
                .iter()
                .map(|l| {
                    let c = charts(l);
                    let chart = c
                        .into_iter()
                        .find(|c| c.tf == tf)
                        .unwrap_or_else(empty_chart);
                    (chart.trades, chart.bars)
                })
                .collect();
            report(&format!("{group} {name}"), &sets, split_ms, hold, &mut good);
        }
    }
    println!();
    println!("RESUMO: o que ficou positivo e acima da base nos dois períodos");
    if good.is_empty() {
        println!(" Nada.");
    }
    for g in good {
        println!(" {g}");
    }
    Ok(std::process::ExitCode::SUCCESS)
}

fn empty_chart() -> Chart {
    Chart {
        tf: "",
        step_ms: DAY_MS,
        bars: Vec::new(),
        trades: Vec::new(),
        signals: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchlist_skips_comments_and_bad_symbols() {
        let text = "# comment\nBrasil,ITUB4.SA,Itaú\nEUA,SPY,S&P 500 (SPY)\nBad,IT UB,x\n\nBrasil,^BVSP,Ibovespa, com vírgula\n";
        let items = parse_watchlist(text);
        assert_eq!(items.len(), 3);
        assert_eq!(items[2].name, "Ibovespa, com vírgula");
    }

    #[test]
    fn prices_keep_six_significant_digits() {
        assert!((sig6(136_420.123_456) - 136_420.0).abs() < 1e-9);
        assert!((sig6(36.123_456_7) - 36.1235).abs() < 1e-9);
        assert!((sig6(0.000_123_456_78) - 0.000_123_457).abs() < 1e-15);
    }

    #[test]
    fn roles_follow_the_stock_study() {
        assert_eq!(
            market_role("EUA", "1w", Pattern::Trap, Side::Long, false),
            Some(Role::Candidate)
        );
        assert_eq!(
            market_role("Brasil", "1w", Pattern::Trap, Side::Long, false),
            Some(Role::Candidate)
        );
        // Positive, but no better than entering on any week.
        assert_eq!(
            market_role("Brasil", "1w", Pattern::OneTwoThree, Side::Long, false),
            None
        );
        assert_eq!(
            market_role("Brasil", "1d", Pattern::GapTrap, Side::Long, true),
            Some(Role::Watch)
        );
        assert_eq!(
            market_role("EUA", "1w", Pattern::OneTwoThree, Side::Short, true),
            None
        );
    }
}
