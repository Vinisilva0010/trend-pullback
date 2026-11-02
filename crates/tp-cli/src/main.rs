//! Command-line entry point for trend-pullback.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tp_data::binance::download::{DownloadConfig, DownloadSummary, download_spot_daily};
use tp_data::month::YearMonth;

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
            print_summary(&summary);
            let failures_path = write_failures(&data_dir, &summary)?;
            println!("failure list: {}", failures_path.display());
            Ok(if summary.failures.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
    }
}

fn print_summary(summary: &DownloadSummary) {
    println!("---");
    println!("symbols:         {}", summary.symbols);
    println!("files available: {}", summary.files_available);
    println!("downloaded now:  {}", summary.downloaded);
    println!("already valid:   {}", summary.already_valid);
    println!("failed:          {}", summary.failures.len());
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
