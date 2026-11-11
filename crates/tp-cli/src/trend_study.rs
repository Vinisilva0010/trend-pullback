//! `tp trend-study`: the long trend. Hold an asset only while its weekly
//! close is above the average of the last 43 weekly closes (about 10
//! months), and compare with simply buying and holding it. The question is
//! whether staying out of long downtrends pays for the late entries and the
//! costs, in each period and with smaller falls.

use std::path::Path;

use tp_core::intraday::Bar;
use tp_core::stormer::weekly;
use tp_data::history::{processed_dir, read_series};

use super::{CliResult, count_f64, daily_bars, held_out_start};
use crate::markets::{self, Loaded};

/// Weeks in the average: about 10 months.
const WEEKS: usize = 43;
/// Cost of each switch in or out.
const COST: f64 = 0.002;

/// Growth of one strategy over a stretch of weeks.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Track {
    growth: f64,
    weeks: usize,
    worst_fall: f64,
}

impl Track {
    fn yearly(self) -> Option<f64> {
        (self.weeks >= 26 && self.growth > 0.0)
            .then(|| (self.growth.powf(52.0 / count_f64(self.weeks)) - 1.0) * 100.0)
    }
}

/// Weekly results of holding and of the trend rule: (week open time,
/// holding return, rule return). The rule decides at a week's close and
/// earns the next week's change; it pays the cost on each switch.
fn weekly_returns(bars: &[Bar]) -> (Vec<(i64, f64, f64)>, usize) {
    let mut out = Vec::new();
    let mut switches = 0;
    let mut held = false;
    for i in WEEKS..bars.len() {
        let prev = &bars[i - 1];
        let average = bars[i - WEEKS..i].iter().map(|b| b.close).sum::<f64>() / count_f64(WEEKS);
        let hold_ret = bars[i].close / prev.close - 1.0;
        let want = prev.close > average;
        let mut rule = if want { hold_ret } else { 0.0 };
        if want != held {
            rule -= COST;
            switches += 1;
            held = want;
        }
        out.push((bars[i].open_time_ms, hold_ret, rule));
    }
    (out, switches)
}

fn track(returns: impl Iterator<Item = f64>) -> Track {
    let mut t = Track {
        growth: 1.0,
        ..Track::default()
    };
    let mut peak: f64 = 1.0;
    for r in returns {
        t.growth *= 1.0 + r;
        t.weeks += 1;
        peak = peak.max(t.growth);
        t.worst_fall = t.worst_fall.min(t.growth / peak - 1.0);
    }
    t
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len() / 2])
}

fn pct(v: Option<f64>) -> String {
    v.map_or_else(|| "–".to_owned(), |x| format!("{x:+.1}%").replace('.', ","))
}

/// One group: the typical asset (median) per period, and how many assets
/// the rule beat.
fn study(group: &str, charts: &[Vec<Bar>], split_ms: i64, good: &mut Vec<String>) {
    println!();
    println!("{group} ({} ativos)", charts.len());
    let mut passed = 0;
    for (name, dev) in [("até out/2024", true), ("de nov/2024 até hoje", false)] {
        let mut hold_y = Vec::new();
        let mut rule_y = Vec::new();
        let mut hold_fall = Vec::new();
        let mut rule_fall = Vec::new();
        let mut beat = 0;
        let mut counted = 0;
        let mut switches = 0;
        let mut weeks = 0;
        for bars in charts {
            let (rows, n) = weekly_returns(bars);
            let part: Vec<&(i64, f64, f64)> =
                rows.iter().filter(|r| (r.0 <= split_ms) == dev).collect();
            let h = track(part.iter().map(|r| r.1));
            let t = track(part.iter().map(|r| r.2));
            let (Some(hy), Some(ty)) = (h.yearly(), t.yearly()) else {
                continue;
            };
            counted += 1;
            if ty > hy {
                beat += 1;
            }
            hold_y.push(hy);
            rule_y.push(ty);
            hold_fall.push(h.worst_fall * 100.0);
            rule_fall.push(t.worst_fall * 100.0);
            switches += n;
            weeks += rows.len();
        }
        if counted == 0 {
            println!("   {name}: sem dados suficientes.");
            continue;
        }
        let (hy, ty) = (median(hold_y), median(rule_y));
        let (hf, tf) = (median(hold_fall), median(rule_fall));
        let years = count_f64(weeks) / 52.0;
        println!(
            "   {name}: segurando, o ativo típico rendeu {} ao ano, com queda máxima de {}. Pela tendência longa: {} ao ano, com queda máxima de {}. A regra rendeu mais em {beat} de {counted} ativos. Trocas: cerca de {} por ativo por ano.",
            pct(hy),
            pct(hf),
            pct(ty),
            pct(tf),
            format!("{:.1}", count_f64(switches) / years.max(1.0)).replace('.', ",")
        );
        let better_return = ty.zip(hy).is_some_and(|(t, h)| t >= h);
        let smaller_fall = tf.zip(hf).is_some_and(|(t, h)| t > h);
        if better_return && smaller_fall {
            passed += 1;
        }
    }
    if passed == 2 {
        let label = format!(
            "{group}: tendência longa rendeu igual ou mais que segurar, com quedas menores, nos dois períodos"
        );
        println!("   {label}.");
        good.push(label);
    } else {
        println!("   Não rendeu igual ou mais que segurar com quedas menores nos dois períodos.");
    }
}

