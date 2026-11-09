//! `tp stormer-study`: the Stormer setups over the history of BTC, ETH and
//! SOL on the 4h, daily and weekly charts, each asset alone and the three
//! together, split in two periods like the other studies.

use std::path::Path;

use tp_core::intraday::{Bar, aggregate};
use tp_core::stormer::{Exit, Params, Pattern, Side, Trade, baseline, median_risk, scan, weekly};
use tp_data::history::{processed_dir, read_series};
use tp_data::intraday::parse_bars_csv;

use super::{CliResult, asset_name, count_f64, daily_bars, held_out_start, r_mean_t, r_pt};

struct Frame {
    name: &'static str,
    max_hold: usize,
}

const FRAMES: [Frame; 3] = [
    Frame {
        name: "4h (até 5 dias)",
        max_hold: 30,
    },
    Frame {
        name: "diário (até 30 dias)",
        max_hold: 30,
    },
    Frame {
        name: "semanal (até 12 semanas)",
        max_hold: 12,
    },
];

fn pattern_name(p: Pattern) -> &'static str {
    match p {
        Pattern::OneTwoThree => "123",
        Pattern::OneTwoThreeInside => "123 com o 3º candle dentro do 2º",
        Pattern::Trap => "Trap (falha ao perder fundo = varrida de liquidez)",
        Pattern::InsideBar => "Inside bar",
        Pattern::ClosedCandle => "Candle fechado (alvo na abertura do candle grande)",
        Pattern::GapTrap => "Gap de compra/venda",
    }
}

/// One closed trade: entry time and result.
pub type Row = (i64, f64, Exit);

/// Mean result of the comparison base in each period.
#[derive(Debug, Clone, Copy, Default)]
pub struct Base {
    pub dev: f64,
    pub fin: f64,
}

/// Verdict line, same rules as the SMC study (development t >= 2 with 30+
/// trades and a positive final period with 10+ trades), plus: in both
/// periods the setup must beat the base (entering on any candle with the
/// same stop distance and target).
pub fn line(label: &str, rows: &[Row], split_ms: i64, base: Option<Base>) -> (String, bool) {
    if rows.is_empty() {
        return (format!("   {label}: nenhum trade."), false);
    }
    let all: Vec<f64> = rows.iter().map(|r| r.1).collect();
    let dev: Vec<f64> = rows
        .iter()
        .filter(|r| r.0 <= split_ms)
        .map(|r| r.1)
        .collect();
    let fin: Vec<f64> = rows
        .iter()
        .filter(|r| r.0 > split_ms)
        .map(|r| r.1)
        .collect();
    let (mean, _) = r_mean_t(&all);
    let (dev_mean, dev_t) = r_mean_t(&dev);
    let (fin_mean, _) = r_mean_t(&fin);
    let targets = rows.iter().filter(|r| r.2 == Exit::Target).count();
    let stops = rows.iter().filter(|r| r.2 == Exit::Stop).count();
    let proven = dev.len() >= 30 && dev_t >= 2.0 && fin.len() >= 10 && fin_mean > 0.0;
    let positive = mean > 0.0 && dev_mean > 0.0 && fin_mean > 0.0;
    let beats = base.is_some_and(|b| dev_mean > b.dev && fin_mean > b.fin);
    let verdict = match (proven, positive, beats) {
        (true, _, true) => "GANHOU NOS DOIS PERÍODOS E GANHOU DA BASE",
        (true, _, false) => "ganhou nos dois períodos, mas NÃO ganhou de entrar em qualquer dia",
        (false, true, true) => "positivo e acima da base nos dois períodos, mas sem prova",
        (false, true, false) => "positivo nos dois períodos, mas não ganhou da base",
        _ if mean > 0.0 => "positivo, mas sem prova (pode ser sorte)",
        _ => "PERDEU",
    };
    let base_text = base.map_or_else(String::new, |b| {
        format!(
            " Base (entrar em qualquer dia com o mesmo stop): {} até out/2024 e {} depois.",
            r_pt(b.dev),
            r_pt(b.fin)
        )
    });
    (
        format!(
            "   {label}: {} trades (alvo {targets}, stop {stops}); média {} por trade. Até out/2024: {} em {}; de nov/2024 até hoje: {} em {}.{base_text} {verdict}.",
            rows.len(),
            r_pt(mean),
            r_pt(dev_mean),
            dev.len(),
            r_pt(fin_mean),
            fin.len()
        ),
        positive && beats,
    )
}

fn rows(trades: &[Trade], bars: &[Bar], keep: impl Fn(&Trade) -> bool) -> Vec<Row> {
    trades
        .iter()
        .filter(|t| t.how != Exit::Open && keep(t))
        .map(|t| (bars[t.entry_index].open_time_ms, t.r, t.how))
        .collect()
}

fn mean(xs: &[f64]) -> Option<f64> {
    (!xs.is_empty()).then(|| xs.iter().sum::<f64>() / count_f64(xs.len()))
}

