//! `tp stormer-study`: the Stormer setups over the history of BTC, ETH and
//! SOL on the 4h, daily and weekly charts, each asset alone and the three
//! together, split in two periods like the other studies.

use std::path::Path;

use tp_core::intraday::{Bar, aggregate};
use tp_core::stormer::{Exit, Params, Pattern, Side, Trade, scan, weekly};
use tp_data::history::{processed_dir, read_series};
use tp_data::intraday::parse_bars_csv;

use super::{CliResult, asset_name, daily_bars, held_out_start, r_mean_t, r_pt};

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
type Row = (i64, f64, Exit);

/// Verdict line, same rules as the SMC study: development t >= 2 with 30+
/// trades and a positive final period with 10+ trades.
fn line(label: &str, rows: &[Row], split_ms: i64) -> (String, bool) {
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
    let passed = dev.len() >= 30 && dev_t >= 2.0 && fin.len() >= 10 && fin_mean > 0.0;
    let verdict = if passed {
        "GANHOU NOS DOIS PERÍODOS"
    } else if mean > 0.0 && dev_mean > 0.0 && fin_mean > 0.0 {
        "positivo nos dois períodos, mas sem prova"
    } else if mean > 0.0 {
        "positivo, mas sem prova (pode ser sorte)"
    } else {
        "PERDEU"
    };
    (
        format!(
            "   {label}: {} trades (alvo {targets}, stop {stops}); média {} por trade. Até out/2024: {} em {}; de nov/2024 até hoje: {} em {}. {verdict}.",
            rows.len(),
            r_pt(mean),
            r_pt(dev_mean),
            dev.len(),
            r_pt(fin_mean),
            fin.len()
        ),
        passed || (mean > 0.0 && dev_mean > 0.0 && fin_mean > 0.0),
    )
}

fn rows(trades: &[Trade], bars: &[Bar], keep: impl Fn(&Trade) -> bool) -> Vec<Row> {
    trades
        .iter()
        .filter(|t| t.how != Exit::Open && keep(t))
        .map(|t| (bars[t.entry_index].open_time_ms, t.r, t.how))
        .collect()
}

/// Lines for one set of trades (one asset, or the three together).
fn report(title: &str, sets: &[(Vec<Trade>, Vec<Bar>)], split_ms: i64, good: &mut Vec<String>) {
    println!("  {title}");
    for pattern in Pattern::ALL {
        let collect = |keep: &dyn Fn(&Trade) -> bool| -> Vec<Row> {
            let mut out: Vec<Row> = sets
                .iter()
                .flat_map(|(trades, bars)| rows(trades, bars, |t| t.pattern == pattern && keep(t)))
                .collect();
            out.sort_by_key(|r| r.0);
            out
        };
        let variants: [(&str, Vec<Row>); 4] = [
            ("compras", collect(&|t| t.side == Side::Long)),
            ("vendas", collect(&|t| t.side == Side::Short)),
            (
                "compras a favor da tendência (médias 8 e 80)",
                collect(&|t| t.side == Side::Long && t.with_trend),
            ),
            (
                "vendas a favor da tendência",
                collect(&|t| t.side == Side::Short && t.with_trend),
            ),
        ];
        println!("   {}", pattern_name(pattern));
        for (label, rows) in &variants {
            let (text, ok) = line(label, rows, split_ms);
            println!(" {text}");
            if ok {
                good.push(format!(
                    "{title} · {} · {label}: {}",
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
            &mut good,
        );
    }
    println!();
    println!("RESUMO: o que ficou positivo nos dois períodos");
    if good.is_empty() {
        println!(" Nada.");
    }
    for g in good {
        println!(" {g}");
    }
    Ok(std::process::ExitCode::SUCCESS)
}
