//! `tp refine-study`: the two stock setups that passed (trap buy on the
//! daily chart with the trend, trap buy on the weekly chart, US stocks of
//! the trade universe) split by conditions known when the signal appears.
//! A condition is worth using only when the trades that have it did better
//! than all trades and better than the trades without it in both periods,
//! with enough trades left. Seven conditions are tried, so one or two can
//! look good by luck: the two periods are the guard against that.

use std::path::Path;

use tp_core::intraday::Bar;
use tp_core::runner::atr;
use tp_core::stormer::{Exit, Params, Pattern, Side, Trade, ema, scan, weekly};
use tp_data::yahoo::{Interval, fetch_chart};

use super::{CliResult, count_f64, held_out_start, r_mean_t, r_pt};
use crate::markets::{self, Loaded};

const DAY_MS: i64 = 24 * 3600 * 1000;

/// The conditions tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filter {
    MarketUp,
    Stronger,
    Volume,
    TightStop,
    NearHigh,
    StrongClose,
    WeeklyUp,
}

impl Filter {
    const ALL: [Self; 7] = [
        Self::MarketUp,
        Self::Stronger,
        Self::Volume,
        Self::TightStop,
        Self::NearHigh,
        Self::StrongClose,
        Self::WeeklyUp,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::MarketUp => "S&P 500 acima da média de 200 dias",
            Self::Stronger => "ação subiu mais que o S&P 500 nos 6 meses antes",
            Self::Volume => "candle do sinal com volume acima da média dos 20 anteriores",
            Self::TightStop => "stop a até 1,5 vez a variação média (ATR de 14 candles)",
            Self::NearHigh => "preço a até 15% da máxima de 52 semanas",
            Self::StrongClose => "candle do sinal fechou na metade de cima",
            Self::WeeklyUp => "gráfico semanal da ação em alta (médias de 8 e 80 semanas)",
        }
    }
}

/// The S&P 500 by day, with its 200-day average.
struct Index {
    times: Vec<i64>,
    closes: Vec<f64>,
    sma200: Vec<Option<f64>>,
}

impl Index {
    fn new(bars: &[Bar]) -> Self {
        let closes: Vec<f64> = bars.iter().map(|b| b.close).collect();
        let sma200 = (0..closes.len())
            .map(|i| (i >= 199).then(|| closes[i - 199..=i].iter().sum::<f64>() / 200.0))
            .collect();
        Self {
            times: bars.iter().map(|b| b.open_time_ms).collect(),
            closes,
            sma200,
        }
    }

    /// Last day at or before `ms`.
    fn at(&self, ms: i64) -> Option<usize> {
        self.times.partition_point(|t| *t <= ms).checked_sub(1)
    }

    fn above_average(&self, ms: i64) -> Option<bool> {
        let i = self.at(ms)?;
        Some(self.closes[i] > self.sma200[i]?)
    }

    /// Change between two moments.
    fn change(&self, from_ms: i64, to_ms: i64) -> Option<f64> {
        Some(self.closes[self.at(to_ms)?] / self.closes[self.at(from_ms)?] - 1.0)
    }
}

/// Weekly candles with their 8- and 80-week averages.
type Weeks = (Vec<Bar>, Vec<Option<f64>>, Vec<Option<f64>>);

/// One chart with what the conditions need.
struct Chart<'a> {
    bars: &'a [Bar],
    atr: Vec<Option<f64>>,
    /// Candles in half a year and in a year.
    half_year: usize,
    year: usize,
    /// How long after its time stamp a candle is closed.
    span_ms: i64,
    /// Weekly candles of the same stock with their averages (daily setup only).
    weeks: Option<Weeks>,
}

