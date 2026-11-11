//! The Mercados tab: Brazilian and US stocks, indexes, ETFs and a few
//! extra coins from `config/watchlist.csv`, with trend and Stormer signals.
//! The US part of `config/trade_universe.csv` is searched too for the stock
//! trades (`stock_trades`), which go to Telegram and the phone with
//! `--notify`. `tp market-study` runs the Stormer history test on a list
//! before any signal is trusted.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use serde_json::{Value, json};
use tp_core::intraday::Bar;
use tp_core::stormer::{Params, Pattern, Side, ema, scan};
use tp_data::yahoo::{Interval, fetch_chart};

use super::{CliResult, held_out_start, parse_env, send_telegram};
use crate::stock_trades::{self, Stock};
use crate::stock_watch;
use crate::stormer_live::{Chart, Role, items_json};
use crate::stormer_study::report;
use crate::{dashboard, smc_live};

const DAY_MS: i64 = 24 * 3600 * 1000;
pub const MARKETS_FILE: &str = "dashboard_markets.json";
/// Candles sent to the dashboard per chart.
const DAILY_SENT: usize = 150;
const WEEKLY_SENT: usize = 104;
const MONTHLY_SENT: usize = 120;

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
pub(crate) struct Loaded {
    pub(crate) item: Item,
    pub(crate) daily: Vec<Bar>,
    pub(crate) weekly: Vec<Bar>,
}

pub(crate) async fn load_all(items: &[Item], now_ms: i64) -> Vec<Loaded> {
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

/// From a Yahoo candle's time to the close, (daily, weekly). A daily
/// candle is stamped at the session open (09:30 in New York, 10:00 on B3),
/// a weekly one at Monday midnight in the exchange's time zone; the week
/// closes on Friday at 16:00 in New York, 17:00 on B3.
fn session_ms(group: &str) -> Option<(i64, i64)> {
    const HOUR_MS: i64 = 3600 * 1000;
    match group {
        "EUA" => Some((13 * HOUR_MS / 2, 4 * DAY_MS + 16 * HOUR_MS)),
        "Brasil" => Some((7 * HOUR_MS, 4 * DAY_MS + 17 * HOUR_MS)),
        _ => None,
    }
}

fn charts(l: &Loaded) -> Vec<Chart> {
    let d = scan(&l.daily, Params::with_max_hold(30));
    let w = scan(&l.weekly, Params::with_max_hold(12));
    let daily = Chart::round_the_clock("1d", DAY_MS, l.daily.clone(), d);
    let weekly = Chart::round_the_clock("1w", 7 * DAY_MS, l.weekly.clone(), w);
    match session_ms(&l.item.group) {
        Some((day, week)) => vec![
            Chart {
                close_after_ms: day,
                weekdays_only: true,
                ..daily
            },
            Chart {
                close_after_ms: week,
                weekdays_only: true,
                ..weekly
            },
        ],
        None => vec![daily, weekly],
    }
}

/// Patterns shown for these assets, from `tp market-study`. US stocks: the
/// two trap buys that beat the base on the 84 US assets of the trade
/// universe (they are the stock trades). Brazil: nothing beat the base on
/// its 62 assets. Coins: the daily 1-2-3 buy of the watchlist test.
fn market_role(
    group: &str,
    tf: &str,
    pattern: Pattern,
    side: Side,
    with_trend: bool,
) -> Option<Role> {
    match group {
        "EUA" => stock_trades::role(tf, pattern, side, with_trend),
        "Cripto" if tf == "1d" && pattern == Pattern::OneTwoThree && side == Side::Long => {
            Some(Role::Candidate)
        }
        _ => None,
    }
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

/// Monthly candles from weekly ones: each week goes to the month of its
/// Monday (a week across two months counts in the first).
fn monthly(weekly: &[Bar]) -> Vec<Bar> {
    let mut out: Vec<Bar> = Vec::new();
    let month_of = |ms: i64| {
        chrono::DateTime::from_timestamp_millis(ms).map(|d| {
            use chrono::Datelike;
            (d.year(), d.month())
        })
    };
    for w in weekly {
        match out.last_mut() {
            Some(m) if month_of(m.open_time_ms) == month_of(w.open_time_ms) => {
                m.high = m.high.max(w.high);
                m.low = m.low.min(w.low);
                m.close = w.close;
                m.volume += w.volume;
            }
            _ => out.push(*w),
        }
    }
    out
}

/// The long view of an asset from ten years of weekly candles: the
/// all-time high (over the history fetched), how far the price is from it,
/// the 52-week range, the 12-month change and the 10-month average trend
/// (price above the average of the last 10 monthly closes).
fn long_view(weekly: &[Bar]) -> Value {
    let Some(last) = weekly.last() else {
        return Value::Null;
    };
    let ath = weekly.iter().map(|b| b.high).fold(f64::MIN, f64::max);
    let year = &weekly[weekly.len().saturating_sub(52)..];
    let high_52 = year.iter().map(|b| b.high).fold(f64::MIN, f64::max);
    let low_52 = year.iter().map(|b| b.low).fold(f64::MAX, f64::min);
    let months = monthly(weekly);
    let closes: Vec<f64> = months.iter().rev().take(10).map(|m| m.close).collect();
    #[allow(clippy::cast_precision_loss)] // ten closes
    let sma10 = (closes.len() == 10).then(|| closes.iter().sum::<f64>() / closes.len() as f64);
    let pct = |a: f64, b: f64| (b > 0.0).then(|| (a / b - 1.0) * 100.0);
    json!({
        "ath": sig6(ath),
        "from_ath": pct(last.close, ath),
        "high_52w": sig6(high_52),
        "low_52w": sig6(low_52),
        "change_12m": change(weekly, 52),
        "trend_10m": sma10.map(|s| if last.close > s { "alta" } else { "baixa" }),
        "years": weekly.len() / 52,
    })
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

/// The Mercados tab: every watchlist asset, plus the other US stocks
/// while they have a signal or an open trade.
fn markets_json(now_ms: i64, loaded: &[(&Loaded, Vec<Chart>, bool)]) -> Value {
    let assets: Vec<Value> = loaded
        .iter()
        .filter_map(|(l, c, listed)| {
            let stormer = items_json(&l.item.symbol, c, &|tf, p, side, trend| {
                market_role(&l.item.group, tf, p, side, trend)
            });
            let live = stormer.as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|i| i["status"] == "sinal" || i["status"] == "aberto")
            });
            (*listed || live).then_some((l, stormer))
        })
        .map(|(l, stormer)| {
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
                "monthly": candles(&monthly(&l.weekly), MONTHLY_SENT),
                "long": long_view(&l.weekly),
                "stormer": stormer,
            })
        })
        .collect();
    json!({ "version": 1, "generated_ms": now_ms, "assets": assets })
}

