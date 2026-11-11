//! `tp runner-study`: setups that let winners run, looking for trades of 5,
//! 10 or more times the risk. No fixed target: the stop follows the price
//! (highest high since entry minus 3 ATRs) and only moves up. Most trades
//! lose a little; the question is whether the few big ones pay for them,
//! and whether that beats entering on any candle with the same stop and
//! the same trailing exit.

use std::path::Path;

use tp_core::intraday::Bar;
use tp_core::runner::{atr, breakouts, ride};
use tp_core::stormer::{Params, Pattern, Side, scan, weekly};
use tp_data::history::{processed_dir, read_series};

use super::{CliResult, count_f64, daily_bars, held_out_start, r_mean_t, r_pt};
use crate::markets::{self, Loaded};

/// Costs per trade, as in the other studies.
const COST: f64 = 0.002;
/// Trailing stop distance in ATRs (14 candles).
const MULT: f64 = 3.0;
/// Longest hold: five years on the weekly chart, three on the daily.
const WEEKLY_HOLD: usize = 260;
const DAILY_HOLD: usize = 750;
const WEEK_MS: i64 = 7 * 24 * 3600 * 1000;

/// The setups of this study.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setup {
    WeeklyTrap,
    DailyTrapTrend,
    Weekly52High,
    WeeklyAllTimeHigh,
}

impl Setup {
    const ALL: [Self; 4] = [
        Self::WeeklyTrap,
        Self::DailyTrapTrend,
        Self::Weekly52High,
        Self::WeeklyAllTimeHigh,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::WeeklyTrap => "trap de compra no semanal",
            Self::DailyTrapTrend => "trap de compra no diário, a favor da tendência",
            Self::Weekly52High => "fechamento semanal na maior alta de 52 semanas",
            Self::WeeklyAllTimeHigh => "fechamento semanal na máxima histórica",
        }
    }

    fn weekly(self) -> bool {
        self != Self::DailyTrapTrend
    }
}

/// (entry time, result in R) of every closed trade.
type Rows = Vec<(i64, f64)>;

/// Entries of a setup on one chart: (entry candle, entry price, stop).
fn entries(setup: Setup, bars: &[Bar], a: &[Option<f64>]) -> Vec<(usize, f64, f64)> {
    match setup {
        Setup::WeeklyTrap | Setup::DailyTrapTrend => {
            let hold = if setup.weekly() { 12 } else { 30 };
            let (trades, _) = scan(bars, Params::with_max_hold(hold));
            trades
                .iter()
                .filter(|t| t.pattern == Pattern::Trap && t.side == Side::Long)
                .filter(|t| setup == Setup::WeeklyTrap || t.with_trend)
                .map(|t| (t.entry_index, t.entry, t.stop))
                .collect()
        }
        Setup::Weekly52High | Setup::WeeklyAllTimeHigh => {
            let found = if setup == Setup::Weekly52High {
                breakouts(bars, 52, 52)
            } else {
                breakouts(bars, 0, 156)
            };
            found
                .into_iter()
                .filter_map(|i| {
                    let k = i + 1;
                    let entry = bars.get(k)?.open;
                    let stop = entry - 2.0 * a.get(i).copied().flatten()?;
                    Some((k, entry, stop))
                })
                .collect()
        }
    }
}

/// Rides every entry, one position per chart at a time.
fn run(
    bars: &[Bar],
    a: &[Option<f64>],
    picks: &[(usize, f64, f64)],
    hold: usize,
) -> (Rows, Vec<f64>) {
    let mut rows = Vec::new();
    let mut risks = Vec::new();
    let mut busy_until = 0;
    for &(k, entry, stop) in picks {
        if k < busy_until {
            continue;
        }
        let Some(r) = ride(bars, a, k, entry, stop, MULT, hold, COST) else {
            continue;
        };
        busy_until = r.exit_index + 1;
        if r.open {
            continue;
        }
        risks.push((entry - stop) / entry);
        rows.push((bars[k].open_time_ms, r.r));
    }
    (rows, risks)
}

/// The base: enter at the open of every `every`-th candle with the given
/// stop distance (share of the entry) and the same trailing exit.
fn base(bars: &[Bar], a: &[Option<f64>], risk: f64, every: usize, hold: usize) -> Rows {
    let picks: Vec<(usize, f64, f64)> = (1..bars.len())
        .step_by(every)
        .map(|k| (k, bars[k].open, bars[k].open * (1.0 - risk)))
        .collect();
    run(bars, a, &picks, hold).0
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len() / 2])
}

