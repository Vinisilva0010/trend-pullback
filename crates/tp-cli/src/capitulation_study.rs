//! `tp capitulation-study`: buying a capitulation, the way it is usually
//! described, turned into fixed rules.
//!
//! Part 1, the capitulation candle, on BTC, ETH, SOL and every coin of the
//! trade universe (only in the months it was in it):
//! - wick: a day that falls 15% (or 25%) below the previous close and closes
//!   back above the middle of that fall; buy at the next open;
//! - drop and recovery: a day that closes 12% or more down, followed by a
//!   green day that takes back at least 60% of it; buy at the open after it;
//! - each also with the Fear & Greed Index at 20 or less that day.
//!
//! Part 2, the three gauges, on BTC, ETH and SOL: the Fear & Greed Index at
//! 20 or less (or 10), the price 50% or more below its all-time high, a year
//! or more since that high. These are few episodes, so every one is listed.
//!
//! Every buy is held 7, 30, 90 and 365 days and compared with buying the
//! same coin on any day of the same period (the base). One event per coin
//! every 30 days, so one long fall does not count many times. A coin whose
//! history ends early (delisted) keeps its last price, so the coins that
//! died count as the losses they were.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, NaiveDate};
use tp_core::intraday::Bar;
use tp_data::fear_greed::fetch;
use tp_data::history::{processed_dir, read_series};

use super::crypto_study::Universe;
use super::{CliResult, count_f64, daily_bars, held_out_start};

const BIG: [&str; 3] = ["BTCUSDT", "ETHUSDT", "SOLUSDT"];
const DAY_MS: i64 = 24 * 3600 * 1000;
const HORIZONS: [usize; 4] = [7, 30, 90, 365];
const COOLDOWN: usize = 30;
const COST: f64 = 0.002;

/// One coin's daily candles.
struct Coin {
    symbol: String,
    bars: Vec<Bar>,
    /// Still traded at the end of the data (not delisted).
    alive: bool,
}

/// Return in percent from the open of `entry` to the close `h` days later,
/// with costs. When the history ends first: `None` for a coin still traded
/// (the future is not known yet), the last close for a delisted one.
fn forward(bars: &[Bar], entry: usize, h: usize, alive: bool) -> Option<f64> {
    let open = bars.get(entry)?.open;
    if alive && entry + h >= bars.len() {
        return None;
    }
    let last = bars.get((entry + h).min(bars.len() - 1))?.close;
    (open > 0.0).then(|| (last / open - 1.0 - COST) * 100.0)
}

/// Deepest fall in percent below the entry within 30 days.
fn worst_after(bars: &[Bar], entry: usize) -> Option<f64> {
    let open = bars.get(entry)?.open;
    let low = bars[entry..(entry + 30).min(bars.len())]
        .iter()
        .map(|b| b.low)
        .fold(f64::MAX, f64::min);
    Some((low / open - 1.0) * 100.0)
}

fn day_of(ms: i64) -> Option<NaiveDate> {
    DateTime::from_timestamp_millis(ms).map(|d| d.date_naive())
}

/// The capitulation candles.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Candle {
    /// Falls `depth` below the previous close, closes above the middle.
    Wick(f64),
    /// Closes 12% down, the next day is green and takes back 60%.
    DropAndRecovery,
}

impl Candle {
    fn label(self) -> String {
        match self {
            Self::Wick(d) => format!(
                "Pavio de capitulação: o dia cai {:.0}% ou mais abaixo do fechamento anterior e fecha acima da metade da queda; compra na abertura seguinte",
                d * 100.0
            ),
            Self::DropAndRecovery => "Queda e recuperação: um dia fecha 12% ou mais em queda e o dia seguinte é verde e devolve 60% ou mais da queda; compra na abertura depois dele".to_owned(),
        }
    }

    /// Index of the entry candle when day `t` completes the pattern.
    fn entry(self, bars: &[Bar], t: usize) -> Option<usize> {
        let prev = bars.get(t.checked_sub(1)?)?.close;
        let b = bars.get(t)?;
        match self {
            Self::Wick(depth) => {
                let fall = prev - b.low;
                (b.low <= prev * (1.0 - depth) && b.close >= b.low + 0.5 * fall).then_some(t + 1)
            }
            Self::DropAndRecovery => {
                let next = bars.get(t + 1)?;
                let drop = prev - b.close;
                (b.close <= prev * 0.88
                    && next.close > next.open
                    && next.close - b.close >= 0.6 * drop)
                    .then_some(t + 2)
            }
        }
    }
}