/// The Mercados tab list.
pub const WATCHLIST: &str = "config/watchlist.csv";
/// The larger list searched for trades.
pub const TRADE_UNIVERSE: &str = "config/trade_universe.csv";

pub(crate) fn read_list(path: &str) -> Result<Vec<Item>, Box<dyn std::error::Error>> {
    let items =
        parse_watchlist(&std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?);
    if items.is_empty() {
        return Err(format!("{path} has no valid lines").into());
    }
    Ok(items)
}

fn watchlist() -> Result<Vec<Item>, Box<dyn std::error::Error>> {
    read_list(WATCHLIST)
}

pub(crate) fn now_ms() -> Result<i64, Box<dyn std::error::Error>> {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    Ok(i64::try_from(since.as_millis())?)
}

/// Fetches the watchlist and the US part of the trade universe, follows
/// the stock trades and publishes the Mercados tab. With `notify`, new
/// stages go to Telegram.
///
/// # Errors
///
/// Returns an error when the watchlist is missing, Telegram refuses a
/// message or a file cannot be saved.
#[allow(clippy::too_many_lines)] // fetch, follow, send, save and publish in order
pub async fn cmd_markets(data_dir: &Path, dry_run: bool, notify: bool) -> CliResult {
    let now = now_ms()?;
    let listed = watchlist()?;
    let listed_symbols: BTreeSet<&str> = listed.iter().map(|i| i.symbol.as_str()).collect();
    let extra: Vec<Item> = match read_list(TRADE_UNIVERSE) {
        Ok(items) => items
            .into_iter()
            .filter(|i| i.group == "EUA" && !listed_symbols.contains(i.symbol.as_str()))
            .collect(),
        Err(e) => {
            eprintln!("warning: {e}; só a lista do painel");
            Vec::new()
        }
    };
    let all: Vec<Item> = listed.iter().cloned().chain(extra).collect();
    let loaded = load_all(&all, now).await;
    if loaded.is_empty() {
        return Err("no market data: Yahoo Finance did not answer for any symbol".into());
    }
    let scanned: Vec<(&Loaded, Vec<Chart>, bool)> = loaded
        .iter()
        .map(|l| {
            (
                l,
                charts(l),
                listed_symbols.contains(l.item.symbol.as_str()),
            )
        })
        .collect();

    let live_dir = data_dir.join("live");
    let sent_path = live_dir.join(stock_trades::SENT_FILE);
    // The first run would report every trade still open from weeks ago:
    // only the current signals are sent, the rest is just marked as seen.
    let first_run = !sent_path.exists();
    let sent: BTreeSet<String> = std::fs::read_to_string(&sent_path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    let mut books = smc_live::Books::load(&live_dir, now);
    let stocks: Vec<Stock> = scanned
        .iter()
        .filter(|(l, _, _)| l.item.group == "EUA")
        .map(|(l, c, _)| Stock {
            symbol: &l.item.symbol,
            name: &l.item.name,
            charts: c,
        })
        .collect();
    let found = stock_trades::run(&stocks, now, &sent, &mut books);
    let mut events = Vec::new();
    for (name, stage) in found.stages {
        if first_run && !stage.key.ends_with("|sinal") {
            continue;
        }
        println!("{}", stage.message);
        events.push(dashboard::Event {
            id: stage.key,
            ms: now,
            asset: name,
            kind: "stormer",
            text: stage.message,
            notify: stage.notify,
            market: "acoes",
            ..Default::default()
        });
    }
    let data = markets_json(now, &scanned);
    if dry_run {
        println!(
            "dry run: {} ativos carregados, {} avisos de ações, nada salvo nem enviado",
            loaded.len(),
            events.len()
        );
        return Ok(std::process::ExitCode::SUCCESS);
    }
    let mut messages: Vec<String> = events
        .iter()
        .filter(|e| e.notify)
        .map(|e| e.text.clone())
        .collect();
    let new_signals: Vec<&stock_watch::Watched> = found
        .watch
        .iter()
        .filter(|w| {
            events
                .iter()
                .any(|e| e.notify && e.id == format!("{}|sinal", w.key))
        })
        .collect();
    if let Some(summary) = signals_summary(&new_signals) {
        messages.insert(0, summary);
    }
    if notify && !messages.is_empty() {
        let env = parse_env(&std::fs::read_to_string(".env")?);
        let (Some(token), Some(chat)) =
            (env.get("TELEGRAM_BOT_TOKEN"), env.get("TELEGRAM_CHAT_ID"))
        else {
            return Err("TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID must be set in .env".into());
        };
        for (i, text) in smc_live::pack_messages(&messages, 3800).iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            }
            send_telegram(token, chat, text).await?;
        }
        println!("telegram: {} avisos de ações enviados", messages.len());
    }
    std::fs::create_dir_all(&live_dir)?;
    let trades = found.rows.iter().fold(String::new(), |mut out, r| {
        let _ = writeln!(out, "{}", r.csv());
        out
    });
    smc_live::save(&live_dir, &books, "", &trades)?;
    if !found.keys.is_empty() {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&sent_path)?;
        std::io::Write::write_all(&mut f, (found.keys.join("\n") + "\n").as_bytes())?;
    }
    let watch: Vec<Value> = found
        .watch
        .iter()
        .map(stock_watch::Watched::to_json)
        .collect();
    std::fs::write(
        live_dir.join(stock_watch::WATCH_FILE),
        Value::Array(watch).to_string(),
    )?;
    dashboard::append_events(&live_dir, &events)?;
    let fresh: Vec<dashboard::Event> = events.into_iter().filter(|e| e.notify).collect();
    dashboard::save_and_publish(&live_dir, MARKETS_FILE, "markets", &data, &fresh).await?;
    Ok(std::process::ExitCode::SUCCESS)
}

