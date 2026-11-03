//! Command-line entry point for trend-pullback.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::{Datelike, NaiveDate};
use clap::{Parser, Subcommand};
use tp_backtest::{BacktestConfig, Market, equity_csv, run, summarize, trades_csv};
use tp_core::market::Candle;
use tp_core::strategy::{EntryMode, Frame, StrategyParams, market_ok, setups_on};
use tp_data::binance::download::{DownloadConfig, DownloadSummary, download_spot_daily, kline_dir};
use tp_data::history::{BuildSummary, build_all, processed_dir, read_series};
use tp_data::month::YearMonth;
use tp_data::universe::{
    ExclusionReason, UniverseResult, UniverseRules, build_and_write, parse_universe_csv,
};

type CliResult = Result<ExitCode, Box<dyn Error>>;

#[derive(Debug, Parser)]
#[command(name = "tp", version, about = "trend-pullback research tool")]
struct Cli {
    /// Directory for downloaded data and generated reports.
    #[arg(long, global = true, default_value = "data")]
    data_dir: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Download Binance spot daily klines (USDT pairs) with checksum verification.
    DownloadBinance {
        /// First month to download (YYYY-MM).
        #[arg(long, default_value = "2019-01")]
        from: YearMonth,
        /// Last month to download (YYYY-MM).
        #[arg(long, default_value = "2026-10")]
        to: YearMonth,
        /// Comma-separated symbols; all USDT pairs when omitted.
        #[arg(long, value_delimiter = ',')]
        symbols: Option<Vec<String>>,
        /// Symbols downloaded in parallel (1 to 32).
        #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u16).range(1..=32))]
        concurrency: u16,
    },
    /// Join verified monthly archives into one daily CSV per symbol and write a quality report.
    BuildSeries,
    /// Rank the most traded eligible USDT pairs at the start of each month.
    BuildUniverse {
        /// Number of pairs selected per month.
        #[arg(long, default_value_t = 20)]
        size: usize,
        /// File with excluded base assets, one per line.
        #[arg(long, default_value = "config/universe_exclusions.txt")]
        exclusions: PathBuf,
    },
    /// Show the setups of one day (crypto), with the order for the next day.
    Signals {
        /// Signal date (YYYY-MM-DD); defaults to the last day with BTCUSDT data.
        #[arg(long)]
        date: Option<NaiveDate>,
        /// Use the fair value gap entry instead of the EMA.
        #[arg(long)]
        fvg: bool,
    },
    /// Simulate the method on crypto. Stops at 2024-10-31 unless --end is given:
    /// the last two years are the held-out final test.
    Backtest {
        /// Use the fair value gap entry instead of the EMA.
        #[arg(long)]
        fvg: bool,
        /// First day (YYYY-MM-DD); defaults to the first BTCUSDT day.
        #[arg(long)]
        start: Option<NaiveDate>,
        /// Last day (YYYY-MM-DD); defaults to 2024-10-31.
        #[arg(long)]
        end: Option<NaiveDate>,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::DownloadBinance {
            from,
            to,
            symbols,
            concurrency,
        } => cmd_download(&cli.data_dir, from, to, symbols, concurrency).await,
        Command::BuildSeries => cmd_build_series(&cli.data_dir),
        Command::BuildUniverse { size, exclusions } => {
            cmd_build_universe(&cli.data_dir, size, &exclusions)
        }
        Command::Signals { date, fvg } => cmd_signals(&cli.data_dir, date, fvg),
        Command::Backtest { fvg, start, end } => cmd_backtest(&cli.data_dir, fvg, start, end),
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn exit_code(success: bool) -> ExitCode {
    if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

async fn cmd_download(
    data_dir: &Path,
    from: YearMonth,
    to: YearMonth,
    symbols: Option<Vec<String>>,
    concurrency: u16,
) -> CliResult {
    if from > to {
        return Err("--from must not be after --to".into());
    }
    let config = DownloadConfig {
        data_dir: data_dir.to_path_buf(),
        from,
        to,
        symbols,
        concurrency: usize::from(concurrency),
        max_attempts: 5,
    };
    println!(
        "downloading Binance spot daily klines {from} to {to} into {}",
        data_dir.display()
    );
    let summary = download_spot_daily(&config, |done, total, report| {
        println!(
            "[{done}/{total}] {}: {} files, {} new, {} already valid, {} failed",
            report.symbol,
            report.files_available,
            report.downloaded,
            report.already_valid,
            report.failures.len()
        );
    })
    .await?;
    println!("---");
    println!("symbols:         {}", summary.symbols);
    println!("files available: {}", summary.files_available);
    println!("downloaded now:  {}", summary.downloaded);
    println!("already valid:   {}", summary.already_valid);
    println!("failed:          {}", summary.failures.len());
    let failures_path = write_failures(data_dir, &summary)?;
    println!("failure list: {}", failures_path.display());
    Ok(exit_code(summary.failures.is_empty()))
}

fn cmd_build_series(data_dir: &Path) -> CliResult {
    let source = kline_dir(data_dir);
    let out_dir = processed_dir(data_dir);
    let report = data_dir
        .join("processed")
        .join("binance")
        .join("quality_report.csv");
    println!(
        "building daily series from {} into {}",
        source.display(),
        out_dir.display()
    );
    let summary = build_all(&source, &out_dir, &report)?;
    print_build_summary(&summary);
    println!("quality report: {}", report.display());
    Ok(exit_code(summary.failed.is_empty()))
}

fn cmd_build_universe(data_dir: &Path, size: usize, exclusions: &Path) -> CliResult {
    let out_dir = data_dir.join("processed").join("binance");
    let rules = UniverseRules {
        size,
        ..UniverseRules::default()
    };
    let result = build_and_write(&processed_dir(data_dir), exclusions, &out_dir, rules)?;
    print_universe_summary(&result);
    println!("written: {}", out_dir.join("universe.csv").display());
    Ok(ExitCode::SUCCESS)
}

fn cmd_signals(data_dir: &Path, date: Option<NaiveDate>, fvg: bool) -> CliResult {
    let processed = processed_dir(data_dir);
    let universe_path = data_dir
        .join("processed")
        .join("binance")
        .join("universe.csv");
    let universe_text = std::fs::read_to_string(&universe_path)?;
    let universe = parse_universe_csv(&universe_path.display().to_string(), &universe_text)?;
    let mut params = StrategyParams::crypto();
    if fvg {
        params.entry = EntryMode::Fvg;
    }
    let btc = read_series(&processed.join("BTCUSDT.csv"))?;
    let date = date
        .or_else(|| btc.last().map(Candle::date))
        .ok_or("no BTCUSDT data")?;
    let month = YearMonth::new(date.year(), date.month())?;
    let members = universe
        .get(&month)
        .ok_or("no universe for that month; run build-universe")?;
    let benchmark = Frame::new("BTCUSDT", btc, &params);
    let mut frames = BTreeMap::new();
    for symbol in members {
        let candles = read_series(&processed.join(format!("{symbol}.csv")))?;
        frames.insert(symbol.clone(), Frame::new(symbol, candles, &params));
    }
    if let Some(i) = benchmark.index_of(date) {
        let slow = benchmark.sma_slow[i].map_or_else(|| "-".to_owned(), |v| format!("{v:.2}"));
        println!(
            "{date}: BTCUSDT close {:.2}, SMA{} {slow}, market filter {}",
            benchmark.candles[i].close(),
            params.sma_slow,
            if market_ok(&benchmark, date) {
                "ON"
            } else {
                "OFF"
            }
        );
    }
    let setups = setups_on(date, &benchmark, &frames, members, &params);
    println!(
        "universe {month}: {} pairs, setups: {}",
        members.len(),
        setups.len()
    );
    for s in &setups {
        println!(
            "{:<12} close {:>12.6} | buy limit {:>12.6} | stop {:>12.6} | target {:>12.6} | risk {:>5.1}% | momentum {:+.1}%",
            s.symbol,
            s.close,
            s.limit,
            s.stop,
            s.target,
            (s.limit - s.stop) / s.limit * 100.0,
            s.momentum * 100.0
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_backtest(
    data_dir: &Path,
    fvg: bool,
    start: Option<NaiveDate>,
    end: Option<NaiveDate>,
) -> CliResult {
    let processed = processed_dir(data_dir);
    let universe_path = data_dir
        .join("processed")
        .join("binance")
        .join("universe.csv");
    let universe_text = std::fs::read_to_string(&universe_path)?;
    let universe = parse_universe_csv(&universe_path.display().to_string(), &universe_text)?;
    let by_month: BTreeMap<(i32, u32), Vec<String>> = universe
        .iter()
        .map(|(month, members)| ((month.year(), month.month()), members.clone()))
        .collect();
    let mut params = StrategyParams::crypto();
    if fvg {
        params.entry = EntryMode::Fvg;
    }
    let symbols: BTreeSet<&String> = universe.values().flatten().collect();
    let mut frames = BTreeMap::new();
    for symbol in symbols {
        let candles = read_series(&processed.join(format!("{symbol}.csv")))?;
        frames.insert(symbol.clone(), Frame::new(symbol, candles, &params));
    }
    let benchmark = Frame::new(
        "BTCUSDT",
        read_series(&processed.join("BTCUSDT.csv"))?,
        &params,
    );
    let first = benchmark
        .candles
        .first()
        .map(Candle::date)
        .ok_or("no BTCUSDT data")?;
    let held_out_from = NaiveDate::from_ymd_opt(2024, 10, 31).ok_or("invalid date")?;
    let (start, end) = (start.unwrap_or(first), end.unwrap_or(held_out_from));
    let config = BacktestConfig::crypto();
    let market = Market {
        benchmark: &benchmark,
        frames: &frames,
        universe: &by_month,
    };
    let result = run(&market, &params, &config, start, end);
    let s = summarize(&result, config.initial_capital);
    let pnl: f64 = result.trades.iter().map(|t| t.pnl).sum();
    println!(
        "period {start} to {end}, entry {}",
        if fvg { "FVG" } else { "EMA20" }
    );
    println!("trades:          {}", s.trades);
    println!("win rate:        {:.1}%", s.win_rate * 100.0);
    println!("average R:       {:+.3}", s.average_r);
    println!("profit factor:   {:.2}", s.profit_factor);
    println!("max drawdown:    {:.1}%", s.max_drawdown * 100.0);
    println!(
        "final equity:    {:.2} ({:+.1}%)",
        s.final_equity,
        s.total_return * 100.0
    );
    println!(
        "accounting check: final cash - (initial + sum of trades) = {:.9}",
        result.final_cash - (config.initial_capital + pnl)
    );
    std::fs::create_dir_all("reports")?;
    let suffix = if fvg { "fvg" } else { "ema" };
    std::fs::write(
        format!("reports/backtest_{suffix}_trades.csv"),
        trades_csv(&result.trades),
    )?;
    std::fs::write(
        format!("reports/backtest_{suffix}_equity.csv"),
        equity_csv(&result.equity),
    )?;
    println!("written: reports/backtest_{suffix}_trades.csv and _equity.csv");
    Ok(ExitCode::SUCCESS)
}

fn print_build_summary(summary: &BuildSummary) {
    let rows: usize = summary.built.iter().map(|s| s.days).sum();
    let with_gaps = summary.built.iter().filter(|s| s.gaps > 0).count();
    println!("---");
    println!("series built:     {}", summary.built.len());
    println!("total daily rows: {rows}");
    println!("series with gaps: {with_gaps}");
    println!("failed:           {}", summary.failed.len());
    for (symbol, reason) in &summary.failed {
        println!("  FAILED {symbol}: {reason}");
    }
    for symbol in ["BTCUSDT", "ETHUSDT", "SOLUSDT"] {
        if let Some(s) = summary.built.iter().find(|s| s.symbol == symbol) {
            println!(
                "{}: {} to {}, {} days, {} gaps",
                s.symbol, s.first, s.last, s.days, s.gaps
            );
        }
    }
}

fn print_universe_summary(result: &UniverseResult) {
    let months: BTreeSet<YearMonth> = result.entries.iter().map(|e| e.month).collect();
    let count = |reason: ExclusionReason| {
        result
            .excluded
            .values()
            .filter(|(r, _)| *r == reason)
            .count()
    };
    println!("---");
    println!("months ranked:  {}", months.len());
    println!("entries:        {}", result.entries.len());
    println!(
        "excluded:       {} listed, {} leveraged tokens, {} tokenized stocks, {} pegged prices",
        count(ExclusionReason::Listed),
        count(ExclusionReason::LeveragedToken),
        count(ExclusionReason::TokenizedStock),
        count(ExclusionReason::Pegged)
    );
    for (symbol, (reason, month)) in &result.excluded {
        if *reason == ExclusionReason::Pegged {
            let month = month.map_or_else(String::new, |m| m.to_string());
            println!("  pegged price: {symbol} (detected {month})");
        }
    }
    let mut samples: Vec<YearMonth> = ["2019-02", "2021-05", "2024-06"]
        .iter()
        .filter_map(|text| text.parse().ok())
        .collect();
    if let Some(last) = months.last() {
        samples.push(*last);
    }
    for month in samples {
        let names: Vec<&str> = result
            .entries
            .iter()
            .filter(|e| e.month == month)
            .map(|e| e.symbol.as_str())
            .collect();
        println!("{month}: {}", names.join(" "));
    }
}

fn write_failures(data_dir: &Path, summary: &DownloadSummary) -> std::io::Result<PathBuf> {
    let path = data_dir
        .join("raw")
        .join("binance")
        .join("download_failures.tsv");
    let mut text = String::from("symbol\tmonth\treason\n");
    for failure in &summary.failures {
        let month = failure
            .month
            .map_or_else(|| "-".to_owned(), |month| month.to_string());
        let _ = writeln!(text, "{}\t{month}\t{}", failure.symbol, failure.reason);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, text)?;
    Ok(path)
}