/// Result of one event at every horizon.
struct Event {
    entry_ms: i64,
    returns: Vec<Option<f64>>,
    excess: Vec<Option<f64>>,
    worst: f64,
}

/// The middle value (median), or 0 for none.
fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        f64::midpoint(v[n / 2 - 1], v[n / 2])
    }
}

/// Median forward return of buying any day of each period, per horizon,
/// only on days `eligible` allows. `(dev, final)`. The median, because a
/// coin that went up 100 times in a year would make the mean of every day
/// meaningless.
fn base(
    bars: &[Bar],
    alive: bool,
    split_ms: i64,
    eligible: &dyn Fn(usize) -> bool,
) -> Vec<(f64, f64)> {
    HORIZONS
        .iter()
        .map(|h| {
            let (mut dev, mut fin) = (Vec::new(), Vec::new());
            for i in 1..bars.len() {
                if !eligible(i) {
                    continue;
                }
                if let Some(r) = forward(bars, i, *h, alive) {
                    if bars[i].open_time_ms <= split_ms {
                        dev.push(r);
                    } else {
                        fin.push(r);
                    }
                }
            }
            (median(&dev), median(&fin))
        })
        .collect()
}

fn event(
    bars: &[Bar],
    alive: bool,
    entry: usize,
    base: &[(f64, f64)],
    split_ms: i64,
) -> Option<Event> {
    let entry_ms = bars.get(entry)?.open_time_ms;
    let returns: Vec<Option<f64>> = HORIZONS
        .iter()
        .map(|h| forward(bars, entry, *h, alive))
        .collect();
    let excess = returns
        .iter()
        .zip(base)
        .map(|(r, (d, f))| r.map(|r| r - if entry_ms <= split_ms { d } else { f }))
        .collect();
    Some(Event {
        entry_ms,
        returns,
        excess,
        worst: worst_after(bars, entry)?,
    })
}

/// How much further the price fell below the purchase, as "24,9%".
fn fell(worst: f64) -> String {
    format!("{:.1}%", (-worst).max(0.0)).replace('.', ",")
}

fn pct(v: f64) -> String {
    format!("{v:+.1}%").replace('.', ",")
}

/// Prints one rule and says whether it passed at 30 and 90 days.
fn report(label: &str, events: &[Event], split_ms: i64) -> bool {
    println!();
    println!(" {label}");
    if events.is_empty() {
        println!("   Nenhum caso.");
        return false;
    }
    let mut worst: Vec<f64> = events.iter().map(|e| e.worst).collect();
    worst.sort_by(f64::total_cmp);
    println!(
        "   {} casos. Depois da compra, nos 30 dias seguintes, o preço ainda caiu tipicamente mais {} abaixo da compra; em 1 de cada 10 casos, mais de {}.",
        events.len(),
        fell(worst[worst.len() / 2]),
        fell(worst[worst.len() / 10]),
    );
    let mut dates: Vec<i64> = events.iter().map(|e| e.entry_ms / DAY_MS).collect();
    dates.sort_unstable();
    dates.dedup();
    println!(
        "   Os casos caíram em {} dias diferentes: casos no mesmo dia em moedas diferentes são o mesmo movimento do mercado, não chances independentes.",
        dates.len()
    );
    let mut passed = true;
    for (k, h) in HORIZONS.iter().enumerate() {
        let all: Vec<f64> = events.iter().filter_map(|e| e.returns[k]).collect();
        if all.is_empty() {
            continue;
        }
        // Share of cases that did better than the coin's typical day.
        let beat = |late: bool| -> (usize, usize) {
            let v: Vec<f64> = events
                .iter()
                .filter(|e| (e.entry_ms > split_ms) == late)
                .filter_map(|e| e.excess[k])
                .collect();
            (v.iter().filter(|x| **x > 0.0).count(), v.len())
        };
        let ((dev_k, dev_n), (fin_k, fin_n)) = (beat(false), beat(true));
        let share = |k: usize, n: usize| count_f64(k) / count_f64(n.max(1)) * 100.0;
        // Sign test: how far above half the share is, in standard errors.
        let z = if dev_n > 0 {
            (count_f64(dev_k) - count_f64(dev_n) / 2.0) / (count_f64(dev_n) / 4.0).sqrt()
        } else {
            0.0
        };
        let proven = dev_n >= 30 && z >= 2.0 && fin_n >= 10 && fin_k * 2 > fin_n;
        if *h == 30 || *h == 90 {
            passed &= proven;
        }
        let wins = all.iter().filter(|r| **r > 0.0).count();
        println!(
            "   {h} dias: caso típico {}, subiu em {:.0}% dos casos. Ganhou do dia típico da mesma moeda em {:.0}% dos casos até out/2024 ({dev_n} casos) e em {:.0}% depois ({fin_n} casos).{}",
            pct(median(&all)),
            count_f64(wins) / count_f64(all.len()) * 100.0,
            share(dev_k, dev_n),
            share(fin_k, fin_n),
            if proven { " GANHOU COM PROVA." } else { "" }
        );
    }
    let verdict = if passed {
        "PASSOU em 30 e 90 dias"
    } else {
        "não passou (precisa ganhar do dia típico na maioria dos casos, com prova, em 30 e 90 dias, nos dois períodos)"
    };
    println!("   {verdict}.");
    passed
}