/// One opening message when several stocks give a signal the same night:
/// what they have in common and which one the paper book takes first.
fn signals_summary(signals: &[&stock_watch::Watched]) -> Option<String> {
    if signals.len() < 2 {
        return None;
    }
    let mut sorted = signals.to_vec();
    // The book takes the daily setup first, then the weekly one.
    sorted.sort_by_key(|w| u8::from(w.tf != "1d"));
    let mut text = format!(
        "AÇÕES DOS EUA: {} possíveis compras para o próximo pregão.\nTodas são o mesmo setup (o preço passou abaixo de um fundo e voltou), que ganhou no teste de 10 anos. Nenhuma é compra agora: cada uma só vale se o preço passar do gatilho.\nA conta fictícia entra em uma por dia, na primeira que passar do gatilho; o gráfico diário tem preferência.\nLista:",
        sorted.len()
    );
    for w in sorted {
        let _ = write!(
            text,
            "\n- {}: acima de {} (gráfico {}), stop {}, alvo {}",
            w.name,
            super::fmt_pt(w.trigger),
            if w.tf == "1w" { "semanal" } else { "diário" },
            super::fmt_pt(w.stop),
            super::fmt_pt(w.target)
        );
    }
    text.push_str("\nOs detalhes de cada uma vêm nas mensagens abaixo. Durante o pregão, aviso quando alguma passar do gatilho.");
    Some(text)
}