/// Runs the study on the trade universe and on BTC, ETH and SOL.
///
/// # Errors
///
/// Returns an error when the list cannot be read.
pub async fn cmd_trend_study(data_dir: &Path) -> CliResult {
    let split_ms = held_out_start()?
        .and_hms_opt(23, 59, 59)
        .ok_or("invalid date")?
        .and_utc()
        .timestamp_millis();
    println!(
        "Tendência longa: ficar comprado só enquanto o fechamento semanal está acima da média dos últimos 43 fechamentos semanais (cerca de 10 meses). Decide no fechamento da semana e vale para a semana seguinte. Custo de 0,2% em cada entrada e saída."
    );
    println!(
        "Comparação: comprar e segurar o mesmo ativo no mesmo período. Os números são do ativo típico do grupo (a mediana). Queda máxima é a maior queda do topo até o fundo."
    );
    println!(
        "Atenção: a lista é das empresas grandes de hoje; as que quebraram nesses anos não estão nela, o que deixa o segurar melhor do que foi de verdade."
    );
    let now = markets::now_ms()?;
    let loaded: Vec<Loaded> =
        markets::load_all(&markets::read_list(markets::TRADE_UNIVERSE)?, now).await;
    let mut good = Vec::new();
    let mut groups: Vec<&str> = loaded.iter().map(|l| l.item.group.as_str()).collect();
    groups.dedup();
    for group in groups {
        let charts: Vec<Vec<Bar>> = loaded
            .iter()
            .filter(|l| l.item.group == group)
            .map(|l| l.weekly.clone())
            .collect();
        study(group, &charts, split_ms, &mut good);
    }
    let crypto: Vec<Vec<Bar>> = ["BTCUSDT", "ETHUSDT", "SOLUSDT"]
        .iter()
        .filter_map(|asset| {
            match read_series(&processed_dir(data_dir).join(format!("{asset}.csv"))) {
                Ok(s) => Some(weekly(&daily_bars(&s))),
                Err(e) => {
                    eprintln!("warning: {asset} skipped: {e}");
                    None
                }
            }
        })
        .collect();
    if !crypto.is_empty() {
        study("Cripto (BTC, ETH e SOL)", &crypto, split_ms, &mut good);
    }
    println!();
    println!(
        "RESUMO: onde a tendência longa rendeu igual ou mais que segurar, com quedas menores, nos dois períodos"
    );
    if good.is_empty() {
        println!(" Nada.");
    }
    for g in good {
        println!(" {g}");
    }
    Ok(std::process::ExitCode::SUCCESS)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    fn bars(closes: &[f64]) -> Vec<Bar> {
        closes
            .iter()
            .enumerate()
            .map(|(i, c)| Bar {
                open_time_ms: i64::try_from(i).unwrap(),
                open: *c,
                high: *c,
                low: *c,
                close: *c,
                volume: 1.0,
            })
            .collect()
    }

    #[test]
    fn the_rule_steps_out_of_a_long_fall() {
        // 60 weeks up 1% a week, then 60 weeks down 1% a week.
        let mut closes = vec![100.0];
        for _ in 0..60 {
            closes.push(closes.last().unwrap() * 1.01);
        }
        for _ in 0..60 {
            closes.push(closes.last().unwrap() * 0.99);
        }
        let (rows, switches) = weekly_returns(&bars(&closes));
        let hold = track(rows.iter().map(|r| r.1));
        let rule = track(rows.iter().map(|r| r.2));
        assert!(rule.growth > hold.growth, "{rule:?} {hold:?}");
        assert!(rule.worst_fall > hold.worst_fall);
        assert_eq!(switches, 2);
    }

    #[test]
    fn a_yearly_rate_needs_half_a_year() {
        let t = Track {
            growth: 1.21,
            weeks: 104,
            worst_fall: 0.0,
        };
        assert!((t.yearly().unwrap() - 10.0).abs() < 1e-9);
        assert_eq!(
            Track {
                growth: 1.1,
                weeks: 10,
                worst_fall: 0.0
            }
            .yearly(),
            None
        );
    }
}