/// Base for one variant over all sets: per asset, enter on every candle
/// with the median stop distance of that asset's trades; assets weigh as
/// many trades as the setup made there in each period.
fn base_for(
    sets: &[(Vec<Trade>, Vec<Bar>)],
    keep: &dyn Fn(&Trade) -> bool,
    side: Side,
    trend_only: bool,
    max_hold: usize,
    split_ms: i64,
) -> Option<Base> {
    let (mut dev_sum, mut dev_n, mut fin_sum, mut fin_n) = (0.0, 0usize, 0.0, 0usize);
    for (trades, bars) in sets {
        let mine: Vec<&Trade> = trades
            .iter()
            .filter(|t| t.how != Exit::Open && keep(t))
            .collect();
        let Some(risk) = median_risk(&mine) else {
            continue;
        };
        let base = baseline(
            bars,
            side,
            risk,
            trend_only,
            Params::with_max_hold(max_hold),
        );
        let period = |dev: bool| -> Vec<f64> {
            base.iter()
                .filter(|(k, _)| (bars[*k].open_time_ms <= split_ms) == dev)
                .map(|(_, r)| *r)
                .collect()
        };
        let in_dev = mine
            .iter()
            .filter(|t| bars[t.entry_index].open_time_ms <= split_ms)
            .count();
        let in_fin = mine.len() - in_dev;
        if let Some(m) = mean(&period(true)) {
            dev_sum += m * count_f64(in_dev);
            dev_n += in_dev;
        }
        if let Some(m) = mean(&period(false)) {
            fin_sum += m * count_f64(in_fin);
            fin_n += in_fin;
        }
    }
    (dev_n > 0 && fin_n > 0).then(|| Base {
        dev: dev_sum / count_f64(dev_n),
        fin: fin_sum / count_f64(fin_n),
    })
}

/// Lines for one set of trades (one asset, or several together).
pub fn report(
    title: &str,
    sets: &[(Vec<Trade>, Vec<Bar>)],
    split_ms: i64,
    max_hold: usize,
    good: &mut Vec<String>,
) {
    println!("  {title}");
    for pattern in Pattern::ALL {
        println!("   {}", pattern_name(pattern));
        let variants: [(&str, Side, bool); 4] = [
            ("compras", Side::Long, false),
            ("vendas", Side::Short, false),
            (
                "compras a favor da tendência (médias 8 e 80)",
                Side::Long,
                true,
            ),
            ("vendas a favor da tendência", Side::Short, true),
        ];
        for (label, side, trend_only) in variants {
            let keep =
                |t: &Trade| t.pattern == pattern && t.side == side && (!trend_only || t.with_trend);
            let mut all: Vec<Row> = sets
                .iter()
                .flat_map(|(trades, bars)| rows(trades, bars, keep))
                .collect();
            all.sort_by_key(|r| r.0);
            let base = base_for(sets, &keep, side, trend_only, max_hold, split_ms);
            let (text, ok) = line(label, &all, split_ms, base);
            println!(" {text}");
            if ok {
                good.push(format!(
                    "{title} · {} · {}",
                    pattern_name(pattern),
                    text.trim()
                ));
            }
        }
    }
}

/// Runs the study and prints it.
///
/// # Errors
///
/// Returns an error when a history file is missing or unreadable.
pub fn cmd_stormer_study(data_dir: &Path) -> CliResult {
    let split_ms = held_out_start()?
        .and_hms_opt(23, 59, 59)
        .ok_or("invalid date")?
        .and_utc()
        .timestamp_millis();
    println!(
        "Setups do Stormer: entrada no rompimento da máxima do candle de sinal (mínima, nas vendas), válida só no candle seguinte; stop na mínima que forma o padrão; alvo em 2 vezes o risco (no candle fechado, a abertura do candle grande). Tendência pelas médias exponenciais de 8 e 80 candles. Custos de 0,2% por trade incluídos."
    );
    println!(
        "1R = o que se perde quando bate o stop. Média +0,30R = em média ganha 30% do que arrisca por trade."
    );
    let mut per_frame: Vec<Vec<(Vec<Trade>, Vec<Bar>)>> = vec![Vec::new(), Vec::new(), Vec::new()];
    let mut good = Vec::new();
    for asset in ["BTCUSDT", "ETHUSDT", "SOLUSDT"] {
        let hourly_path = data_dir
            .join("processed")
            .join("binance")
            .join("futures_1h")
            .join(format!("{asset}.csv"));
        let hourly = std::fs::read_to_string(&hourly_path)
            .map_err(|e| format!("{}: {e}", hourly_path.display()))
            .and_then(|t| parse_bars_csv(&t).map_err(|e| e.to_string()))?;
        let daily = daily_bars(&read_series(
            &processed_dir(data_dir).join(format!("{asset}.csv")),
        )?);
        let charts = [aggregate(&hourly, 4), daily.clone(), weekly(&daily)];
        println!();
        println!("{}", asset_name(asset));
        for (i, (frame, bars)) in FRAMES.iter().zip(charts).enumerate() {
            let (trades, _) = scan(&bars, Params::with_max_hold(frame.max_hold));
            let set = vec![(trades, bars)];
            report(
                &format!("{} {}", asset_name(asset), frame.name),
                &set,
                split_ms,
                frame.max_hold,
                &mut good,
            );
            per_frame[i].extend(set);
        }
    }
    println!();
    println!("BTC, ETH e SOL juntos (mais trades, teste mais forte)");
    for (frame, sets) in FRAMES.iter().zip(&per_frame) {
        report(
            &format!("Os três {}", frame.name),
            sets,
            split_ms,
            frame.max_hold,
            &mut good,
        );
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