/// The Stormer history test on the watchlist, each group pooled.
///
/// # Errors
///
/// Returns an error when the watchlist is missing.
pub async fn cmd_market_study(list: &str) -> CliResult {
    let now = now_ms()?;
    let split_ms = held_out_start()?
        .and_hms_opt(23, 59, 59)
        .ok_or("invalid date")?
        .and_utc()
        .timestamp_millis();
    let loaded = load_all(&read_list(list)?, now).await;
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
    Chart::round_the_clock("", DAY_MS, Vec::new(), (Vec::new(), Vec::new()))
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
            market_role("EUA", "1d", Pattern::Trap, Side::Long, true),
            Some(Role::Candidate)
        );
        // Below the base on the 84 US assets.
        assert_eq!(
            market_role("EUA", "1w", Pattern::OneTwoThreeInside, Side::Long, true),
            None
        );
        // Nothing beat the base on the 62 Brazilian assets.
        assert_eq!(
            market_role("Brasil", "1w", Pattern::Trap, Side::Long, false),
            None
        );
        assert_eq!(
            market_role("Brasil", "1d", Pattern::GapTrap, Side::Long, true),
            None
        );
        assert_eq!(
            market_role("EUA", "1w", Pattern::OneTwoThree, Side::Short, true),
            None
        );
    }

    #[test]
    fn a_us_daily_candle_closes_the_same_afternoon_and_expires_after_the_weekend() {
        let friday = 1_762_525_800_000; // 2025-11-07 14:30 UTC (09:30 New York), a Friday
        let bar = |ms: i64| Bar {
            open_time_ms: ms,
            open: 1.0,
            high: 1.0,
            low: 1.0,
            close: 1.0,
            volume: 1.0,
        };
        let l = Loaded {
            item: Item {
                group: "EUA".to_owned(),
                symbol: "SPY".to_owned(),
                name: "SPY".to_owned(),
            },
            daily: vec![bar(friday)],
            // Monday 05:00 UTC, midnight in New York.
            weekly: vec![bar(friday - 4 * DAY_MS - 9 * 3600 * 1000 - 1_800_000)],
        };
        let c = charts(&l);
        // Closes 21:00 UTC, 16:00 in New York.
        assert_eq!(c[0].close_ms(0), friday + 6 * 3600 * 1000 + 1_800_000);
        // Valid through Monday's close, not Saturday's.
        assert_eq!(c[0].deadline_ms(0), c[0].close_ms(0) + 3 * DAY_MS);
        // The week closes on Friday.
        assert_eq!(c[1].close_ms(0), c[0].close_ms(0));
    }

    #[test]
    fn several_signals_get_one_summary_with_the_daily_first() {
        let w = |name: &str, tf: &str| stock_watch::Watched {
            key: format!("{name}|{tf}"),
            symbol: name.to_owned(),
            name: name.to_owned(),
            tf: tf.to_owned(),
            status: "sinal".to_owned(),
            trigger: 10.0,
            stop: 9.0,
            target: 12.0,
            from_ms: 0,
            until_ms: Some(1),
            in_book: false,
        };
        let (a, b) = (w("Oracle", "1w"), w("Texas", "1d"));
        assert!(signals_summary(&[&a]).is_none());
        let text = signals_summary(&[&a, &b]).unwrap();
        assert!(
            text.starts_with("AÇÕES DOS EUA: 2 possíveis compras"),
            "{text}"
        );
        assert!(
            text.find("Texas").unwrap() < text.find("Oracle").unwrap(),
            "{text}"
        );
    }

    #[test]
    fn months_and_the_long_view_come_from_weeks() {
        let week = |day: i64, close: f64| Bar {
            open_time_ms: day * DAY_MS,
            open: close,
            high: close + 1.0,
            low: close - 1.0,
            close,
            volume: 1.0,
        };
        // 2025-01-06, -13, -20, -27 and 2025-02-03 (days since 1970).
        let weeks: Vec<Bar> = [20094, 20101, 20108, 20115, 20122]
            .iter()
            .zip([10.0, 12.0, 11.0, 13.0, 9.0])
            .map(|(d, c)| week(*d, c))
            .collect();
        let m = monthly(&weeks);
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].high, m[0].low, m[0].close), (14.0, 9.0, 13.0));
        let v = long_view(&weeks);
        assert_eq!(v["ath"], 14.0);
        assert!((v["from_ath"].as_f64().unwrap() - (9.0 / 14.0 - 1.0) * 100.0).abs() < 1e-9);
        assert!(v["trend_10m"].is_null());
    }
}