/// Whether trade `t` has the condition, from data up to the signal candle
/// only; `None` when there is not enough history to tell.
#[allow(clippy::many_single_char_names)] // trade, chart and candle indexes, as in the scanner
fn has(filter: Filter, t: &Trade, c: &Chart, index: &Index) -> Option<bool> {
    let s = t.signal_index;
    let bar = c.bars.get(s)?;
    let closed_ms = bar.open_time_ms + c.span_ms;
    match filter {
        Filter::MarketUp => index.above_average(closed_ms),
        Filter::Stronger => {
            let before = c.bars.get(s.checked_sub(c.half_year)?)?;
            let own = bar.close / before.close - 1.0;
            Some(own > index.change(before.open_time_ms + c.span_ms, closed_ms)?)
        }
        Filter::Volume => {
            let prior = c.bars.get(s.checked_sub(20)?..s)?;
            let mean = prior.iter().map(|b| b.volume).sum::<f64>() / 20.0;
            (mean > 0.0 && bar.volume > 0.0).then_some(bar.volume > mean)
        }
        Filter::TightStop => {
            let a = c.atr.get(s).copied().flatten()?;
            (a > 0.0).then_some((t.entry - t.stop) / a <= 1.5)
        }
        Filter::NearHigh => {
            let window = c.bars.get(s.checked_sub(c.year - 1)?..=s)?;
            let high = window.iter().map(|b| b.high).fold(f64::MIN, f64::max);
            Some(bar.close >= 0.85 * high)
        }
        Filter::StrongClose => {
            let range = bar.high - bar.low;
            (range > 0.0).then_some((bar.close - bar.low) / range >= 0.5)
        }
        Filter::WeeklyUp => {
            let (weeks, fast, slow) = c.weeks.as_ref()?;
            // The last week that had closed when the signal appeared.
            let i = weeks
                .partition_point(|w| w.open_time_ms + 7 * DAY_MS <= closed_ms)
                .checked_sub(1)?;
            let (f, sl) = (fast[i]?, slow[i]?);
            Some(weeks[i].close > f && f > sl)
        }
    }
}

/// (entry time, result in R, has the condition) per closed trade.
type Rows = Vec<(i64, f64, Option<bool>)>;

struct Stats {
    n: usize,
    mean: f64,
    wins: f64,
}

fn stats(rs: &[f64]) -> Stats {
    Stats {
        n: rs.len(),
        mean: r_mean_t(rs).0,
        wins: if rs.is_empty() {
            0.0
        } else {
            count_f64(rs.iter().filter(|r| **r > 0.0).count()) / count_f64(rs.len()) * 100.0
        },
    }
}

/// Welch's t of the difference of two means.
fn welch(a: &[f64], b: &[f64]) -> f64 {
    let var = |xs: &[f64]| {
        let m = xs.iter().sum::<f64>() / count_f64(xs.len());
        xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / count_f64(xs.len() - 1)
    };
    if a.len() < 2 || b.len() < 2 {
        return 0.0;
    }
    let se = (var(a) / count_f64(a.len()) + var(b) / count_f64(b.len())).sqrt();
    if se > 0.0 {
        (r_mean_t(a).0 - r_mean_t(b).0) / se
    } else {
        0.0
    }
}

fn part(rows: &Rows, late: bool, split_ms: i64, want: Option<bool>) -> Vec<f64> {
    rows.iter()
        .filter(|r| (r.0 > split_ms) == late)
        .filter(|r| want.is_none_or(|w| r.2 == Some(w)))
        .map(|r| r.1)
        .collect()
}

/// Verdict of one condition: `(text, helps)`.
fn verdict(rows: &Rows, split_ms: i64) -> (String, bool) {
    let side = |late: bool| {
        (
            stats(&part(rows, late, split_ms, None)),
            stats(&part(rows, late, split_ms, Some(true))),
            stats(&part(rows, late, split_ms, Some(false))),
        )
    };
    let (dev_all, dev_with, dev_without) = side(false);
    let (fin_all, fin_with, fin_without) = side(true);
    let better = |all: &Stats, with: &Stats, without: &Stats| {
        with.n > 0 && with.mean > all.mean && (without.n == 0 || with.mean > without.mean)
    };
    let dev_ok = better(&dev_all, &dev_with, &dev_without);
    let fin_ok = better(&fin_all, &fin_with, &fin_without);
    let enough = dev_with.n >= 30 && fin_with.n >= 10;
    let with: Vec<f64> = rows
        .iter()
        .filter(|r| r.2 == Some(true))
        .map(|r| r.1)
        .collect();
    let without: Vec<f64> = rows
        .iter()
        .filter(|r| r.2 == Some(false))
        .map(|r| r.1)
        .collect();
    let t = welch(&with, &without);
    let kept = count_f64(with.len()) / count_f64((with.len() + without.len()).max(1)) * 100.0;
    let (word, helps) = match (dev_ok && fin_ok, enough, t >= 2.0) {
        (true, true, true) => ("AJUDA NOS DOIS PERÍODOS, COM PROVA", true),
        (true, true, false) => (
            "melhor nos dois períodos, mas a diferença é pequena para o número de trades (pode ser sorte)",
            false,
        ),
        (true, false, _) => ("melhor nos dois períodos, mas sobram poucos trades", false),
        _ if dev_ok || fin_ok => ("melhor só em um dos períodos: NÃO USAR", false),
        _ => ("NÃO AJUDA", false),
    };
    let show = |s: &Stats| {
        format!(
            "{} em {} trades, {:.0}% no lucro",
            r_pt(s.mean),
            s.n,
            s.wins
        )
    };
    (
        format!(
            "ficam {kept:.0}% dos trades. Até out/2024: com ela {}; sem ela {}. Depois: com ela {}; sem ela {}. {word}.",
            show(&dev_with),
            show(&dev_without),
            show(&fin_with),
            show(&fin_without),
        ),
        helps,
    )
}