/// Events of a rule over some coins, one per coin every [`COOLDOWN`] days.
fn candle_events(
    coins: &[Coin],
    universe: Option<&Universe>,
    rule: Candle,
    fear: &BTreeMap<NaiveDate, u8>,
    fear_max: Option<u8>,
    split_ms: i64,
) -> Vec<Event> {
    let mut out = Vec::new();
    for c in coins {
        let member = |i: usize| universe.is_none_or(|u| u.has(&c.symbol, c.bars[i].open_time_ms));
        let base = base(&c.bars, c.alive, split_ms, &member);
        let mut next_free = 0;
        for t in 1..c.bars.len() {
            if t < next_free || !member(t) {
                continue;
            }
            let Some(entry) = rule.entry(&c.bars, t) else {
                continue;
            };
            if let Some(max) = fear_max {
                let scared = day_of(c.bars[t].open_time_ms)
                    .and_then(|d| fear.get(&d))
                    .is_some_and(|v| *v <= max);
                if !scared {
                    continue;
                }
            }
            if let Some(e) = event(&c.bars, c.alive, entry, &base, split_ms) {
                out.push(e);
                next_free = entry + COOLDOWN;
            }
        }
    }
    out
}

/// The gauges on one coin: the first day of each episode.
fn gauge_days(
    coin: &Coin,
    fear: &BTreeMap<NaiveDate, u8>,
    test: &dyn Fn(Option<u8>, f64, i64) -> bool,
) -> Vec<usize> {
    let mut out = Vec::new();
    let mut high = f64::MIN;
    let mut high_ms = 0;
    let mut next_free = 0;
    for (i, b) in coin.bars.iter().enumerate() {
        if b.high > high {
            high = b.high;
            high_ms = b.open_time_ms;
        }
        let f = day_of(b.open_time_ms).and_then(|d| fear.get(&d).copied());
        let drawdown = 1.0 - b.close / high;
        let days = (b.open_time_ms - high_ms) / DAY_MS;
        if i >= next_free && i + 1 < coin.bars.len() && test(f, drawdown, days) {
            out.push(i + 1);
            next_free = i + 1 + COOLDOWN;
        }
    }
    out
}

fn load(data_dir: &Path, symbol: &str) -> Option<Coin> {
    let candles = read_series(&processed_dir(data_dir).join(format!("{symbol}.csv"))).ok()?;
    let bars = daily_bars(&candles);
    (bars.len() > 60).then(|| Coin {
        symbol: symbol.to_owned(),
        bars,
        alive: true,
    })
}

/// A gauge: its name and the test on (Fear & Greed, drawdown, days since the high).
type Gauge<'a> = (&'a str, Box<dyn Fn(Option<u8>, f64, i64) -> bool>);

