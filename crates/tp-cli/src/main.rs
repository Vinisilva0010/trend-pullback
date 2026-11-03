//! Command-line entry point for trend-pullback.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tp_data::binance::download::{DownloadConfig, DownloadSummary, download_spot_daily, kline_dir};
use tp_data::history::{BuildSummary, build_all, processed_dir};
use tp_data::month::YearMonth;
use tp_data::universe::{ExclusionReason, UniverseResult, UniverseRules, build_and_write};

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
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let Cli { data_dir, command } = cli;
    match command {
        Command::DownloadBinance {
            from,
            to,
            symbols,
            concurrency,
        } => {
            if from > to {
                return Err("--from must not be after --to".into());
            }
            let config = DownloadConfig {
                data_dir: data_dir.clone(),
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
            print_download_summary(&summary);
            let failures_path = write_failures(&data_dir, &summary)?;
            println!("failure list: {}", failures_path.display());
            Ok(exit_code(summary.failures.is_empty()))
        }
        Command::BuildSeries => {
            let source = kline_dir(&data_dir);
            let out_dir = processed_dir(&data_dir);
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
        Command::BuildUniverse { size, exclusions } => {
            let out_dir = data_dir.join("processed").join("binance");
            let rules = UniverseRules {
                size,
                ..UniverseRules::default()
            };
            let result = build_and_write(&processed_dir(&data_dir), &exclusions, &out_dir, rules)?;
            print_universe_summary(&result);
            println!("written: {}", out_dir.join("universe.csv").display());
            Ok(ExitCode::SUCCESS)
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

fn print_download_summary(summary: &DownloadSummary) {
    println!("---");
    println!("symbols:         {}", summary.symbols);
    println!("files available: {}", summary.files_available);
    println!("downloaded now:  {}", summary.downloaded);
    println!("already valid:   {}", summary.already_valid);
    println!("failed:          {}", summary.failures.len());
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