fn share(rows: &Rows, min_r: f64) -> String {
    let n = rows.iter().filter(|r| r.1 >= min_r).count();
    format!(
        "{}%",
        format!("{:.1}", count_f64(n) / count_f64(rows.len()) * 100.0).replace('.', ",")
    )
}

fn split(rows: &Rows, split_ms: i64) -> (Vec<f64>, Vec<f64>) {
    (
        rows.iter()
            .filter(|r| r.0 <= split_ms)
            .map(|r| r.1)
            .collect(),
        rows.iter()
            .filter(|r| r.0 > split_ms)
            .map(|r| r.1)
            .collect(),
    )
}

/// One result line and whether it passed (positive and above the base in
/// both periods).
fn line(label: &str, rows: &mut Rows, base_rows: &Rows, split_ms: i64) -> (String, bool) {
    if rows.is_empty() {
        return (format!("   {label}: nenhum trade."), false);
    }
    rows.sort_by_key(|r| r.0);
    let (dev, fin) = split(rows, split_ms);
    let (bdev, bfin) = split(base_rows, split_ms);
    let all: Vec<f64> = rows.iter().map(|r| r.1).collect();
    let (mean, _) = r_mean_t(&all);
    let (dev_mean, dev_t) = r_mean_t(&dev);
    let (fin_mean, _) = r_mean_t(&fin);
    let (bdev_mean, _) = r_mean_t(&bdev);
    let (bfin_mean, _) = r_mean_t(&bfin);
    let best = all.iter().copied().fold(f64::MIN, f64::max);
    let proven = dev.len() >= 30 && dev_t >= 2.0 && fin.len() >= 10 && fin_mean > 0.0;
    let positive = mean > 0.0 && dev_mean > 0.0 && fin_mean > 0.0;
    let beats =
        !bdev.is_empty() && !bfin.is_empty() && dev_mean > bdev_mean && fin_mean > bfin_mean;
    let verdict = match (proven, positive, beats) {
        (true, _, true) => "GANHOU NOS DOIS PERÍODOS E GANHOU DA BASE",
        (true, _, false) => "ganhou nos dois períodos, mas NÃO ganhou de entrar em qualquer dia",
        (false, true, true) => "positivo e acima da base nos dois períodos, mas sem prova",
        (false, true, false) => "positivo nos dois períodos, mas não ganhou da base",
        _ if mean > 0.0 => "positivo, mas sem prova (pode ser sorte)",
        _ => "PERDEU",
    };
    let weeks = (rows[rows.len() - 1].0 - rows[0].0).max(WEEK_MS) / WEEK_MS;
    let pace = format!(
        "{:.1}",
        count_f64(rows.len()) / count_f64(usize::try_from(weeks).unwrap_or(1))
    )
    .replace('.', ",");
    (
        format!(
            "   {label}: {} trades (cerca de {pace} por semana). Média {} por trade: {} até out/2024 ({} trades) e {} depois ({}). Base: {} e {}. Fecharam no lucro: {}. Chegaram a 3R: {}; 5R: {}; 10R: {}; 20R: {}. O maior: {}. {verdict}.",
            rows.len(),
            r_pt(mean),
            r_pt(dev_mean),
            dev.len(),
            r_pt(fin_mean),
            fin.len(),
            r_pt(bdev_mean),
            r_pt(bfin_mean),
            share(rows, f64::MIN_POSITIVE),
            share(rows, 3.0),
            share(rows, 5.0),
            share(rows, 10.0),
            share(rows, 20.0),
            r_pt(best),
        ),
        positive && beats,
    )
}