fn study(
    title: &str,
    weekly_setup: bool,
    loaded: &[Loaded],
    index: &Index,
    split_ms: i64,
    good: &mut Vec<String>,
) {
    let mut per_filter: Vec<Rows> = vec![Vec::new(); Filter::ALL.len()];
    for l in loaded {
        let bars: &[Bar] = if weekly_setup { &l.weekly } else { &l.daily };
        let weeks = (!weekly_setup).then(|| {
            let w = weekly(&l.daily);
            let (fast, slow) = (ema(&w, 8), ema(&w, 80));
            (w, fast, slow)
        });
        let chart = Chart {
            bars,
            atr: atr(bars, 14),
            half_year: if weekly_setup { 26 } else { 126 },
            year: if weekly_setup { 52 } else { 252 },
            span_ms: if weekly_setup { 7 * DAY_MS } else { DAY_MS },
            weeks,
        };
        let hold = if weekly_setup { 12 } else { 30 };
        let (trades, _) = scan(bars, Params::with_max_hold(hold));
        for t in trades.iter().filter(|t| {
            t.pattern == Pattern::Trap
                && t.side == Side::Long
                && t.how != Exit::Open
                && (weekly_setup || t.with_trend)
        }) {
            let ms = bars[t.entry_index].open_time_ms;
            for (rows, f) in per_filter.iter_mut().zip(Filter::ALL) {
                rows.push((ms, t.r, has(f, t, &chart, index)));
            }
        }
    }
    println!();
    let all = &per_filter[0];
    let dev = stats(&part(all, false, split_ms, None));
    let fin = stats(&part(all, true, split_ms, None));
    println!(
        "{title}: {} trades. Sem filtro nenhum: {} até out/2024 ({} trades, {:.0}% no lucro) e {} depois ({} trades, {:.0}% no lucro).",
        all.len(),
        r_pt(dev.mean),
        dev.n,
        dev.wins,
        r_pt(fin.mean),
        fin.n,
        fin.wins
    );
    for (rows, f) in per_filter.iter().zip(Filter::ALL) {
        if f == Filter::WeeklyUp && weekly_setup {
            continue;
        }
        let (text, helps) = verdict(rows, split_ms);
        println!("   Condição: {}. Com ela {text}", f.label());
        if helps {
            good.push(format!("{title}: {}", f.label()));
        }
    }
}