/// Runs the study and prints it.
///
/// # Errors
///
/// Returns an error when the BTC history is missing.
#[allow(clippy::too_many_lines)] // the two parts of the report, in order
pub async fn cmd_capitulation_study(data_dir: &Path) -> CliResult {
    let split_ms = held_out_start()?
        .and_hms_opt(23, 59, 59)
        .ok_or("invalid date")?
        .and_utc()
        .timestamp_millis();
    println!(
        "Compra na capitulação, em regras fixas. Cada compra é medida 7, 30, 90 e 365 dias depois, já com 0,2% de custo, e comparada com o dia típico da mesma moeda no mesmo período (a mediana de comprar em qualquer dia). Um caso por moeda a cada 30 dias. Moedas cujo histórico acaba (saíram da corretora) ficam com o último preço, então as que morreram contam como as perdas que foram."
    );
    let fear: BTreeMap<NaiveDate, u8> = match fetch(0).await {
        Ok(r) => {
            println!(
                "Índice de Medo e Ganância (alternative.me): {} dias, de {} a {}.",
                r.len(),
                r.first()
                    .map_or(String::new(), |x| x.date.format("%d/%m/%Y").to_string()),
                r.last()
                    .map_or(String::new(), |x| x.date.format("%d/%m/%Y").to_string()),
            );
            r.into_iter().map(|x| (x.date, x.value)).collect()
        }
        Err(e) => {
            println!(
                "Índice de Medo e Ganância indisponível ({e}); as partes que usam o índice ficam sem casos."
            );
            BTreeMap::new()
        }
    };
    let mut big = Vec::new();
    for s in BIG {
        big.push(load(data_dir, s).ok_or_else(|| format!("{s}: daily history missing"))?);
    }
    let universe = super::crypto_study::load(data_dir).ok();
    let mut all: Vec<Coin> = Vec::new();
    if let Some(u) = &universe {
        let symbols: std::collections::BTreeSet<&String> = u.symbols().collect();
        for s in symbols {
            if !BIG.contains(&s.as_str())
                && let Some(c) = load(data_dir, s)
            {
                all.push(c);
            }
        }
    }
    let others = all.len();
    // A coin whose history stops more than 3 days before the newest one was delisted.
    let newest = all
        .iter()
        .chain(&big)
        .filter_map(|c| c.bars.last())
        .map(|b| b.open_time_ms)
        .max()
        .unwrap_or(0);
    for c in &mut all {
        c.alive = c
            .bars
            .last()
            .is_some_and(|b| b.open_time_ms >= newest - 3 * DAY_MS);
    }
    for s in BIG {
        if let Some(c) = load(data_dir, s) {
            all.push(c);
        }
    }

    println!();
    println!(
        "PARTE 1: a barra de capitulação, no BTC, ETH, SOL e nas outras {others} moedas do universo (cada uma só nos meses em que estava nele)."
    );
    let mut good = Vec::new();
    let rules = [
        Candle::Wick(0.15),
        Candle::Wick(0.25),
        Candle::DropAndRecovery,
    ];
    for rule in rules {
        for fear_max in [None, Some(20)] {
            let label = match fear_max {
                None => rule.label(),
                Some(m) => format!(
                    "{} — só com o Medo e Ganância em {m} ou menos no dia",
                    rule.label()
                ),
            };
            let events = candle_events(&all, universe.as_ref(), rule, &fear, fear_max, split_ms);
            if report(&label, &events, split_ms) {
                good.push(label);
            }
        }
    }

    println!();
    println!("PARTE 2: as réguas, no BTC, ETH e SOL. Poucos casos: cada um é listado.");
    let gauges: Vec<Gauge> = vec![
        (
            "Medo e Ganância em 20 ou menos",
            Box::new(|f, _, _| f.is_some_and(|v| v <= 20)),
        ),
        (
            "Medo e Ganância em 10 ou menos",
            Box::new(|f, _, _| f.is_some_and(|v| v <= 10)),
        ),
        (
            "Preço 50% ou mais abaixo da máxima histórica",
            Box::new(|_, d, _| d >= 0.5),
        ),
        (
            "As três juntas: Medo e Ganância em 20 ou menos, 50% ou mais abaixo da máxima e 1 ano ou mais desde ela",
            Box::new(|f, d, days| f.is_some_and(|v| v <= 20) && d >= 0.5 && days >= 365),
        ),
    ];
    for (name, test) in &gauges {
        println!();
        println!(" {name}");
        for coin in &big {
            let member = |_: usize| true;
            let base = base(&coin.bars, coin.alive, split_ms, &member);
            let days = gauge_days(coin, &fear, test.as_ref());
            let events: Vec<Event> = days
                .iter()
                .filter_map(|e| event(&coin.bars, coin.alive, *e, &base, split_ms))
                .collect();
            let short = coin.symbol.trim_end_matches("USDT");
            if events.is_empty() {
                println!("   {short}: nenhum caso.");
                continue;
            }
            let lines: Vec<String> = events
                .iter()
                .map(|e| {
                    let date = DateTime::from_timestamp_millis(e.entry_ms)
                        .map_or(String::new(), |d| d.format("%d/%m/%Y").to_string());
                    let r = |k: usize| e.returns[k].map_or("ainda sem dado".to_owned(), pct);
                    format!(
                        "{date}: 30 dias {}, 1 ano {}; nos 30 dias seguintes o preço chegou a {} abaixo da compra",
                        r(1),
                        r(3),
                        fell(e.worst)
                    )
                })
                .collect();
            let beat = |k: usize| {
                let v: Vec<f64> = events.iter().filter_map(|e| e.excess[k]).collect();
                (v.iter().filter(|x| **x > 0.0).count(), v.len())
            };
            let (b30, n30) = beat(1);
            let (b365, n365) = beat(3);
            println!(
                "   {short}: {} casos. Ganhou do dia típico do {short} em {b30} de {n30} casos em 30 dias e em {b365} de {n365} em 1 ano.",
                events.len(),
            );
            for l in lines {
                println!("     {l}");
            }
        }
    }

    println!();
    println!("RESUMO: barras de capitulação que passaram em 30 e 90 dias");
    if good.is_empty() {
        println!(" Nenhuma.");
    }
    for g in good {
        println!(" {g}");
    }
    println!(
        "As réguas da parte 2 têm poucos casos e não dão prova; servem para ver como foram as outras vezes."
    );
    Ok(std::process::ExitCode::SUCCESS)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    fn bar(day: i64, open: f64, high: f64, low: f64, close: f64) -> Bar {
        Bar {
            open_time_ms: day * DAY_MS,
            open,
            high,
            low,
            close,
            volume: 1.0,
        }
    }

    #[test]
    fn the_median_ignores_one_huge_year() {
        assert_eq!(median(&[1.0, 2.0, 10_000.0]), 2.0);
        assert_eq!(median(&[1.0, 3.0]), 2.0);
        assert_eq!(median(&[]), 0.0);
    }

    #[test]
    fn the_capitulation_candles_are_recognized() {
        let bars = vec![
            bar(0, 100.0, 101.0, 99.0, 100.0),
            // Falls to 80 (20% below 100), closes at 92: above the middle (90).
            bar(1, 99.0, 99.0, 80.0, 92.0),
            bar(2, 92.0, 95.0, 90.0, 94.0),
        ];
        assert_eq!(Candle::Wick(0.15).entry(&bars, 1), Some(2));
        assert_eq!(Candle::Wick(0.25).entry(&bars, 1), None);
        let drop = vec![
            bar(0, 100.0, 101.0, 99.0, 100.0),
            bar(1, 99.0, 99.0, 84.0, 86.0),
            // Green, takes back 10 of the 14 lost (71%).
            bar(2, 87.0, 97.0, 86.0, 96.0),
            bar(3, 96.0, 98.0, 95.0, 97.0),
        ];
        assert_eq!(Candle::DropAndRecovery.entry(&drop, 1), Some(3));
    }

    #[test]
    fn returns_keep_the_last_price_when_the_history_ends() {
        let bars = vec![bar(0, 10.0, 10.0, 10.0, 10.0), bar(1, 10.0, 10.0, 2.0, 2.0)];
        // 365 days later does not exist: a delisted coin keeps its last close (2);
        // for a coin still traded the result is not known yet.
        assert_eq!(forward(&bars, 0, 365, true), None);
        let r = forward(&bars, 0, 365, false).unwrap();
        assert!((r - (-80.0 - COST * 100.0)).abs() < 1e-9, "{r}");
        assert!((worst_after(&bars, 0).unwrap() + 80.0).abs() < 1e-9);
    }

    #[test]
    fn gauges_take_the_first_day_of_each_episode() {
        let coin = Coin {
            symbol: "BTCUSDT".to_owned(),
            bars: (0..100)
                .map(|d| bar(d, 100.0, 100.0, 99.0, 100.0))
                .collect(),
            alive: true,
        };
        let fear: BTreeMap<NaiveDate, u8> = (0..100)
            .filter_map(|d| {
                Some((
                    day_of(d * DAY_MS)?,
                    if (10..20).contains(&d) || (60..70).contains(&d) {
                        15
                    } else {
                        50
                    },
                ))
            })
            .collect();
        let days = gauge_days(&coin, &fear, &|f, _, _| f.is_some_and(|v| v <= 20));
        assert_eq!(days, vec![11, 61]);
    }
}