/// Every setup over a group of charts: (daily, weekly) per asset.
fn study(group: &str, charts: &[(Vec<Bar>, Vec<Bar>)], split_ms: i64, good: &mut Vec<String>) {
    println!();
    println!("{group} ({} ativos juntos)", charts.len());
    for setup in Setup::ALL {
        let (every, hold) = if setup.weekly() {
            (4, WEEKLY_HOLD)
        } else {
            (10, DAILY_HOLD)
        };
        let mut rows = Vec::new();
        let mut risks = Vec::new();
        for (daily, week) in charts {
            let bars = if setup.weekly() { week } else { daily };
            let a = atr(bars, 14);
            let (r, k) = run(bars, &a, &entries(setup, bars, &a), hold);
            rows.extend(r);
            risks.extend(k);
        }
        let mut base_rows = Vec::new();
        if let Some(risk) = median(risks).filter(|r| *r > 0.0 && *r < 0.9) {
            for (daily, week) in charts {
                let bars = if setup.weekly() { week } else { daily };
                base_rows.extend(base(bars, &atr(bars, 14), risk, every, hold));
            }
        }
        let label = format!("{group}, {}", setup.label());
        let (text, passed) = line(&label, &mut rows, &base_rows, split_ms);
        println!("{text}");
        if passed {
            good.push(label);
        }
    }
}

/// Runs the study on the stocks of the trade universe and on BTC, ETH and SOL.
///
/// # Errors
///
/// Returns an error when the list or the crypto history cannot be read.
pub async fn cmd_runner_study(data_dir: &Path) -> CliResult {
    let split_ms = held_out_start()?
        .and_hms_opt(23, 59, 59)
        .ok_or("invalid date")?
        .and_utc()
        .timestamp_millis();
    println!(
        "Trades longos: sem alvo fixo. O stop começa no stop do setup e sobe junto com o preço (máxima desde a entrada menos 3 vezes a variação média de 14 candles); nunca desce. Custos de 0,2% por trade."
    );
    println!(
        "1R = o que se perde no primeiro stop. Chegar a 10R = ganhar 10 vezes o que arriscou. A base entra em qualquer candle com o mesmo stop e a mesma saída."
    );
    let mut good = Vec::new();
    let now = markets::now_ms()?;
    let loaded: Vec<Loaded> =
        markets::load_all(&markets::read_list(markets::TRADE_UNIVERSE)?, now).await;
    let mut groups: Vec<&str> = loaded.iter().map(|l| l.item.group.as_str()).collect();
    groups.dedup();
    for group in groups {
        let charts: Vec<(Vec<Bar>, Vec<Bar>)> = loaded
            .iter()
            .filter(|l| l.item.group == group)
            .map(|l| (l.daily.clone(), l.weekly.clone()))
            .collect();
        study(group, &charts, split_ms, &mut good);
    }
    let mut crypto = Vec::new();
    for asset in ["BTCUSDT", "ETHUSDT", "SOLUSDT"] {
        match read_series(&processed_dir(data_dir).join(format!("{asset}.csv"))) {
            Ok(s) => {
                let daily = daily_bars(&s);
                let week = weekly(&daily);
                crypto.push((daily, week));
            }
            Err(e) => eprintln!("warning: {asset} skipped: {e}"),
        }
    }
    if !crypto.is_empty() {
        study("Cripto (BTC, ETH e SOL)", &crypto, split_ms, &mut good);
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn one_position_per_chart_at_a_time() {
        let bars: Vec<Bar> = (0..40)
            .map(|i| {
                let p = 100.0 + f64::from(i);
                Bar {
                    open_time_ms: i64::from(i) * WEEK_MS,
                    open: p,
                    high: p + 0.5,
                    low: p - 0.5,
                    close: p,
                    volume: 1.0,
                }
            })
            .collect();
        let a = atr(&bars, 14);
        // Two entries while the first is still running: the second is skipped.
        let (rows, _) = run(&bars, &a, &[(1, 101.0, 99.0), (3, 103.0, 101.0)], 10);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn the_line_shows_the_big_wins() {
        let mut rows: Rows = (0..40)
            .map(|i| {
                (
                    i64::from(i) * WEEK_MS,
                    if i % 10 == 0 { 12.0 } else { -1.0 },
                )
            })
            .collect();
        let base_rows: Rows = (0..40).map(|i| (i64::from(i) * WEEK_MS, -0.5)).collect();
        let (text, _) = line("x", &mut rows, &base_rows, 20 * WEEK_MS);
        assert!(text.contains("10R: 10,0%"), "{text}");
        assert!(text.contains("O maior: +12,00R"), "{text}");
        assert!(text.contains("Fecharam no lucro: 10,0%"), "{text}");
    }
}