/// Runs the study on the US stocks of the trade universe.
///
/// # Errors
///
/// Returns an error when the list cannot be read or the S&P 500 history
/// cannot be fetched.
pub async fn cmd_refine_study(_data_dir: &Path) -> CliResult {
    let split_ms = held_out_start()?
        .and_hms_opt(23, 59, 59)
        .ok_or("invalid date")?
        .and_utc()
        .timestamp_millis();
    println!(
        "Lapidação dos dois setups de ações que passaram no teste. Cada condição é conhecida na hora do sinal. Resultado em R (1R = o que se perde no stop), já com custos."
    );
    println!(
        "Uma condição só vale se os trades com ela forem melhores que todos e melhores que os sem ela nos dois períodos, com pelo menos 30 trades no primeiro e 10 no segundo. São 7 condições testadas: uma ou duas podem parecer boas por sorte, por isso a exigência dos dois períodos."
    );
    let now = markets::now_ms()?;
    let index = Index::new(&fetch_chart("SPY", Interval::Day, now).await?);
    let items: Vec<markets::Item> = markets::read_list(markets::TRADE_UNIVERSE)?
        .into_iter()
        .filter(|i| i.group == "EUA")
        .collect();
    let loaded = markets::load_all(&items, now).await;
    let mut good = Vec::new();
    study(
        "Trap de compra no diário, a favor da tendência",
        false,
        &loaded,
        &index,
        split_ms,
        &mut good,
    );
    study(
        "Trap de compra no semanal",
        true,
        &loaded,
        &index,
        split_ms,
        &mut good,
    );
    println!();
    println!("RESUMO: condições que ajudam nos dois períodos, com prova");
    if good.is_empty() {
        println!(" Nenhuma. Os setups ficam como estão.");
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

    fn bar(day: i64, close: f64) -> Bar {
        Bar {
            open_time_ms: day * DAY_MS,
            open: close,
            high: close + 1.0,
            low: close - 1.0,
            close,
            volume: 100.0,
        }
    }

    #[test]
    fn the_index_average_needs_200_days_and_reads_the_last_day_before() {
        let bars: Vec<Bar> = (0..260)
            .map(|d| bar(d, 100.0 + f64::from(u8::try_from(d % 200).unwrap())))
            .collect();
        let index = Index::new(&bars);
        assert_eq!(index.above_average(100 * DAY_MS), None);
        // Day 199 closes at 299, far above the average of 100..=299.
        assert_eq!(index.above_average(199 * DAY_MS + 5), Some(true));
        // Day 200 falls back to 100.
        assert_eq!(index.above_average(200 * DAY_MS), Some(false));
        assert_eq!(index.at(-1), None);
    }

    #[test]
    fn a_condition_helps_only_when_better_in_both_periods() {
        let split = 1000;
        let mut rows: Rows = Vec::new();
        // With the condition: +0.5 or +0.7; without: -0.3 or -0.1; in both periods.
        for i in 0..80_i64 {
            let late = i >= 40;
            let wobble = if i % 2 == 0 { 0.1 } else { -0.1 };
            rows.push((if late { 2000 + i } else { i }, 0.6 + wobble, Some(true)));
            rows.push((if late { 2000 + i } else { i }, -0.2 + wobble, Some(false)));
        }
        let (text, helps) = verdict(&rows, split);
        assert!(helps, "{text}");
        assert!(text.contains("ficam 50% dos trades"), "{text}");
        // Better only before the split: not used.
        let flipped: Rows = rows
            .iter()
            .map(|r| if r.0 > split { (r.0, -r.1, r.2) } else { *r })
            .collect();
        let (text, helps) = verdict(&flipped, split);
        assert!(!helps);
        assert!(text.contains("NÃO USAR"), "{text}");
    }

    #[test]
    fn volume_and_close_are_read_from_the_signal_candle() {
        let mut bars: Vec<Bar> = (0..30).map(|d| bar(d, 50.0)).collect();
        bars[25].volume = 300.0;
        bars[25].close = 50.9;
        let chart = Chart {
            bars: &bars,
            atr: atr(&bars, 14),
            half_year: 126,
            year: 252,
            span_ms: DAY_MS,
            weeks: None,
        };
        let index = Index::new(&bars);
        let t = Trade {
            pattern: Pattern::Trap,
            side: Side::Long,
            signal_index: 25,
            entry_index: 26,
            exit_index: 27,
            entry: 51.0,
            stop: 49.0,
            target: 55.0,
            exit: 55.0,
            r: 2.0,
            how: Exit::Target,
            with_trend: true,
        };
        assert_eq!(has(Filter::Volume, &t, &chart, &index), Some(true));
        assert_eq!(has(Filter::StrongClose, &t, &chart, &index), Some(true));
        // ATR is 2: a stop 2 away is 1 ATR.
        assert_eq!(has(Filter::TightStop, &t, &chart, &index), Some(true));
        // Not enough history for the yearly ones.
        assert_eq!(has(Filter::NearHigh, &t, &chart, &index), None);
        assert_eq!(has(Filter::Stronger, &t, &chart, &index), None);
    }
}
