//! Command-line entry point for trend-pullback.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::{DateTime, Datelike, NaiveDate};
use clap::{Parser, Subcommand};
use tp_backtest::allocation::needs_rebalance;
use tp_backtest::allocation::{
    AllocationConfig, AllocationResult, AllocationStats, LeverageConfig, allocation_stats,
    monthly_returns, run_allocation, run_leveraged,
};
use tp_backtest::levels::{EdgeEvent, LevelParams, run_levels};
use tp_backtest::paper::{DayBar, Event, Order, Paper, Signal, trade_csv};
use tp_backtest::range::{RangeCosts, RangeParams, RangeResult, run_range};
use tp_backtest::{
    BacktestConfig, Criteria, Evaluation, Market, equity_csv, evaluate, run, trades_csv,
};
use tp_core::anomaly::{ANOMALY_RATIO, FOLLOW_HOURS, Spike, follow_after_spikes, spike_at};
use tp_core::crowding::{HIGH_RANK, LOW_RANK, funding_ranks};
use tp_core::indicators::{close_values, segments};
use tp_core::intraday::{Bar, HOUR_MS, aggregate};
use tp_core::liquidity::{LiquidityResult, mean_and_t, study_symbol};
use tp_core::liquidity::{odds_after_break, round_above, round_below, simple_atr};
use tp_core::market::Candle;
use tp_core::news::{Coins, Kind, classify_headline};
use tp_core::odds::{Entry, Forward, Race, Shape, forward_stats, race_odds, touch_odds, weekly};
use tp_core::regime::{Regime, average_run, classify};
use tp_core::strategy::{EntryMode, Frame, StrategyParams, market_ok, setups_on};
use tp_core::support::{horizontal_levels, open_gap_below, support_below};
use tp_core::trend::{DONCHIAN_LOOKBACKS, donchian_positions};
use tp_core::trend::{VolTarget, donchian_ensemble_weights, sma_trend_weights};
use tp_data::binance::download::{DownloadConfig, DownloadSummary, download_spot_daily, kline_dir};
use tp_data::binance::rest::{
    LongShort, fetch_daily_since, fetch_funding_since, fetch_long_short, fetch_open_interest,
    fetch_recent_hourly,
};
use tp_data::funding::parse_funding_csv;
use tp_data::history::{BuildSummary, build_all, processed_dir, read_series};
use tp_data::intraday::parse_bars_csv;
use tp_data::live::{State, extend_series, parse_env, parse_state, state_csv};
use tp_data::month::YearMonth;
use tp_data::news::{NewsItem, fetch_feed};
use tp_data::notify::{html_escape, send_telegram, send_telegram_html};
use tp_data::universe::{
    ExclusionReason, UniverseResult, UniverseRules, build_and_write, parse_universe_csv,
};

type CliResult = Result<ExitCode, Box<dyn Error>>;

const STOP_ATR: [f64; 3] = [1.5, 2.5, 3.5];
const HOLD_DAYS: [usize; 3] = [14, 30, 60];

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
        /// Stop distance in ATR.
        #[arg(long, default_value_t = 1.5)]
        stop_atr: f64,
        /// Maximum holding days.
        #[arg(long, default_value_t = 14)]
        hold: usize,
        /// Exit everything by trailing low or time, without selling half at the target.
        #[arg(long)]
        no_partial: bool,
    },
    /// Run the 36 pre-registered variations on the development period (until 2024-10-31).
    Grid,
    /// Phase 7 test: buying at the close of days with extreme funding, per period.
    FundingStudy,
    /// Hourly alerts: tells when BTC, ETH or SOL breaks its 20- or 60-day high or
    /// low, with the next liquidity levels and their historical odds.
    Alerts {
        /// Send the alerts on Telegram (reads .env).
        #[arg(long)]
        notify: bool,
        /// Show the alerts without saving or sending.
        #[arg(long)]
        dry_run: bool,
    },
    /// Phase 6 level module: horizontal ranges from repeated touches, trading
    /// breakouts and failed breakouts on 1h and 4h futures bars.
    Levels {
        /// Use the final period (2024-11-01 to 2026-10-31) instead of development.
        #[arg(long)]
        final_period: bool,
    },
    /// Phase 6 range module: fade Bollinger bands with limit orders on 1h and 4h
    /// futures bars, only after a range day.
    Range {
        /// Use the final period (2024-11-01 to 2026-10-31) instead of development.
        #[arg(long)]
        final_period: bool,
    },
    /// Phase 6 test: BTC, ETH and SOL sleeves in one account, with all positions scaled by k.
    Portfolio {
        /// Use the final period (2024-11-01 to 2026-10-31) instead of development.
        #[arg(long)]
        final_period: bool,
    },
    /// Phase 6 test: hold L times equity only while the market is in the up regime.
    RegimeSizing {
        /// Comma-separated assets.
        #[arg(long, value_delimiter = ',', default_value = "BTCUSDT,ETHUSDT,SOLUSDT")]
        assets: Vec<String>,
        /// Use the final period (2024-11-01 to 2026-10-31) instead of development.
        #[arg(long)]
        final_period: bool,
    },
    /// Phase 6 step 1: how much time each asset spends up, down, in range or in transition.
    Regimes {
        /// Comma-separated assets.
        #[arg(long, value_delimiter = ',', default_value = "BTCUSDT,ETHUSDT,SOLUSDT")]
        assets: Vec<String>,
    },
    /// Phase 4 sizing test: the approved signal on a perpetual with cross margin,
    /// real funding, liquidation checks, and monthly results per position size.
    Sizing {
        /// Comma-separated assets.
        #[arg(long, value_delimiter = ',', default_value = "BTCUSDT,ETHUSDT,SOLUSDT")]
        assets: Vec<String>,
        /// Use the final period (2024-11-01 to 2026-10-31) instead of development.
        #[arg(long)]
        final_period: bool,
    },
    /// Phase 3 liquidity study: magnet test and returns after breaks and sweeps.
    Liquidity {
        /// Use the final period (2024-11-01 to 2026-10-31) instead of development.
        #[arg(long)]
        final_period: bool,
    },
    /// Daily report in Portuguese for BTC, ETH and SOL: trend, candle reading,
    /// touch chances from history and what to do; also records the model weights.
    Daily {
        /// Starting money of the paper account (used only on its first day).
        #[arg(long, default_value_t = 1000.0)]
        capital: f64,
        /// Send a Telegram message when an order is needed (reads .env).
        #[arg(long)]
        notify: bool,
        /// Show the report without saving anything; with --notify, send it to
        /// Telegram as a preview.
        #[arg(long)]
        dry_run: bool,
    },
    /// Pre-registered trend-following test (Donchian ensemble and SMA family) per asset.
    Trend {
        /// Comma-separated assets.
        #[arg(long, value_delimiter = ',', default_value = "BTCUSDT,ETHUSDT")]
        assets: Vec<String>,
        /// First day (YYYY-MM-DD); defaults to 2019-01-01.
        #[arg(long)]
        start: Option<NaiveDate>,
        /// Last day (YYYY-MM-DD); defaults to 2024-10-31 (final two years held out).
        #[arg(long)]
        end: Option<NaiveDate>,
    },
}

#[derive(Debug, Clone, Copy)]
struct Variant {
    fvg: bool,
    stop: usize,
    partial: bool,
    hold: usize,
}

impl Variant {
    fn label(self) -> String {
        format!(
            "{} stop {:.1}ATR {} hold {:>2}d",
            if self.fvg { "FVG" } else { "EMA" },
            STOP_ATR[self.stop],
            if self.partial { "half@2R" } else { "no-half" },
            HOLD_DAYS[self.hold]
        )
    }

    fn distance(self, other: Self) -> usize {
        self.stop.abs_diff(other.stop)
            + self.hold.abs_diff(other.hold)
            + usize::from(self.partial != other.partial)
    }
}

struct CryptoMarket {
    benchmark: Frame,
    frames: BTreeMap<String, Frame>,
    by_month: BTreeMap<(i32, u32), Vec<String>>,
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
        Command::Backtest {
            fvg,
            start,
            end,
            stop_atr,
            hold,
            no_partial,
        } => cmd_backtest(&cli.data_dir, fvg, start, end, stop_atr, hold, !no_partial),
        Command::Grid => cmd_grid(&cli.data_dir),
        Command::FundingStudy => cmd_funding_study(&cli.data_dir),
        Command::Alerts { notify, dry_run } => cmd_alerts(&cli.data_dir, notify, dry_run).await,
        Command::Levels { final_period } => cmd_levels(&cli.data_dir, final_period),
        Command::Range { final_period } => cmd_range(&cli.data_dir, final_period),
        Command::Portfolio { final_period } => cmd_portfolio(&cli.data_dir, final_period),
        Command::RegimeSizing {
            assets,
            final_period,
        } => cmd_regime_sizing(&cli.data_dir, &assets, final_period),
        Command::Regimes { assets } => cmd_regimes(&cli.data_dir, &assets),
        Command::Sizing {
            assets,
            final_period,
        } => cmd_sizing(&cli.data_dir, &assets, final_period),
        Command::Liquidity { final_period } => cmd_liquidity(&cli.data_dir, final_period),
        Command::Daily {
            capital,
            notify,
            dry_run,
        } => run_daily(&cli.data_dir, capital, notify, dry_run).await,
        Command::Trend { assets, start, end } => cmd_trend(&cli.data_dir, &assets, start, end),
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

fn held_out_start() -> Result<NaiveDate, Box<dyn Error>> {
    Ok(NaiveDate::from_ymd_opt(2024, 10, 31).ok_or("invalid date")?)
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

fn load_universe(data_dir: &Path) -> Result<BTreeMap<YearMonth, Vec<String>>, Box<dyn Error>> {
    let path = data_dir
        .join("processed")
        .join("binance")
        .join("universe.csv");
    let text = std::fs::read_to_string(&path)?;
    Ok(parse_universe_csv(&path.display().to_string(), &text)?)
}

fn load_crypto_market(
    data_dir: &Path,
    params: &StrategyParams,
) -> Result<CryptoMarket, Box<dyn Error>> {
    let processed = processed_dir(data_dir);
    let universe = load_universe(data_dir)?;
    let by_month = universe
        .iter()
        .map(|(month, members)| ((month.year(), month.month()), members.clone()))
        .collect();
    let symbols: BTreeSet<&String> = universe.values().flatten().collect();
    let mut frames = BTreeMap::new();
    for symbol in symbols {
        let candles = read_series(&processed.join(format!("{symbol}.csv")))?;
        frames.insert(symbol.clone(), Frame::new(symbol, candles, params));
    }
    let benchmark = Frame::new(
        "BTCUSDT",
        read_series(&processed.join("BTCUSDT.csv"))?,
        params,
    );
    Ok(CryptoMarket {
        benchmark,
        frames,
        by_month,
    })
}

fn cmd_signals(data_dir: &Path, date: Option<NaiveDate>, fvg: bool) -> CliResult {
    let processed = processed_dir(data_dir);
    let universe = load_universe(data_dir)?;
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
            "{:<12} close {:>14.8} | buy limit {:>14.8} | stop {:>14.8} | target {:>14.8} | risk {:>5.1}% | momentum {:+.1}%",
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
    stop_atr: f64,
    hold: usize,
    partial: bool,
) -> CliResult {
    let mut params = StrategyParams::crypto();
    params.stop_atr = stop_atr;
    if fvg {
        params.entry = EntryMode::Fvg;
    }
    let m = load_crypto_market(data_dir, &params)?;
    let first = m
        .benchmark
        .candles
        .first()
        .map(Candle::date)
        .ok_or("no BTCUSDT data")?;
    let (start, end) = (start.unwrap_or(first), end.unwrap_or(held_out_start()?));
    let config = BacktestConfig {
        max_hold_days: hold,
        take_partial: partial,
        ..BacktestConfig::crypto()
    };
    let market = Market {
        benchmark: &m.benchmark,
        frames: &m.frames,
        universe: &m.by_month,
    };
    let result = run(&market, &params, &config, start, end);
    let e = evaluate(&result, config.initial_capital, &Criteria::default());
    let pnl: f64 = result.trades.iter().map(|t| t.pnl).sum();
    println!(
        "period {start} to {end}, entry {}, stop {stop_atr} ATR, hold {hold}d, {}",
        if fvg { "FVG" } else { "EMA20" },
        if partial { "half at 2R" } else { "no partial" }
    );
    print_evaluation(&e);
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

fn print_evaluation(e: &Evaluation) {
    let s = e.summary;
    println!("trades:            {}", s.trades);
    println!("win rate:          {:.1}%", s.win_rate * 100.0);
    println!("average R:         {:+.3}", s.average_r);
    println!("profit factor:     {:.2}", s.profit_factor);
    println!("max drawdown:      {:.1}%", s.max_drawdown * 100.0);
    println!(
        "final equity:      {:.2} ({:+.1}%)",
        s.final_equity,
        s.total_return * 100.0
    );
    println!(
        "non-losing years:  {:.0}%",
        e.non_losing_year_fraction * 100.0
    );
    println!(
        "R total {:+.1} | without top 3 trades {:+.1} | without best symbol {:+.1}",
        e.r_total, e.r_without_top3, e.r_without_best_symbol
    );
    println!(
        "verdict:           {}",
        if e.passes { "PASSES" } else { "fails" }
    );
}

fn run_variant(m: &CryptoMarket, v: Variant, start: NaiveDate, end: NaiveDate) -> Evaluation {
    let mut params = StrategyParams::crypto();
    params.stop_atr = STOP_ATR[v.stop];
    if v.fvg {
        params.entry = EntryMode::Fvg;
    }
    let config = BacktestConfig {
        max_hold_days: HOLD_DAYS[v.hold],
        take_partial: v.partial,
        ..BacktestConfig::crypto()
    };
    let market = Market {
        benchmark: &m.benchmark,
        frames: &m.frames,
        universe: &m.by_month,
    };
    let result = run(&market, &params, &config, start, end);
    evaluate(&result, config.initial_capital, &Criteria::default())
}

fn cmd_grid(data_dir: &Path) -> CliResult {
    let m = load_crypto_market(data_dir, &StrategyParams::crypto())?;
    let start = m
        .benchmark
        .candles
        .first()
        .map(Candle::date)
        .ok_or("no BTCUSDT data")?;
    let end = held_out_start()?;
    let mut variants = Vec::new();
    for fvg in [false, true] {
        for stop in 0..STOP_ATR.len() {
            for partial in [true, false] {
                for hold in 0..HOLD_DAYS.len() {
                    variants.push(Variant {
                        fvg,
                        stop,
                        partial,
                        hold,
                    });
                }
            }
        }
    }
    println!(
        "running {} variants from {start} to {end} (final two years stay held out)",
        variants.len()
    );
    let threads = std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get);
    let chunk = variants.len().div_ceil(threads.max(1));
    let market = &m;
    let results: Vec<(Variant, Evaluation)> = std::thread::scope(|scope| {
        let handles: Vec<_> = variants
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .map(|&v| (v, run_variant(market, v, start, end)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut all = Vec::new();
        for handle in handles {
            all.extend(handle.join().map_err(|_| "a grid worker panicked")?);
        }
        Ok::<_, Box<dyn Error>>(all)
    })?;
    print_grid(&results);
    Ok(ExitCode::SUCCESS)
}

async fn run_daily(data_dir: &Path, capital: f64, notify: bool, dry_run: bool) -> CliResult {
    let result = cmd_daily(data_dir, capital, notify, dry_run).await;
    let Err(error) = &result else {
        return result;
    };
    if !notify || dry_run {
        return result;
    }
    let message = format!(
        "trend-pullback: o relatório diário FALHOU ({error}). Nada foi calculado hoje; confira o computador."
    );
    let Ok(text) = std::fs::read_to_string(".env") else {
        return result;
    };
    let env = parse_env(&text);
    let (Some(token), Some(chat)) = (env.get("TELEGRAM_BOT_TOKEN"), env.get("TELEGRAM_CHAT_ID"))
    else {
        return result;
    };
    if send_telegram(token, chat, &message).await.is_err() {
        eprintln!("warning: could not send the failure alert");
    }
    result
}

#[allow(clippy::too_many_lines)] // one run: data, report, paper account, save, send
async fn cmd_daily(data_dir: &Path, capital: f64, notify: bool, dry_run: bool) -> CliResult {
    let since_epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    let now_ms = i64::try_from(since_epoch.as_millis())?;
    let today = DateTime::from_timestamp_millis(now_ms)
        .map(|d| d.date_naive())
        .ok_or("invalid clock")?;
    let live_dir = data_dir.join("live");
    let state_path = live_dir.join("state.csv");
    let mut state = match std::fs::read_to_string(&state_path) {
        Ok(text) => parse_state(&text)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::new(),
        Err(e) => return Err(e.into()),
    };
    let vol = VolTarget::paper_spot();
    let threshold = AllocationConfig::crypto().rebalance_threshold;
    let mut sections = Vec::new();
    let mut log = String::new();
    let mut market: BTreeMap<String, (Vec<Candle>, Analysis)> = BTreeMap::new();
    for asset in ["BTCUSDT", "ETHUSDT", "SOLUSDT"] {
        let mut candles = read_series(&processed_dir(data_dir).join(format!("{asset}.csv")))?;
        let last = candles.last().map(Candle::date).ok_or("empty history")?;
        let since = last.succ_opt().ok_or("date overflow")?;
        let recent = fetch_daily_since(asset, since, now_ms).await?;
        extend_series(&mut candles, &recent)?;
        let signal_date = candles.last().map(Candle::date).ok_or("empty history")?;
        let mut lines = Vec::new();
        if (today - signal_date).num_days() > 1 {
            lines.push(format!(
                "AVISO {}: o último candle fechado é de {}; os dados podem estar atrasados",
                asset_name(asset),
                signal_date.format("%d/%m")
            ));
        }
        let closes = close_values(&candles);
        let funding = load_funding_live(data_dir, asset, now_ms).await;
        let analysis = daily_report_with(asset, &candles, &closes, funding.as_ref());
        lines.extend(analysis.lines.iter().cloned());
        if let Some(oi) = positioning_line(asset).await {
            // Right after the trend line.
            let at = lines
                .len()
                .min(lines.len().saturating_sub(analysis.lines.len()) + 2);
            lines.insert(at, oi);
        }
        sections.push(lines.join("\n"));
        // The tested model weight is still tracked for the record and the
        // paper account; it is no longer part of the message.
        let (held_date, held) = state.get(asset).copied().unwrap_or((NaiveDate::MIN, 0.0));
        if held_date < signal_date {
            let target = live_weight(asset, &closes, &vol);
            let close = closes.last().copied().unwrap_or(0.0);
            let order = needs_rebalance(held, target, threshold);
            let new_held = if order { target } else { held };
            let _ = writeln!(
                log,
                "{today},{asset},{signal_date},{close},{target:.6},{held:.6},{new_held:.6},{order}"
            );
            state.insert(asset.to_owned(), (signal_date, new_held));
        }
        market.insert(asset.to_owned(), (candles, analysis));
    }
    sections.push(run_paper(&live_dir, capital, &market, dry_run)?);
    let mut header = "Relatório diário (candle fechado às 21h de Brasília). Chance = quantas vezes aconteceu no histórico da moeda; não é certeza.".to_owned();
    if let Some(agenda) = DateTime::from_timestamp_millis(now_ms).and_then(agenda_line) {
        header.push('\n');
        header.push_str(&agenda);
    }
    if dry_run && notify {
        header.insert_str(
            0,
            "PRÉVIA (teste: nada foi salvo e a conta fictícia não mudou)\n",
        );
    }
    let lines = sections;
    println!("{header}");
    for line in &lines {
        println!("{line}");
    }
    if dry_run && !notify {
        println!("dry run: nothing saved, nothing sent");
        return Ok(ExitCode::SUCCESS);
    }
    if dry_run {
        println!("preview: nothing saved, sending to Telegram");
    } else {
        std::fs::create_dir_all(&live_dir)?;
        std::fs::write(&state_path, state_csv(&state))?;
        let log_path = append_signal_log(&live_dir, &log)?;
        println!("saved: {} and {}", state_path.display(), log_path.display());
    }
    if notify {
        let env = parse_env(&std::fs::read_to_string(".env")?);
        let (Some(token), Some(chat)) =
            (env.get("TELEGRAM_BOT_TOKEN"), env.get("TELEGRAM_CHAT_ID"))
        else {
            return Err("TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID must be set in .env".into());
        };
        // One message per asset keeps each under Telegram's 4096-character limit.
        let mut messages = lines.clone();
        messages.insert(0, header.clone());
        send_sections(token, chat, &messages).await?;
        println!("telegram: sent");
    }
    Ok(ExitCode::SUCCESS)
}

fn append_signal_log(live_dir: &Path, log: &str) -> std::io::Result<PathBuf> {
    let path = live_dir.join("signals.csv");
    let new_log = !path.exists();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    if new_log {
        std::io::Write::write_all(
            &mut file,
            b"run_date,asset,signal_date,close,model_weight,held_before,held_after,order\n",
        )?;
    }
    std::io::Write::write_all(&mut file, log.as_bytes())?;
    Ok(path)
}

fn cmd_liquidity(data_dir: &Path, final_period: bool) -> CliResult {
    let m = load_crypto_market(data_dir, &StrategyParams::crypto())?;
    let (start, end) = if final_period {
        (
            NaiveDate::from_ymd_opt(2024, 11, 1).ok_or("invalid date")?,
            NaiveDate::from_ymd_opt(2026, 10, 31).ok_or("invalid date")?,
        )
    } else {
        (default_trend_start()?, held_out_start()?)
    };
    println!(
        "liquidity study {start} to {end} ({}), top-20 universe, daily candles",
        if final_period {
            "final period"
        } else {
            "development"
        }
    );
    for horizon in [1, 3, 7, 14] {
        let mut result = LiquidityResult::default();
        for (symbol, frame) in &m.frames {
            let eligible: Vec<bool> = frame
                .candles
                .iter()
                .map(|c| {
                    let d = c.date();
                    d >= start
                        && d <= end
                        && m.by_month
                            .get(&(d.year(), d.month()))
                            .is_some_and(|members| members.iter().any(|s| s == symbol))
                })
                .collect();
            study_symbol(&frame.candles, &frame.atr, &eligible, horizon, &mut result);
        }
        print_liquidity(horizon, &result);
    }
    Ok(ExitCode::SUCCESS)
}

fn print_liquidity(horizon: usize, r: &LiquidityResult) {
    println!("--- horizon {horizon} day(s) ---");
    println!(
        "{:<12} {:>6} {:>7} {:>10} {:>9} {:>6}",
        "level", "n", "hit%", "expected%", "excess", "z"
    );
    for (kind, s) in &r.touches {
        println!(
            "{:<12} {:>6} {:>6.1}% {:>9.1}% {:>+7.1}pp {:>+6.1}",
            kind.label(),
            s.count,
            s.hit_rate() * 100.0,
            s.expected_rate() * 100.0,
            (s.hit_rate() - s.expected_rate()) * 100.0,
            s.z()
        );
    }
    println!(
        "{:<12} {:>6} {:>17} {:>6}",
        "event", "n", "mean excess ATR", "t"
    );
    for (kind, values) in &r.events {
        let (mean, t) = mean_and_t(values);
        println!(
            "{:<12} {:>6} {:>+17.3} {:>+6.1}",
            kind.label(),
            values.len(),
            mean,
            t
        );
    }
}

fn count_f64(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

fn cmd_sizing(data_dir: &Path, assets: &[String], final_period: bool) -> CliResult {
    let (start, end) = if final_period {
        (
            NaiveDate::from_ymd_opt(2024, 11, 1).ok_or("invalid date")?,
            NaiveDate::from_ymd_opt(2026, 10, 31).ok_or("invalid date")?,
        )
    } else {
        (default_trend_start()?, held_out_start()?)
    };
    let config = LeverageConfig::crypto();
    println!(
        "sizing test {start} to {end} ({}) | USDT perpetual, cross margin, real Binance funding, 0.1% fee + 0.1% slippage per side",
        if final_period {
            "final period"
        } else {
            "development"
        }
    );
    if final_period {
        println!(
            "criteria: Sharpe > 0, maxDD <= 35%, never liquidated, liquidation always >= 50% below price"
        );
    } else {
        println!(
            "criteria: Sharpe >= 1.0, maxDD <= 35%, non-losing years >= 60%, never liquidated, liquidation always >= 50% below price"
        );
    }
    for asset in assets {
        let candles = read_series(&processed_dir(data_dir).join(format!("{asset}.csv")))?;
        if segments(&candles).len() > 1 {
            return Err(format!("{asset} has gaps; trend models need continuous data").into());
        }
        let funding_path = data_dir
            .join("processed")
            .join("binance")
            .join("funding")
            .join(format!("{asset}.csv"));
        let text = std::fs::read_to_string(&funding_path).map_err(|e| {
            format!(
                "{}: {e} (run python3 scripts/funding_history.py first)",
                funding_path.display()
            )
        })?;
        let funding = parse_funding_csv(&text)?;
        let closes = close_values(&candles);
        println!("=== {asset} ===");
        for target in [0.25, 0.5, 0.75] {
            for cap in [1.0_f64, 2.0, 3.0] {
                let vol = VolTarget {
                    target,
                    window: 90,
                    max_per_signal: cap.max(2.0),
                    max_total: cap,
                };
                let weights = donchian_ensemble_weights(&closes, &vol);
                let r = run_leveraged(&candles, &weights, &funding, &config, start, end);
                print_sizing_row(
                    &format!("vol {:>3.0}% cap {:.0}x", target * 100.0, cap),
                    &r,
                    &config,
                    final_period,
                );
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn print_sizing_row(
    label: &str,
    r: &tp_backtest::allocation::LeveragedResult,
    config: &LeverageConfig,
    final_period: bool,
) {
    let s = allocation_stats(&r.equity, config.initial_capital);
    let months = monthly_returns(&r.equity, config.initial_capital);
    let (month_avg, _) = mean_and_t(&months);
    let positive = months.iter().filter(|m| **m > 0.0).count();
    let worst = months.iter().copied().fold(0.0_f64, f64::min);
    let (trade_avg, _) = mean_and_t(&r.round_trips);
    let wins = r.round_trips.iter().filter(|t| **t > 0.0).count();
    let liquidation = match r.liquidated_on {
        Some(day) => format!("LIQUIDATED {day}"),
        None if r.min_liquidation_distance >= 1.0 => "liq at zero".to_owned(),
        None => format!("liq {:.0}% below", r.min_liquidation_distance * 100.0),
    };
    let safe = r.liquidated_on.is_none() && r.min_liquidation_distance >= 0.5;
    let passes = if final_period {
        s.sharpe > 0.0 && s.max_drawdown <= 0.35 && safe
    } else {
        s.sharpe >= 1.0 && s.max_drawdown <= 0.35 && s.non_losing_year_fraction >= 0.6 && safe
    };
    println!(
        "{label} | CAGR {:>+6.1}% | month avg {:>+5.2}% | months+ {:>3.0}% | worst month {:>+6.1}% | maxDD {:>5.1}% | Sharpe {:>4.2} | {} | trades {} (win {:.0}%, avg {:+.1}%) | funding {:.1}% | {}",
        s.cagr * 100.0,
        month_avg * 100.0,
        if months.is_empty() {
            0.0
        } else {
            count_f64(positive) / count_f64(months.len()) * 100.0
        },
        worst * 100.0,
        s.max_drawdown * 100.0,
        s.sharpe,
        liquidation,
        r.round_trips.len(),
        if r.round_trips.is_empty() {
            0.0
        } else {
            count_f64(wins) / count_f64(r.round_trips.len()) * 100.0
        },
        trade_avg * 100.0,
        r.funding_paid / config.initial_capital * 100.0,
        if passes { "PASSES" } else { "fails" }
    );
}

fn cmd_regimes(data_dir: &Path, assets: &[String]) -> CliResult {
    let all = [Regime::Up, Regime::Down, Regime::Range, Regime::Transition];
    for asset in assets {
        let candles = read_series(&processed_dir(data_dir).join(format!("{asset}.csv")))?;
        if segments(&candles).len() > 1 {
            return Err(format!("{asset} has gaps; regimes need continuous data").into());
        }
        let closes = close_values(&candles);
        let regimes = classify(&closes);
        let classified = regimes.iter().filter(|r| r.is_some()).count();
        println!("=== {asset}: {classified} classified days ===");
        println!(
            "{:<11} {:>6} {:>12} {:>16} {:>11}",
            "regime", "days%", "avg run (d)", "mean day return", "daily vol"
        );
        for regime in all {
            let returns: Vec<f64> = (1..closes.len())
                .filter(|&i| regimes[i - 1] == Some(regime))
                .map(|i| closes[i] / closes[i - 1] - 1.0)
                .collect();
            let (mean, _) = mean_and_t(&returns);
            let var = if returns.len() > 1 {
                returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>()
                    / (count_f64(returns.len()) - 1.0)
            } else {
                0.0
            };
            println!(
                "{:<11} {:>5.1}% {:>12.1} {:>+15.3}% {:>10.2}%",
                regime.label(),
                count_f64(returns.len()) / count_f64(classified.max(1)) * 100.0,
                average_run(&regimes, regime),
                mean * 100.0,
                var.sqrt() * 100.0
            );
        }
        let mut years: BTreeMap<i32, [usize; 4]> = BTreeMap::new();
        for (candle, regime) in candles.iter().zip(&regimes) {
            if let Some(r) = regime {
                let slot = all.iter().position(|x| x == r).unwrap_or(3);
                years.entry(candle.date().year()).or_default()[slot] += 1;
            }
        }
        for (year, counts) in &years {
            let total = count_f64(counts.iter().sum::<usize>().max(1));
            println!(
                "  {year}: up {:>3.0}% | down {:>3.0}% | range {:>3.0}% | transition {:>3.0}%",
                count_f64(counts[0]) / total * 100.0,
                count_f64(counts[1]) / total * 100.0,
                count_f64(counts[2]) / total * 100.0,
                count_f64(counts[3]) / total * 100.0
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn load_funding(data_dir: &Path, asset: &str) -> Result<BTreeMap<NaiveDate, f64>, Box<dyn Error>> {
    let path = data_dir
        .join("processed")
        .join("binance")
        .join("funding")
        .join(format!("{asset}.csv"));
    let text = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "{}: {e} (run python3 scripts/funding_history.py first)",
            path.display()
        )
    })?;
    Ok(parse_funding_csv(&text)?)
}

fn cmd_regime_sizing(data_dir: &Path, assets: &[String], final_period: bool) -> CliResult {
    let (start, end) = if final_period {
        (
            NaiveDate::from_ymd_opt(2024, 11, 1).ok_or("invalid date")?,
            NaiveDate::from_ymd_opt(2026, 10, 31).ok_or("invalid date")?,
        )
    } else {
        (default_trend_start()?, held_out_start()?)
    };
    let config = LeverageConfig::crypto();
    println!(
        "up-regime sizing {start} to {end} ({}) | USDT perpetual, cross margin, real funding, 0.1% fee + 0.1% slippage per side",
        if final_period {
            "final period"
        } else {
            "development"
        }
    );
    for asset in assets {
        let candles = read_series(&processed_dir(data_dir).join(format!("{asset}.csv")))?;
        if segments(&candles).len() > 1 {
            return Err(format!("{asset} has gaps; regimes need continuous data").into());
        }
        let funding = load_funding(data_dir, asset)?;
        let closes = close_values(&candles);
        let regimes = classify(&closes);
        println!("=== {asset} ===");
        let base = donchian_ensemble_weights(&closes, &VolTarget::paper_spot());
        let r = run_leveraged(&candles, &base, &funding, &config, start, end);
        print_sizing_row("current: Donchian vol 25%", &r, &config, final_period);
        for level in [0.5, 1.0, 1.5, 2.0] {
            let weights: Vec<f64> = regimes
                .iter()
                .map(|r| if *r == Some(Regime::Up) { level } else { 0.0 })
                .collect();
            let r = run_leveraged(&candles, &weights, &funding, &config, start, end);
            print_sizing_row(&format!("up regime {level:.1}x"), &r, &config, final_period);
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Sums sleeve equity curves day by day; before a sleeve has data it holds
/// its starting cash.
fn combine_sleeves(
    sleeves: &[Vec<tp_backtest::EquityPoint>],
    initial_each: f64,
) -> Vec<tp_backtest::EquityPoint> {
    let maps: Vec<BTreeMap<NaiveDate, f64>> = sleeves
        .iter()
        .map(|curve| curve.iter().map(|p| (p.date, p.equity)).collect())
        .collect();
    let dates: BTreeSet<NaiveDate> = maps.iter().flat_map(|m| m.keys().copied()).collect();
    let mut last = vec![initial_each; maps.len()];
    dates
        .into_iter()
        .map(|date| {
            for (value, map) in last.iter_mut().zip(&maps) {
                if let Some(v) = map.get(&date) {
                    *value = *v;
                }
            }
            let equity: f64 = last.iter().sum();
            tp_backtest::EquityPoint {
                date,
                equity,
                cash: equity,
                positions: 0,
            }
        })
        .collect()
}

fn cmd_portfolio(data_dir: &Path, final_period: bool) -> CliResult {
    let (start, end) = if final_period {
        (
            NaiveDate::from_ymd_opt(2024, 11, 1).ok_or("invalid date")?,
            NaiveDate::from_ymd_opt(2026, 10, 31).ok_or("invalid date")?,
        )
    } else {
        (default_trend_start()?, held_out_start()?)
    };
    let total = 10_000.0;
    let config = LeverageConfig {
        initial_capital: total / 3.0,
        ..LeverageConfig::crypto()
    };
    println!(
        "portfolio {start} to {end} ({}) | BTC and ETH Donchian vol 25%, SOL up regime 0.5x, equal thirds, positions times k",
        if final_period {
            "final period"
        } else {
            "development"
        }
    );
    let mut inputs = Vec::new();
    for asset in ["BTCUSDT", "ETHUSDT", "SOLUSDT"] {
        let candles = read_series(&processed_dir(data_dir).join(format!("{asset}.csv")))?;
        if segments(&candles).len() > 1 {
            return Err(format!("{asset} has gaps").into());
        }
        let closes = close_values(&candles);
        let base: Vec<f64> = if asset == "SOLUSDT" {
            classify(&closes)
                .iter()
                .map(|r| if *r == Some(Regime::Up) { 0.5 } else { 0.0 })
                .collect()
        } else {
            donchian_ensemble_weights(&closes, &VolTarget::paper_spot())
        };
        inputs.push((candles, base, load_funding(data_dir, asset)?));
    }
    for k in [1.0, 1.5, 2.0, 3.0] {
        let mut curves = Vec::new();
        let mut liquidated = false;
        let mut min_distance = 1.0_f64;
        let mut funding = 0.0;
        for (candles, base, funding_map) in &inputs {
            let weights: Vec<f64> = base.iter().map(|w| w * k).collect();
            let r = run_leveraged(candles, &weights, funding_map, &config, start, end);
            liquidated |= r.liquidated_on.is_some();
            min_distance = min_distance.min(r.min_liquidation_distance);
            funding += r.funding_paid;
            curves.push(r.equity);
        }
        let combined = combine_sleeves(&curves, config.initial_capital);
        let s = allocation_stats(&combined, total);
        let months = monthly_returns(&combined, total);
        let (month_avg, _) = mean_and_t(&months);
        let positive = months.iter().filter(|m| **m > 0.0).count();
        let worst = months.iter().copied().fold(0.0_f64, f64::min);
        let safe = !liquidated && min_distance >= 0.5;
        let passes = if final_period {
            s.sharpe > 0.0 && s.max_drawdown <= 0.35 && safe
        } else {
            s.sharpe >= 1.0 && s.max_drawdown <= 0.35 && s.non_losing_year_fraction >= 0.6 && safe
        };
        let liquidation = if liquidated {
            "LIQUIDATED".to_owned()
        } else if min_distance >= 1.0 {
            "liq at zero".to_owned()
        } else {
            format!("liq {:.0}% below", min_distance * 100.0)
        };
        println!(
            "k {k:.1}x | CAGR {:>+6.1}% | month avg {:>+5.2}% | months+ {:>3.0}% | worst month {:>+6.1}% | maxDD {:>5.1}% | Sharpe {:>4.2} | years {:>3.0}% | {} | funding {:.1}% | {}",
            s.cagr * 100.0,
            month_avg * 100.0,
            if months.is_empty() {
                0.0
            } else {
                count_f64(positive) / count_f64(months.len()) * 100.0
            },
            worst * 100.0,
            s.max_drawdown * 100.0,
            s.sharpe,
            s.non_losing_year_fraction * 100.0,
            liquidation,
            funding / total * 100.0,
            if passes { "PASSES" } else { "fails" }
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// Approved Phase 6 portfolio: BTC and ETH Donchian at a 25% volatility
/// target and SOL at 0.5x during the up regime, every position times 1.5.
/// Development 2019-2024: +33% a year, 27.9% max drawdown; final period
/// 2024-11 to 2026-10: +1.8% a year, 18.6% max drawdown.
const PORTFOLIO_SCALE: f64 = 1.5;

fn live_weight(asset: &str, closes: &[f64], vol: &VolTarget) -> f64 {
    let base = if asset == "SOLUSDT" {
        if classify(closes).last().copied().flatten() == Some(Regime::Up) {
            0.5
        } else {
            0.0
        }
    } else {
        donchian_ensemble_weights(closes, vol)
            .last()
            .copied()
            .unwrap_or(0.0)
    };
    base * PORTFOLIO_SCALE
}

fn cmd_range(data_dir: &Path, final_period: bool) -> CliResult {
    let (start, end) = if final_period {
        (
            NaiveDate::from_ymd_opt(2024, 11, 1).ok_or("invalid date")?,
            NaiveDate::from_ymd_opt(2026, 10, 31).ok_or("invalid date")?,
        )
    } else {
        (default_trend_start()?, held_out_start()?)
    };
    println!(
        "range module {start} to {end} ({}) | limit orders 0.02%, stops 0.05% + 0.05% slippage, real funding, 1% risk per trade, at most 1x",
        if final_period {
            "final period"
        } else {
            "development"
        }
    );
    if final_period {
        println!("criteria: Sharpe > 0 and profit factor > 1.0");
    } else {
        println!(
            "criteria: >= 200 trades, profit factor >= 1.2, Sharpe >= 1.0, maxDD <= 20%, non-losing years >= 60%"
        );
    }
    for asset in ["BTCUSDT", "ETHUSDT", "SOLUSDT"] {
        let daily = read_series(&processed_dir(data_dir).join(format!("{asset}.csv")))?;
        let classes = classify(&close_values(&daily));
        let regimes: BTreeMap<NaiveDate, Regime> = daily
            .iter()
            .zip(&classes)
            .filter_map(|(c, r)| r.map(|r| (c.date(), r)))
            .collect();
        let path = data_dir
            .join("processed")
            .join("binance")
            .join("futures_1h")
            .join(format!("{asset}.csv"));
        let text = std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "{}: {e} (run python3 scripts/futures_1h.py first)",
                path.display()
            )
        })?;
        let hourly = parse_bars_csv(&text)?;
        let funding = load_funding(data_dir, asset)?;
        println!("=== {asset} ===");
        for bar_hours in [1_i64, 4] {
            let bars = if bar_hours == 1 {
                hourly.clone()
            } else {
                aggregate(&hourly, bar_hours)
            };
            for window in [24_usize, 48, 96] {
                for band in [2.0, 2.5] {
                    let params = RangeParams {
                        bar_hours,
                        window,
                        band,
                        stop_extra: 1.5,
                        risk: 0.01,
                        max_hold: window,
                    };
                    let r = run_range(
                        &bars,
                        &regimes,
                        &funding,
                        &params,
                        &RangeCosts::binance_futures(),
                        start,
                        end,
                        10_000.0,
                    );
                    print_range_row(
                        &format!(
                            "{}h win {:>2} band {:.1}",
                            params.bar_hours, params.window, params.band
                        ),
                        &r,
                        final_period,
                    );
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn print_range_row(label: &str, r: &RangeResult, final_period: bool) {
    let s = allocation_stats(&r.equity, 10_000.0);
    let months = monthly_returns(&r.equity, 10_000.0);
    let (month_avg, _) = mean_and_t(&months);
    let worst = months.iter().copied().fold(0.0_f64, f64::min);
    let (trade_avg, _) = mean_and_t(&r.trade_returns);
    let trades = r.trade_returns.len();
    let wins = r.trade_returns.iter().filter(|t| **t > 0.0).count();
    let pf = r.profit_factor();
    let passes = if final_period {
        s.sharpe > 0.0 && pf > 1.0
    } else {
        trades >= 200
            && pf >= 1.2
            && s.sharpe >= 1.0
            && s.max_drawdown <= 0.2
            && s.non_losing_year_fraction >= 0.6
    };
    println!(
        "{label} | trades {:>5} ({:>4.1}/month, win {:>3.0}%, avg {:+.3}%) | PF {:>4.2} | CAGR {:>+6.1}% | month avg {:>+5.2}% | worst month {:>+5.1}% | maxDD {:>4.1}% | Sharpe {:>5.2} | in market {:>3.0}% | {}",
        trades,
        if months.is_empty() {
            0.0
        } else {
            count_f64(trades) / count_f64(months.len())
        },
        if trades == 0 {
            0.0
        } else {
            count_f64(wins) / count_f64(trades) * 100.0
        },
        trade_avg * 100.0,
        pf,
        s.cagr * 100.0,
        month_avg * 100.0,
        worst * 100.0,
        s.max_drawdown * 100.0,
        s.sharpe,
        if r.bars_total == 0 {
            0.0
        } else {
            count_f64(r.bars_in_market) / count_f64(r.bars_total) * 100.0
        },
        if passes { "PASSES" } else { "fails" }
    );
}

fn cmd_levels(data_dir: &Path, final_period: bool) -> CliResult {
    let (start, end) = if final_period {
        (
            NaiveDate::from_ymd_opt(2024, 11, 1).ok_or("invalid date")?,
            NaiveDate::from_ymd_opt(2026, 10, 31).ok_or("invalid date")?,
        )
    } else {
        (default_trend_start()?, held_out_start()?)
    };
    println!(
        "level module {start} to {end} ({}) | entries at next open 0.05% + 0.05% slippage, targets 0.02%, real funding, 1% risk per trade, at most 1x",
        if final_period {
            "final period"
        } else {
            "development"
        }
    );
    if final_period {
        println!("criteria: Sharpe > 0 and profit factor > 1.0");
    } else {
        println!(
            "criteria: >= 200 trades, profit factor >= 1.2, Sharpe >= 1.0, maxDD <= 20%, non-losing years >= 60%"
        );
    }
    for asset in ["BTCUSDT", "ETHUSDT", "SOLUSDT"] {
        let path = data_dir
            .join("processed")
            .join("binance")
            .join("futures_1h")
            .join(format!("{asset}.csv"));
        let text = std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "{}: {e} (run python3 scripts/futures_1h.py first)",
                path.display()
            )
        })?;
        let hourly = parse_bars_csv(&text)?;
        let funding = load_funding(data_dir, asset)?;
        println!("=== {asset} ===");
        for bar_hours in [1_i64, 4] {
            let bars = if bar_hours == 1 {
                hourly.clone()
            } else {
                aggregate(&hourly, bar_hours)
            };
            for lookback in [30_usize, 60] {
                for (event, name) in [
                    (EdgeEvent::Breakout, "breakout"),
                    (EdgeEvent::Failure, "failed break"),
                ] {
                    let params = LevelParams::new(bar_hours, lookback, event);
                    let r = run_levels(
                        &bars,
                        &funding,
                        &params,
                        &RangeCosts::binance_futures(),
                        start,
                        end,
                        10_000.0,
                    );
                    print_range_row(
                        &format!("{bar_hours}h range {lookback:>2} {name:<12}"),
                        &r,
                        final_period,
                    );
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Days the touch chance and the up-or-down race look ahead.
const REPORT_HORIZON: usize = 7;
/// Past days in today's trend state needed before the odds use only them.
const MIN_SAME_TREND_DAYS: usize = 200;
/// Decided races needed before the report draws a conclusion.
const MIN_RACES: usize = 30;
/// |z| a race needs to say one side wins.
const RACE_Z: f64 = 2.0;
/// Days after a daily candle used for its forward result.
const CANDLE_HORIZON: usize = 7;
/// Weeks after a weekly candle used for its forward result.
const WEEK_HORIZON: usize = 2;

fn asset_name(asset: &str) -> &str {
    asset.strip_suffix("USDT").unwrap_or(asset)
}

/// Price in Brazilian notation: comma for decimals, dot for thousands.
fn fmt_pt(v: f64) -> String {
    let text = fmt_price(v);
    let (int, frac) = text.split_once('.').unwrap_or((text.as_str(), ""));
    let digits: Vec<char> = int.chars().collect();
    let mut grouped = String::new();
    for (i, d) in digits.iter().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push('.');
        }
        grouped.push(*d);
    }
    if frac.is_empty() {
        grouped
    } else {
        format!("{grouped},{frac}")
    }
}

fn pct_pt(v: f64) -> String {
    format!("{v:+.1}%").replace('.', ",")
}

fn trend_pt(regime: Option<Regime>) -> &'static str {
    match regime {
        Some(Regime::Up) => "alta",
        Some(Regime::Down) => "queda",
        Some(Regime::Range) => "lateral",
        Some(Regime::Transition) => "indefinida",
        None => "sem dados suficientes",
    }
}

/// Highest high and lowest low of the `days` candles that end at `last` (inclusive).
fn extremes_until(candles: &[Candle], last: usize, days: usize) -> (f64, f64) {
    let w = &candles[last + 1 - days.min(last + 1)..=last];
    (
        w.iter().map(Candle::high).fold(f64::MIN, f64::max),
        w.iter().map(Candle::low).fold(f64::MAX, f64::min),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Up,
    Down,
    Tie,
    FewCases,
}

fn verdict(race: Option<Race>) -> Verdict {
    match race {
        Some(r) if r.decided() >= MIN_RACES && r.z() >= RACE_Z => Verdict::Up,
        Some(r) if r.decided() >= MIN_RACES && r.z() <= -RACE_Z => Verdict::Down,
        Some(r) if r.decided() >= MIN_RACES => Verdict::Tie,
        _ => Verdict::FewCases,
    }
}

fn verdict_pt(v: Verdict) -> &'static str {
    match v {
        Verdict::Up => "Resultado: GANHA MAIS DO QUE PERDE. Vale o plano.",
        Verdict::Down => "Resultado: PERDE MAIS DO QUE GANHA. Não vale.",
        Verdict::Tie => "Resultado: EMPATE, igual cara ou coroa. Não tem vantagem.",
        Verdict::FewCases => "Resultado: POUCOS CASOS no histórico para dizer.",
    }
}

/// Plan lines for a race: target and stop one daily move from the entry,
/// how often each came first in the past, and the result.
fn race_sentence(what: &str, race: Option<Race>, step: f64, entry: f64) -> String {
    let pct = format!("{:.1}%", step / entry * 100.0).replace('.', ",");
    let plan = format!(
        "  Alvo {} (+{pct}) | Stop {} (-{pct}) | Prazo {REPORT_HORIZON} dias",
        fmt_pt(entry + step),
        fmt_pt(entry - step)
    );
    let Some(r) = race else {
        return format!("{plan}\n  Não há no histórico nenhuma vez em que {what}.");
    };
    format!(
        "{plan}\n  No passado ({} vezes assim): alvo primeiro {}, stop primeiro {}, nenhum dos dois {}.\n  {}",
        r.total(),
        r.up_first,
        r.down_first,
        r.neither,
        verdict_pt(verdict(race))
    )
}

fn as_share(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

/// Mask of the days in the same trend state as `last`, or every day when
/// there are too few of them; the flag says which one was used.
fn trend_mask(regimes: &[Option<Regime>], last: usize) -> (Vec<bool>, bool) {
    let regime = regimes[last];
    let same: Vec<bool> = regimes
        .iter()
        .map(|r| r.is_some() && *r == regime)
        .collect();
    if regime.is_some() && same.iter().filter(|s| **s).count() >= MIN_SAME_TREND_DAYS {
        (same, true)
    } else {
        (vec![true; regimes.len()], false)
    }
}

/// Lines for buying with a limit order at the 20-day low.
fn support_lines(
    name: &str,
    candles: &[Candle],
    atr: &[Option<f64>],
    regimes: &[Option<Regime>],
    last: usize,
    a: f64,
) -> (Vec<String>, Option<Race>) {
    let close = candles[last].close();
    let (_, low20) = extremes_until(candles, last, 20);
    let (mask, same_trend) = trend_mask(regimes, last);
    let trend = if same_trend {
        format!("numa {} como agora", trend_pt(regimes[last]))
    } else {
        "em qualquer tendência".to_owned()
    };
    let touch = touch_odds(
        candles,
        atr,
        &mask,
        REPORT_HORIZON,
        (close - low20) / a,
        false,
    )
    .map_or(0.0, |(p, _)| p);
    let entries: Vec<Option<Entry>> = (0..candles.len())
        .map(|t| {
            (t > 20)
                .then(|| extremes_until(candles, t - 1, 20).1)
                .and_then(|prior| (candles[t].low() <= prior).then_some(Entry::Intraday(prior)))
        })
        .collect();
    let race = race_odds(candles, atr, &entries, &mask, REPORT_HORIZON, 1.0);
    let lines = vec![
        String::new(),
        format!(
            "Plano: ordem limite em {} (mínima de 20 dias, {} do preço)",
            fmt_pt(low20),
            pct_pt((low20 / close - 1.0) * 100.0)
        ),
        format!(
            "  Chance de chegar lá: {:.0}% em {REPORT_HORIZON} dias",
            touch * 100.0
        ),
        race_sentence(
            &format!("a {name} chegou na mínima de 20 dias {trend}"),
            race,
            a,
            low20,
        ),
    ];
    (lines, race)
}

/// Lines for buying on a daily close above the 20-day high.
fn breakout_lines(
    name: &str,
    candles: &[Candle],
    atr: &[Option<f64>],
    regimes: &[Option<Regime>],
    last: usize,
    a: f64,
) -> (Vec<String>, Option<Race>) {
    let close = candles[last].close();
    let (high20, _) = extremes_until(candles, last, 20);
    let (trend_days, same_trend) = trend_mask(regimes, last);
    let touch = touch_odds(
        candles,
        atr,
        &trend_days,
        REPORT_HORIZON,
        (high20 - close) / a,
        true,
    )
    .map_or(0.0, |(p, _)| p);
    let entries: Vec<Option<Entry>> = (0..candles.len())
        .map(|t| {
            (t > 20)
                .then(|| extremes_until(candles, t - 1, 20).0)
                .and_then(|prior| {
                    (candles[t].close() > prior).then_some(Entry::AtClose(candles[t].close()))
                })
        })
        .collect();
    // Breakouts that came out of the same trend state as today, judged by the
    // day before the breakout; every breakout when there are too few.
    let from_same: Vec<bool> = (0..candles.len())
        .map(|t| t > 0 && trend_days[t - 1])
        .collect();
    let mut race = race_odds(candles, atr, &entries, &from_same, REPORT_HORIZON, 1.0);
    let mut trend = if same_trend {
        format!("saindo de uma {} como agora", trend_pt(regimes[last]))
    } else {
        "em qualquer tendência".to_owned()
    };
    if same_trend && race.is_none_or(|r| r.decided() < MIN_RACES) {
        race = race_odds(
            candles,
            atr,
            &entries,
            &vec![true; candles.len()],
            REPORT_HORIZON,
            1.0,
        );
        "em qualquer tendência (poucos casos saindo de uma tendência igual)".clone_into(&mut trend);
    }
    let lines = vec![
        String::new(),
        format!(
            "Plano: comprar se um dia fechar acima de {} (máxima de 20 dias, {} do preço)",
            fmt_pt(high20),
            pct_pt((high20 / close - 1.0) * 100.0)
        ),
        format!(
            "  Chance de chegar lá: {:.0}% em {REPORT_HORIZON} dias",
            touch * 100.0
        ),
        race_sentence(
            &format!("a {name} fechou acima da máxima de 20 dias {trend}"),
            race,
            a,
            high20,
        ),
    ];
    (lines, race)
}

/// Candle reading, only when that shape changed the following days in the past.
fn candle_line(what: &str, label: &str, horizon: &str, f: Option<Forward>) -> Option<String> {
    let f = f.filter(Forward::is_edge)?;
    let side = if f.mean_atr > f.base_mean_atr {
        "subiu mais"
    } else {
        "caiu mais"
    };
    Some(format!(
        "{what}: {label}. Isso já fez diferença: {horizon} depois de {} casos assim, o preço {side} que num dia qualquer (subiu em {:.0}% das vezes, contra {:.0}% normalmente).",
        f.cases,
        f.up_rate * 100.0,
        f.base_up_rate * 100.0
    ))
}

/// Weekly candle reading, on Sundays and only when it made a difference.
fn weekly_line(candles: &[Candle], last: usize) -> Option<String> {
    let date = candles[last].date();
    if date.weekday() != chrono::Weekday::Sun {
        return None;
    }
    let weeks = weekly(&candles[..=last]);
    let week = weeks.last().filter(|w| w.date() == date)?;
    let watr = simple_atr(&weeks, 14);
    let shape = Shape::of(week);
    let same: Vec<bool> = weeks.iter().map(|w| Shape::of(w) == shape).collect();
    candle_line(
        "Candle da semana",
        shape.label_pt(),
        "2 semanas",
        forward_stats(&weeks, &watr, &same, WEEK_HORIZON),
    )
}

/// Candles of history used to find horizontal levels and open gaps.
const LEVEL_LOOKBACK: usize = 365;
/// Candles needed on each side of a swing point.
const SWING_WING: usize = 5;
/// Swing points within this many ATRs of each other form one level.
const LEVEL_TOLERANCE_ATR: f64 = 0.5;
/// Turning points a level needs before it counts as support.
const MIN_LEVEL_TOUCHES: usize = 2;
/// Longer horizon, in days, for the chance of reaching a far level.
const FAR_HORIZON: usize = 30;

/// Per day: the support level visible the day before (from data up to then),
/// filled as a limit buy when the day's low reaches it.
fn support_entries(candles: &[Candle], atr: &[Option<f64>]) -> Vec<Option<Entry>> {
    (0..candles.len())
        .map(|t| {
            if t < 120 {
                return None;
            }
            let a = atr[t - 1].filter(|a| *a > 0.0)?;
            let levels = horizontal_levels(
                candles,
                t - 1,
                LEVEL_LOOKBACK,
                SWING_WING,
                LEVEL_TOLERANCE_ATR * a,
            );
            let s = support_below(&levels, candles[t - 1].close(), MIN_LEVEL_TOUCHES)?;
            (candles[t].low() <= s.price).then(|| Entry::Intraday(s.price.min(candles[t].open())))
        })
        .collect()
}

/// Per day: the top of the open gap visible the day before, filled as a limit
/// buy when the day's low reaches it.
fn gap_entries(candles: &[Candle]) -> Vec<Option<Entry>> {
    (0..candles.len())
        .map(|t| {
            if t < 120 {
                return None;
            }
            let g = open_gap_below(candles, t - 1, LEVEL_LOOKBACK, candles[t - 1].close())?;
            (candles[t].low() <= g.top).then(|| Entry::Intraday(g.top.min(candles[t].open())))
        })
        .collect()
}

/// Race over the days in today's trend, or over every day (with a note) when
/// that gives too few decided cases.
fn race_in_trend(
    candles: &[Candle],
    atr: &[Option<f64>],
    entries: &[Option<Entry>],
    trend_days: &[bool],
    same_trend: bool,
    same_label: &str,
) -> (Option<Race>, String) {
    let race = race_odds(candles, atr, entries, trend_days, REPORT_HORIZON, 1.0);
    if same_trend && race.is_some_and(|r| r.decided() >= MIN_RACES) {
        return (race, same_label.to_owned());
    }
    let all = race_odds(
        candles,
        atr,
        entries,
        &vec![true; candles.len()],
        REPORT_HORIZON,
        1.0,
    );
    let label = if same_trend {
        "em qualquer tendência (poucos casos numa tendência igual)"
    } else {
        "em qualquer tendência"
    };
    (all, label.to_owned())
}

/// Plans whose price was reached within 30 days less often than this are not shown.
const MIN_SHOWN_REACH: f64 = 0.10;

/// Chance of falling to a level within [`FAR_HORIZON`] days.
fn reach_chance(
    candles: &[Candle],
    atr: &[Option<f64>],
    trend_days: &[bool],
    distance_atr: f64,
) -> f64 {
    touch_odds(candles, atr, trend_days, FAR_HORIZON, distance_atr, false).map_or(0.0, |(p, _)| p)
}

fn reach_sentence(
    candles: &[Candle],
    atr: &[Option<f64>],
    trend_days: &[bool],
    distance_atr: f64,
    up: bool,
) -> String {
    let chance = |h: usize| {
        touch_odds(candles, atr, trend_days, h, distance_atr, up).map_or(0.0, |(p, _)| p) * 100.0
    };
    format!(
        "  Chance de chegar lá: {:.0}% em {REPORT_HORIZON} dias, {:.0}% em {FAR_HORIZON} dias",
        chance(REPORT_HORIZON),
        chance(FAR_HORIZON)
    )
}

/// Lines for buying at the nearest horizontal support of the past year.
fn level_lines(
    name: &str,
    candles: &[Candle],
    atr: &[Option<f64>],
    regimes: &[Option<Regime>],
    last: usize,
    a: f64,
) -> Option<(Vec<String>, Option<Race>, String, f64)> {
    let close = candles[last].close();
    let levels = horizontal_levels(
        candles,
        last,
        LEVEL_LOOKBACK,
        SWING_WING,
        LEVEL_TOLERANCE_ATR * a,
    );
    let s = support_below(&levels, close, MIN_LEVEL_TOUCHES)?;
    let (trend_days, same_trend) = trend_mask(regimes, last);
    let entries = support_entries(candles, atr);
    let (race, trend) = race_in_trend(
        candles,
        atr,
        &entries,
        &trend_days,
        same_trend,
        &format!("numa {} como agora", trend_pt(regimes[last])),
    );
    let lines = vec![
        String::new(),
        format!(
            "Plano: ordem limite em {} (suporte do último ano, {} do preço, o preço virou ali {} vezes)",
            fmt_pt(s.price),
            pct_pt((s.price / close - 1.0) * 100.0),
            s.touches
        ),
        reach_sentence(candles, atr, &trend_days, (close - s.price) / a, false),
        race_sentence(
            &format!("a {name} chegou num suporte assim {trend}"),
            race,
            a,
            s.price,
        ),
    ];
    let lines = if reach_chance(candles, atr, &trend_days, (close - s.price) / a) < MIN_SHOWN_REACH
    {
        Vec::new()
    } else {
        lines
    };
    Some((
        lines,
        race,
        format!("ordem limite no suporte em {}", fmt_pt(s.price)),
        s.price,
    ))
}

/// Lines for buying at the top of the nearest open gap below the price.
fn gap_lines(
    name: &str,
    candles: &[Candle],
    atr: &[Option<f64>],
    regimes: &[Option<Regime>],
    last: usize,
    a: f64,
) -> Option<(Vec<String>, Option<Race>, String, f64)> {
    let close = candles[last].close();
    let g = open_gap_below(candles, last, LEVEL_LOOKBACK, close)?;
    let (trend_days, same_trend) = trend_mask(regimes, last);
    let entries = gap_entries(candles);
    let (race, trend) = race_in_trend(
        candles,
        atr,
        &entries,
        &trend_days,
        same_trend,
        &format!("numa {} como agora", trend_pt(regimes[last])),
    );
    let lines = vec![
        String::new(),
        format!(
            "Plano: ordem limite em {} (gap aberto de {} a {}, formado em {}, {} do preço)",
            fmt_pt(g.top),
            fmt_pt(g.bottom),
            fmt_pt(g.top),
            candles[g.formed].date().format("%d/%m/%Y"),
            pct_pt((g.top / close - 1.0) * 100.0)
        ),
        reach_sentence(candles, atr, &trend_days, (close - g.top) / a, false),
        race_sentence(
            &format!("a {name} voltou num gap aberto {trend}"),
            race,
            a,
            g.top,
        ),
    ];
    let lines = if reach_chance(candles, atr, &trend_days, (close - g.top) / a) < MIN_SHOWN_REACH {
        Vec::new()
    } else {
        lines
    };
    Some((
        lines,
        race,
        format!("ordem limite no gap em {}", fmt_pt(g.top)),
        g.top,
    ))
}

/// Report lines for the last candle and the entries with an edge, as
/// signals for the paper account.
fn daily_report(asset: &str, candles: &[Candle], closes: &[f64]) -> Analysis {
    daily_report_with(asset, candles, closes, None)
}

#[allow(clippy::too_many_lines)] // one report: header, entry blocks, action, signals
fn daily_report_with(
    asset: &str,
    candles: &[Candle],
    closes: &[f64],
    funding: Option<&BTreeMap<NaiveDate, f64>>,
) -> Analysis {
    let mut out = Vec::new();
    let Some(last) = candles.len().checked_sub(1).filter(|l| *l > 60) else {
        return Analysis::default();
    };
    let atr = simple_atr(candles, 14);
    let Some(a) = atr[last].filter(|a| *a > 0.0) else {
        return Analysis::default();
    };
    let name = asset_name(asset);
    let today = &candles[last];
    let regimes = classify(closes);
    let votes = DONCHIAN_LOOKBACKS
        .iter()
        .filter(|&&l| donchian_positions(closes, l)[last])
        .count();
    out.push(format!(
        "{name} — dia fechado em {} às 21h: {} ({} no dia)",
        today.date().format("%d/%m"),
        fmt_pt(today.close()),
        pct_pt((today.close() / candles[last - 1].close() - 1.0) * 100.0)
    ));
    out.push(format!(
        "Tendência: {} ({votes} de 9 canais apontam para cima). Um dia normal da {name} anda {} ({}).",
        trend_pt(regimes[last]),
        fmt_pt(a),
        format!("{:.1}%", a / today.close() * 100.0).replace('.', ",")
    ));
    let shape = Shape::of(today);
    let same_shape: Vec<bool> = candles.iter().map(|c| Shape::of(c) == shape).collect();
    out.extend(candle_line(
        "Candle de hoje",
        shape.label_pt(),
        "7 dias",
        forward_stats(candles, &atr, &same_shape, CANDLE_HORIZON),
    ));
    out.extend(weekly_line(candles, last));
    let mut choices: Vec<(Option<Race>, String, Order)> = Vec::new();
    for (lines, race, label, price) in [
        level_lines(name, candles, &atr, &regimes, last, a),
        gap_lines(name, candles, &atr, &regimes, last, a),
    ]
    .into_iter()
    .flatten()
    {
        out.extend(lines);
        choices.push((race, label, Order::Limit(price)));
    }
    let (high20, low20) = extremes_until(candles, last, 20);
    let (support, support_race) = support_lines(name, candles, &atr, &regimes, last, a);
    out.extend(support);
    choices.push((
        support_race,
        format!("ordem limite na mínima de 20 dias em {}", fmt_pt(low20)),
        Order::Limit(low20),
    ));
    let (breakout, breakout_race) = breakout_lines(name, candles, &atr, &regimes, last, a);
    out.extend(breakout);
    let broke_today = today.close() > extremes_until(candles, last - 1, 20).0;
    choices.push((
        breakout_race,
        if broke_today {
            "compra agora: o dia fechou acima da máxima de 20 dias".to_owned()
        } else {
            format!("compra se um dia fechar acima de {}", fmt_pt(high20))
        },
        Order::AtClose,
    ));
    let winners: Vec<&(Option<Race>, String, Order)> = choices
        .iter()
        .filter(|(race, _, _)| verdict(*race) == Verdict::Up)
        .collect();
    let mut labels: Vec<String> = winners.iter().map(|(_, label, _)| label.clone()).collect();
    let mut signals: Vec<Signal> = winners
        .iter()
        .filter(|(_, _, order)| *order != Order::AtClose || broke_today)
        .map(|(race, label, order)| Signal {
            asset: asset.to_owned(),
            label: label.clone(),
            order: *order,
            atr: a,
            z: race.map_or(0.0, |r| r.z()),
        })
        .collect();
    if let Some((lines, extreme)) = funding.and_then(|f| funding_lines(candles, &atr, f, last)) {
        out.extend(lines);
        if let Some((Verdict::Up, z, high)) = extreme {
            let label = format!(
                "compra agora: funding {} extremo",
                if high { "alto" } else { "baixo" }
            );
            labels.push(label.clone());
            signals.push(Signal {
                asset: asset.to_owned(),
                label,
                order: Order::AtClose,
                atr: a,
                z,
            });
        }
    }
    let action = if labels.is_empty() {
        "O que fazer: esperar. Nenhum lugar de entrada tem vantagem agora.".to_owned()
    } else {
        format!(
            "O que fazer: só estes lugares subiram mais do que caíram no histórico: {}.",
            labels.join("; ")
        )
    };
    // The answer first: right after the name and trend lines.
    out.insert(2.min(out.len()), action);
    let levels = choices
        .iter()
        .filter_map(|(race, label, order)| match order {
            Order::Limit(price) => Some((level_name(label), *price, verdict(*race))),
            Order::AtClose => None,
        })
        .collect();
    Analysis {
        lines: out,
        signals,
        levels,
        funding: funding.cloned(),
    }
}

/// Decided races the development period needs before funding can conclude.
const FUNDING_MIN_DEV: usize = 30;
/// Decided races the final period needs before funding can conclude.
const FUNDING_MIN_FINAL: usize = 10;

/// Races of buying at the close of days with extreme funding (high or low),
/// split into the development period (through the held-out start) and the
/// final period after it.
fn funding_races(
    candles: &[Candle],
    atr: &[Option<f64>],
    ranks: &[Option<f64>],
    high: bool,
    split: NaiveDate,
) -> (Option<Race>, Option<Race>) {
    let entries: Vec<Option<Entry>> = candles
        .iter()
        .zip(ranks)
        .map(|(c, r)| {
            r.filter(|r| {
                if high {
                    *r >= HIGH_RANK
                } else {
                    *r <= LOW_RANK
                }
            })
            .map(|_| Entry::AtClose(c.close()))
        })
        .collect();
    let dev: Vec<bool> = candles.iter().map(|c| c.date() <= split).collect();
    let fin: Vec<bool> = dev.iter().map(|d| !d).collect();
    (
        race_odds(candles, atr, &entries, &dev, REPORT_HORIZON, 1.0),
        race_odds(candles, atr, &entries, &fin, REPORT_HORIZON, 1.0),
    )
}

/// Funding conclusion: the development period must show the edge and the
/// final period must point the same way.
fn funding_verdict(dev: Option<Race>, fin: Option<Race>) -> Verdict {
    let (Some(d), Some(f)) = (dev, fin) else {
        return Verdict::FewCases;
    };
    if d.decided() < FUNDING_MIN_DEV || f.decided() < FUNDING_MIN_FINAL {
        return Verdict::FewCases;
    }
    if d.z() >= RACE_Z && f.z() > 0.0 {
        Verdict::Up
    } else if d.z() <= -RACE_Z && f.z() < 0.0 {
        Verdict::Down
    } else {
        Verdict::Tie
    }
}

fn race_shares(r: Option<Race>) -> String {
    r.map_or_else(
        || "nenhum caso".to_owned(),
        |r| {
            let n = as_share(r.total());
            format!(
                "das {} vezes, subiu primeiro em {:.0}% e caiu primeiro em {:.0}%",
                r.total(),
                as_share(r.up_first) / n * 100.0,
                as_share(r.down_first) / n * 100.0
            )
        },
    )
}

/// Verdict, development z and side (true = high) of an extreme funding day.
type FundingExtreme = (Verdict, f64, bool);

/// Funding lines for the last candle, and the verdict and z when today's
/// funding is extreme.
fn funding_lines(
    candles: &[Candle],
    atr: &[Option<f64>],
    funding: &BTreeMap<NaiveDate, f64>,
    last: usize,
) -> Option<(Vec<String>, Option<FundingExtreme>)> {
    let dates: Vec<NaiveDate> = candles.iter().map(Candle::date).collect();
    let ranks = funding_ranks(&dates, funding);
    let rank = ranks[last]?;
    let high = rank >= HIGH_RANK;
    if !high && rank > LOW_RANK {
        // Normal funding: nothing worth a line.
        return Some((Vec::new(), None));
    }
    let split = held_out_start().ok()?;
    let (dev, fin) = funding_races(candles, atr, &ranks, high, split);
    let v = funding_verdict(dev, fin);
    let counts = |r: Option<Race>| {
        r.map_or_else(
            || "nenhum caso".to_owned(),
            |r| {
                format!(
                    "alvo primeiro {}, stop primeiro {}",
                    r.up_first, r.down_first
                )
            },
        )
    };
    let lines = vec![
        String::new(),
        format!(
            "Plano: comprar no fechamento de hoje, porque o funding está muito {} (mais {} que {:.0}% dos dias do ano)",
            if high { "alto" } else { "baixo" },
            if high { "alto" } else { "baixo" },
            if high {
                rank * 100.0
            } else {
                (1.0 - rank) * 100.0
            }
        ),
        format!(
            "  {}",
            if high {
                "Funding alto = quem está comprado está pagando caro para continuar comprado."
            } else {
                "Funding baixo = quem está vendido está pagando para continuar vendido."
            }
        ),
        format!(
            "  No passado, em dias assim: de 2019 a out/2024, {}; de nov/2024 até hoje, {}.",
            counts(dev),
            counts(fin)
        ),
        format!("  {}", verdict_pt(v)),
    ];
    Some((lines, Some((v, dev.map_or(0.0, |r| r.z()), high))))
}

/// Daily funding sums from the saved history, with the most recent days
/// replaced by complete sums from the live API. Never fails: without a file
/// or a connection it returns what it has.
async fn load_funding_live(
    data_dir: &Path,
    asset: &str,
    now_ms: i64,
) -> Option<BTreeMap<NaiveDate, f64>> {
    let mut daily = load_funding(data_dir, asset).unwrap_or_default();
    let start_day = daily
        .keys()
        .next_back()
        .and_then(NaiveDate::pred_opt)
        .unwrap_or_else(|| {
            DateTime::from_timestamp_millis(now_ms - 330 * 86_400_000)
                .map_or(NaiveDate::MIN, |d| d.date_naive())
        });
    let start_ms = start_day
        .and_hms_opt(0, 0, 0)
        .map_or(0, |t| t.and_utc().timestamp_millis());
    match fetch_funding_since(asset, start_ms).await {
        Ok(rows) => {
            let mut live: BTreeMap<NaiveDate, f64> = BTreeMap::new();
            for (ms, rate) in rows {
                if let Some(d) = DateTime::from_timestamp_millis(ms) {
                    *live.entry(d.date_naive()).or_insert(0.0) += rate;
                }
            }
            daily.extend(live);
        }
        Err(e) => eprintln!("warning: live funding for {asset} unavailable: {e}"),
    }
    (!daily.is_empty()).then_some(daily)
}

/// Long share of accounts at or above which the crowd counts as heavily long.
const CROWD_LONG: f64 = 0.65;
/// Long share of accounts at or below which the crowd counts as heavily short.
const CROWD_SHORT: f64 = 0.45;

/// Futures positioning line: long and short shares of accounts and of the top
/// traders' money, and the 24h open interest change. None when the futures
/// API cannot be reached.
async fn positioning_line(asset: &str) -> Option<String> {
    let accounts = fetch_long_short(asset, LongShort::Accounts, 25)
        .await
        .ok()?;
    let (_, now) = *accounts.last()?;
    let (_, before) = *accounts.first()?;
    let top = fetch_long_short(asset, LongShort::TopPositions, 1)
        .await
        .ok()
        .and_then(|r| r.last().map(|(_, v)| *v));
    let mut text = format!(
        "Futuros: {:.0}% das contas estão compradas e {:.0}% vendidas (24h atrás: {:.0}% compradas).",
        now * 100.0,
        (1.0 - now) * 100.0,
        before * 100.0
    );
    if let Some(t) = top {
        let _ = write!(
            text,
            " Entre os maiores traders, {:.0}% do dinheiro está comprado.",
            t * 100.0
        );
    }
    if let Some(oi) = open_interest_change(asset).await {
        let _ = write!(text, " Posições abertas: {} em 24h.", pct_pt(oi));
    }
    text.push_str(if now >= CROWD_LONG {
        " Muita gente comprada: se o preço cair, a liquidação desses comprados pode empurrar o preço mais para baixo."
    } else if now <= CROWD_SHORT {
        " Muita gente vendida: se o preço subir, a liquidação desses vendidos pode empurrar o preço mais para cima."
    } else {
        " Comprados e vendidos equilibrados."
    });
    text.push_str(
        " (Informação sem teste no histórico: a Binance só guarda 30 dias desses números.)",
    );
    Some(text)
}

/// Open interest change over the last 24 hours, in percent.
async fn open_interest_change(asset: &str) -> Option<f64> {
    let rows = fetch_open_interest(asset, 25).await.ok()?;
    let (_, first) = *rows.first()?;
    let (_, last) = *rows.last()?;
    (rows.len() >= 20 && first > 0.0).then(|| (last / first - 1.0) * 100.0)
}

/// Telegram HTML for a report section: everything escaped, with the asset
/// line, the plans, the action and the account line in bold.
fn section_html(section: &str) -> String {
    section
        .lines()
        .enumerate()
        .map(|(i, line)| {
            let escaped = html_escape(line);
            let bold = i == 0
                || line.starts_with("Plano:")
                || line.starts_with("O que fazer")
                || line.starts_with("Conta fictícia");
            if bold && !line.is_empty() {
                format!("<b>{escaped}</b>")
            } else {
                escaped
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Sends the report sections one by one, a little apart so Telegram does not
/// refuse them, retrying a failed one once.
async fn send_sections(token: &str, chat: &str, sections: &[String]) -> Result<(), Box<dyn Error>> {
    for (i, section) in sections.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        }
        let html = section_html(section);
        if let Err(first) = send_telegram_html(token, chat, &html).await {
            eprintln!(
                "warning: Telegram refused section {}: {first}; retrying",
                i + 1
            );
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            send_telegram_html(token, chat, &html).await?;
        }
        println!("telegram: section {} of {} sent", i + 1, sections.len());
    }
    Ok(())
}

/// Open interest change in the last hour, for the abnormal-candle alert.
async fn open_interest_hour(asset: &str) -> Option<String> {
    let rows = fetch_open_interest(asset, 2).await.ok()?;
    let (_, before) = *rows.first()?;
    let (_, now) = *rows.last()?;
    if rows.len() < 2 || before <= 0.0 {
        return None;
    }
    let change = (now / before - 1.0) * 100.0;
    let meaning = if change <= -2.0 {
        "sinal de liquidação em massa ou de muita gente fechando posição"
    } else if change >= 2.0 {
        "muita posição alavancada nova entrando"
    } else {
        "sem mudança grande"
    };
    Some(format!(
        " Posições abertas nos futuros na última hora: {} ({meaning}).",
        pct_pt(change)
    ))
}

fn cmd_funding_study(data_dir: &Path) -> CliResult {
    let split = held_out_start()?;
    for asset in ["BTCUSDT", "ETHUSDT", "SOLUSDT"] {
        let candles = read_series(&processed_dir(data_dir).join(format!("{asset}.csv")))?;
        let funding = load_funding(data_dir, asset)?;
        let atr = simple_atr(&candles, 14);
        let dates: Vec<NaiveDate> = candles.iter().map(Candle::date).collect();
        let ranks = funding_ranks(&dates, &funding);
        println!("{}:", asset_name(asset));
        for high in [true, false] {
            let (dev, fin) = funding_races(&candles, &atr, &ranks, high, split);
            println!(
                "  Funding {}, comprando no fechamento: de 2019 a out/2024, {}; de nov/2024 até o fim dos dados, {}. {}",
                if high {
                    "muito alto (10% maiores do ano)"
                } else {
                    "muito baixo (10% menores do ano)"
                },
                race_shares(dev),
                race_shares(fin),
                verdict_pt(funding_verdict(dev, fin))
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Days the paper account replays at most when the machine missed some runs.
const PAPER_MAX_REPLAY: i64 = 30;

fn date_pt(d: NaiveDate) -> String {
    d.format("%d/%m").to_string()
}

fn money_pt(v: f64) -> String {
    let sign = if v < 0.0 { "-" } else { "" };
    format!("{sign}${}", fmt_pt(v.abs()))
}

/// Runs the paper account over every closed day it has not seen yet and
/// returns its Telegram section. Saves the state and the trade journal unless
/// `dry_run`.
fn run_paper(
    live_dir: &Path,
    capital: f64,
    market: &BTreeMap<String, (Vec<Candle>, Analysis)>,
    dry_run: bool,
) -> Result<String, Box<dyn Error>> {
    let latest = market
        .values()
        .filter_map(|(c, _)| c.last().map(Candle::date))
        .min()
        .ok_or("no market data")?;
    let state_path = live_dir.join("paper.txt");
    let mut paper = match std::fs::read_to_string(&state_path) {
        Ok(text) => Paper::from_text(&text)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Paper::new(capital, latest.pred_opt().ok_or("date overflow")?)
        }
        Err(e) => return Err(e.into()),
    };
    let first = paper
        .last
        .succ_opt()
        .ok_or("date overflow")?
        .max(latest - chrono::Duration::days(PAPER_MAX_REPLAY - 1));
    let mut events: Vec<(NaiveDate, Event)> = Vec::new();
    let mut day = first;
    while day <= latest {
        let mut bars = BTreeMap::new();
        let mut signals = Vec::new();
        for (asset, (candles, analysis)) in market {
            let i = candles.partition_point(|c| c.date() < day);
            let Some(c) = candles.get(i).filter(|c| c.date() == day) else {
                continue;
            };
            bars.insert(
                asset.clone(),
                DayBar {
                    open: c.open(),
                    high: c.high(),
                    low: c.low(),
                    close: c.close(),
                },
            );
            if day == latest {
                signals.extend(analysis.signals.iter().cloned());
            } else {
                let past = &candles[..=i];
                signals.extend(
                    daily_report_with(asset, past, &close_values(past), analysis.funding.as_ref())
                        .signals,
                );
            }
        }
        for e in paper.step(day, &bars, &signals) {
            events.push((day, e));
        }
        day = day.succ_opt().ok_or("date overflow")?;
    }
    let closes: BTreeMap<String, f64> = market
        .iter()
        .filter_map(|(a, (c, _))| c.last().map(|c| (a.clone(), c.close())))
        .collect();
    let journal_path = live_dir.join("paper_trades.csv");
    let mut journal = match std::fs::read_to_string(&journal_path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let mut new_rows = String::new();
    if journal.is_empty() {
        new_rows.push_str(
            "asset,entry,entry_date,entry_price,exit_date,exit_price,quantity,pnl,reason\n",
        );
    }
    for (_, e) in &events {
        if let Event::Sold(t) = e {
            let _ = writeln!(new_rows, "{}", trade_csv(t));
        }
    }
    journal.push_str(&new_rows);
    let section = paper_section(&paper, &events, &closes, market, &journal, latest);
    if !dry_run {
        std::fs::create_dir_all(live_dir)?;
        std::fs::write(&state_path, paper.to_text())?;
        if !new_rows.is_empty() {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&journal_path)?;
            std::io::Write::write_all(&mut file, new_rows.as_bytes())?;
        }
    }
    Ok(section)
}

fn event_pt(day: NaiveDate, e: &Event) -> String {
    match e {
        Event::Bought(p) => format!(
            "{}: comprou {} a {} ({}). Alvo {}, stop {}, sai em no máximo 7 dias.",
            date_pt(day),
            asset_name(&p.asset),
            fmt_pt(p.entry_price),
            p.label,
            fmt_pt(p.target),
            fmt_pt(p.stop)
        ),
        Event::Placed(o) => format!(
            "{}: deixou ordem limite de compra de {} em {} para o dia seguinte ({}).",
            date_pt(day),
            asset_name(&o.asset),
            fmt_pt(o.price),
            o.label
        ),
        Event::Expired(o) => format!(
            "{}: a ordem de {} em {} não executou e foi cancelada.",
            date_pt(day),
            asset_name(&o.asset),
            fmt_pt(o.price)
        ),
        Event::Sold(t) => format!(
            "{}: vendeu {} a {} ({}). Resultado do trade: {} ({}).",
            date_pt(day),
            asset_name(&t.asset),
            fmt_pt(t.exit_price),
            t.reason.label_pt(),
            money_pt(t.pnl),
            pct_pt((t.exit_price / t.entry_price - 1.0) * 100.0)
        ),
    }
}

/// Trades in the journal text (header and blank lines skipped): exit date and profit.
fn journal_results(journal: &str) -> Vec<(NaiveDate, f64)> {
    journal
        .lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            Some((f.get(4)?.parse().ok()?, f.get(7)?.parse().ok()?))
        })
        .collect()
}

fn paper_section(
    paper: &Paper,
    events: &[(NaiveDate, Event)],
    closes: &BTreeMap<String, f64>,
    market: &BTreeMap<String, (Vec<Candle>, Analysis)>,
    journal: &str,
    latest: NaiveDate,
) -> String {
    let equity = paper.equity(closes);
    let results = journal_results(journal);
    let wins = results.iter().filter(|(_, p)| *p > 0.0).count();
    let mut out = vec![format!(
        "Conta fictícia (começou em {} com {}): vale {} hoje ({}). Trades fechados: {}, no lucro: {}.",
        date_pt(paper.start),
        money_pt(paper.capital),
        money_pt(equity),
        pct_pt((equity / paper.capital - 1.0) * 100.0),
        results.len(),
        wins
    )];
    for (day, e) in events {
        out.push(event_pt(*day, e));
    }
    match (&paper.position, &paper.pending) {
        (Some(p), _) => {
            let now = closes.get(&p.asset).copied().unwrap_or(p.entry_price);
            out.push(format!(
                "Posição aberta: {} comprado a {}, agora {} ({}). Alvo {}, stop {}, dia {} de 7.",
                asset_name(&p.asset),
                fmt_pt(p.entry_price),
                fmt_pt(now),
                pct_pt((now / p.entry_price - 1.0) * 100.0),
                fmt_pt(p.target),
                fmt_pt(p.stop),
                p.days
            ));
        }
        (None, Some(o)) => out.push(format!(
            "Ordem esperando: compra de {} em {} amanhã.",
            asset_name(&o.asset),
            fmt_pt(o.price)
        )),
        (None, None) => out.push("Sem posição: dinheiro parado em USDT.".to_owned()),
    }
    if let Some((candles, _)) = market.get("SOLUSDT") {
        let at = |d: NaiveDate| {
            let i = candles.partition_point(|c| c.date() < d);
            candles.get(i).filter(|c| c.date() == d).map(Candle::close)
        };
        if let (Some(start), Some(now)) = (at(paper.start), at(latest)) {
            let held = paper.capital * now / start;
            out.push(format!(
                "Comparação: só segurando SOL desde o início, teria {} ({}).",
                money_pt(held),
                pct_pt((now / start - 1.0) * 100.0)
            ));
        }
    }
    if latest.weekday() == chrono::Weekday::Sun {
        let week_start = latest - chrono::Duration::days(6);
        let week: Vec<f64> = results
            .iter()
            .filter(|(d, _)| *d >= week_start)
            .map(|(_, p)| *p)
            .collect();
        out.push(format!(
            "Resumo da semana: {} trades fechados, {} no lucro, resultado {}.",
            week.len(),
            week.iter().filter(|p| **p > 0.0).count(),
            money_pt(week.iter().sum())
        ));
    }
    out.join("\n")
}

/// Short name of a buy level from its order label.
fn level_name(label: &str) -> String {
    if label.contains("suporte") {
        "do suporte do último ano".to_owned()
    } else if label.contains("gap") {
        "do gap aberto".to_owned()
    } else {
        "da mínima de 20 dias".to_owned()
    }
}

/// Daily report of one asset.
#[derive(Debug, Default)]
struct Analysis {
    lines: Vec<String>,
    signals: Vec<Signal>,
    /// Buy levels below the price: name, price and historical result.
    levels: Vec<(String, f64, Verdict)>,
    /// Daily funding used for the report, kept for replaying past days.
    funding: Option<BTreeMap<NaiveDate, f64>>,
}

fn fmt_price(v: f64) -> String {
    if v >= 1000.0 {
        format!("{v:.0}")
    } else if v >= 1.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.6}")
    }
}

/// Hours back a news item may be published to still be alerted.
const NEWS_WINDOW_HOURS: i64 = 3;
/// News messages sent at most per hourly run.
const MAX_NEWS_MESSAGES: usize = 4;
/// Distance from 1.00 at which USDC/USDT counts as off its peg.
const DEPEG_LIMIT: f64 = 0.005;
/// Minutes before a scheduled event when the warning goes out.
const EVENT_WARN_MINUTES: i64 = 90;
/// Links remembered so a story is never sent twice.
const NEWS_MEMORY: usize = 3000;

/// Non-empty, non-comment lines of a config file (empty when missing).
fn config_lines(path: &str) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// Scheduled events from `config/events.csv` lines (date, UTC time, description).
fn parse_events(lines: &[String]) -> Vec<(DateTime<chrono::Utc>, String)> {
    lines
        .iter()
        .filter_map(|l| {
            let mut f = l.splitn(3, ',');
            let date: NaiveDate = f.next()?.trim().parse().ok()?;
            let time = chrono::NaiveTime::parse_from_str(f.next()?.trim(), "%H:%M").ok()?;
            let what = f.next()?.trim().to_owned();
            Some((date.and_time(time).and_utc(), what))
        })
        .collect()
}

/// Date and time in Brasília (UTC-3, no daylight saving since 2019).
fn brt_pt(t: DateTime<chrono::Utc>) -> String {
    (t - chrono::Duration::hours(3))
        .format("%d/%m às %Hh%M")
        .to_string()
}

/// Agenda line for the daily report: events in the next 7 days.
fn agenda_line(now: DateTime<chrono::Utc>) -> Option<String> {
    let events = parse_events(&config_lines("config/events.csv"));
    let soon: Vec<String> = events
        .iter()
        .filter(|(t, _)| *t > now && *t <= now + chrono::Duration::days(7))
        .map(|(t, what)| format!("{what} em {}", brt_pt(*t)))
        .collect();
    (!soon.is_empty()).then(|| {
        format!(
            "Agenda dos próximos 7 dias (eventos que costumam mexer forte no preço): {}.",
            soon.join("; ")
        )
    })
}

/// Warnings for scheduled events starting within [`EVENT_WARN_MINUTES`].
fn event_alerts(
    now: DateTime<chrono::Utc>,
    events: &[(DateTime<chrono::Utc>, String)],
) -> Vec<(String, String)> {
    events
        .iter()
        .filter(|(t, _)| *t > now && *t - now <= chrono::Duration::minutes(EVENT_WARN_MINUTES))
        .map(|(t, what)| {
            (
                format!("evento-{}", t.timestamp()),
                format!(
                    "Daqui a pouco: {what} em {} (horário de Brasília). Costuma mexer forte no preço nos minutos seguintes; ordens apertadas podem ser executadas no susto.",
                    brt_pt(*t)
                ),
            )
        })
        .collect()
}

/// Price change over the last `hours` hourly bars, in percent.
fn change_over(hourly: &[Bar], hours: usize) -> Option<f64> {
    let last = hourly.last()?;
    let before = hourly.get(hourly.len().checked_sub(hours + 1)?)?;
    Some((last.close / before.close - 1.0) * 100.0)
}

fn price_line(coins: Coins, hourly: &BTreeMap<String, Vec<Bar>>) -> String {
    let mut names = Vec::new();
    for (on, asset) in [
        (coins.btc, "BTCUSDT"),
        (coins.eth, "ETHUSDT"),
        (coins.sol, "SOLUSDT"),
    ] {
        if on {
            names.push(asset);
        }
    }
    if names.is_empty() {
        names.push("BTCUSDT");
    }
    let parts: Vec<String> = names
        .iter()
        .filter_map(|a| {
            let h = hourly.get(*a)?;
            Some(format!(
                "{} {} na última hora e {} em 24h",
                asset_name(a),
                pct_pt(change_over(h, 1)?),
                pct_pt(change_over(h, 24)?)
            ))
        })
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("\nPreço agora: {}.", parts.join("; "))
    }
}

fn domain_of(link: &str) -> &str {
    let rest = link.split_once("://").map_or(link, |(_, r)| r);
    let host = rest.split('/').next().unwrap_or(rest);
    host.strip_prefix("www.").unwrap_or(host)
}

/// News alerts from the configured feeds: recent stories that name BTC, ETH
/// or SOL (or are market-wide) and fit a price-moving kind, grouped by kind
/// and coins, most serious first. Returns the messages and the new links.
async fn news_alerts(
    now: DateTime<chrono::Utc>,
    sent: &BTreeSet<String>,
    hourly: &BTreeMap<String, Vec<Bar>>,
) -> (Vec<String>, Vec<String>) {
    let mut groups: BTreeMap<(Kind, String), Vec<(NewsItem, Coins)>> = BTreeMap::new();
    let mut new_links = Vec::new();
    for url in config_lines("config/news_feeds.txt") {
        let items = match fetch_feed(&url).await {
            Ok(items) => items,
            Err(e) => {
                eprintln!("warning: news feed skipped: {e}");
                continue;
            }
        };
        for item in items {
            let recent = item
                .published
                .is_some_and(|p| p <= now && now - p <= chrono::Duration::hours(NEWS_WINDOW_HOURS));
            if !recent || sent.contains(&item.link) || new_links.contains(&item.link) {
                continue;
            }
            let Some(c) = classify_headline(&item.title, &item.summary) else {
                continue;
            };
            new_links.push(item.link.clone());
            groups
                .entry((c.kind, c.coins.label_pt()))
                .or_default()
                .push((item, c.coins));
        }
    }
    let messages = groups
        .into_iter()
        .take(MAX_NEWS_MESSAGES)
        .map(|((kind, coins), items)| {
            let sources: BTreeSet<&str> = items.iter().map(|(i, _)| domain_of(&i.link)).collect();
            let mut msg = format!(
                "NOTÍCIA: {} ({coins}), {} {}:",
                kind.label_pt(),
                sources.len(),
                if sources.len() == 1 {
                    "fonte"
                } else {
                    "fontes"
                }
            );
            for (item, _) in items.iter().take(3) {
                let _ = write!(
                    msg,
                    "\n- {} ({}{}) {}",
                    item.title,
                    domain_of(&item.link),
                    item.published
                        .map_or_else(String::new, |p| format!(", {}", brt_pt(p))),
                    item.link
                );
            }
            msg.push_str(&price_line(items[0].1, hourly));
            msg
        })
        .collect();
    (messages, new_links)
}

/// Alert when USDC trades away from one USDT.
async fn depeg_alert(now_ms: i64) -> Option<(String, String)> {
    let bars = fetch_recent_hourly("USDCUSDT", 3, now_ms).await.ok()?;
    let price = bars.last()?.close;
    if (price - 1.0).abs() < DEPEG_LIMIT {
        return None;
    }
    Some((
        "usdc-paridade".to_owned(),
        format!(
            "Stablecoin fora da paridade: USDC/USDT está em {} ({} do 1 a 1). Isso indica problema em uma das duas stablecoins; confira as notícias antes de operar.",
            format!("{price:.4}").replace('.', ","),
            pct_pt((price - 1.0) * 100.0)
        ),
    ))
}

/// Hours of recent bars fetched each run: the 30-day baseline plus a margin.
const ALERT_FETCH_HOURS: i64 = 760;
/// Recent hours checked for a crossed 20- or 60-day level.
const CROSS_HOURS: usize = 3;
/// How close (share of price) an hourly low must come to a level to count as reaching it.
const NEAR_LEVEL: f64 = 0.005;

/// Every alert for one asset this hour, as (dedup name, message).
async fn alerts_for(
    data_dir: &Path,
    asset: &str,
    now_ms: i64,
) -> Result<(Vec<(String, String)>, Vec<Bar>), Box<dyn Error>> {
    let mut daily = read_series(&processed_dir(data_dir).join(format!("{asset}.csv")))?;
    let last = daily.last().map(Candle::date).ok_or("empty history")?;
    let since = last.succ_opt().ok_or("date overflow")?;
    let recent = fetch_daily_since(asset, since, now_ms).await?;
    extend_series(&mut daily, &recent)?;
    let hourly = fetch_recent_hourly(asset, ALERT_FETCH_HOURS, now_ms).await?;
    let mut out = Vec::new();
    let cross = &hourly[hourly.len().saturating_sub(CROSS_HOURS)..];
    out.extend(alert_for(&daily, cross, asset));
    let history_path = data_dir
        .join("processed/binance/futures_1h")
        .join(format!("{asset}.csv"));
    let history = std::fs::read_to_string(&history_path)
        .ok()
        .and_then(|t| parse_bars_csv(&t).ok());
    if let Some((key, mut msg)) = anomaly_alert(asset, &hourly, history.as_deref()) {
        if let Some(oi) = open_interest_hour(asset).await {
            msg.push_str(&oi);
        }
        out.push((key, msg));
    }
    let closes = close_values(&daily);
    let analysis = daily_report(asset, &daily, &closes);
    out.extend(level_alerts(asset, &analysis, cross));
    Ok((out, hourly))
}

/// Alert when the last closed hour is abnormal, with what followed such
/// hours in the asset's hourly history.
fn anomaly_alert(asset: &str, hourly: &[Bar], history: Option<&[Bar]>) -> Option<(String, String)> {
    let i = hourly.len().checked_sub(1)?;
    let spike = spike_at(hourly, i).filter(Spike::is_abnormal)?;
    let bar = hourly[i];
    let name = asset_name(asset);
    let brt = |ms: i64| {
        DateTime::from_timestamp_millis(ms - 3 * HOUR_MS)
            .map_or_else(String::new, |d| d.format("%Hh").to_string())
    };
    let change = (bar.close / bar.open - 1.0) * 100.0;
    let mut msg = format!(
        "{name}: vela de 1 hora fora do normal ({} às {}, horário de Brasília): {} {}, tamanho {} vezes o normal de uma hora, volume {} vezes o normal.",
        brt(bar.open_time_ms),
        brt(bar.open_time_ms + HOUR_MS),
        if spike.up { "subiu" } else { "caiu" },
        format!("{:.1}%", change.abs()).replace('.', ","),
        format!("{:.1}", spike.range_ratio).replace('.', ","),
        format!("{:.1}", spike.volume_ratio).replace('.', ",")
    );
    if spike.range_ratio < ANOMALY_RATIO {
        msg.push_str(" Só o volume está fora do normal; o tamanho da vela não, então o histórico de velas grandes não se aplica.");
    } else if let Some(h) = history {
        let f = follow_after_spikes(h, spike.up);
        let n = f.total();
        if n > 0 {
            let pct = |k: usize| as_share(k) / as_share(n) * 100.0;
            let range_pct =
                format!("{:.1}%", (bar.high - bar.low) / bar.close * 100.0).replace('.', ",");
            let decided = f.continued + f.reversed;
            let verdict = if decided < MIN_RACES {
                "Conclusão: poucos casos para dizer."
            } else if f.z() >= RACE_Z {
                "Conclusão: costuma continuar."
            } else if f.z() <= -RACE_Z {
                "Conclusão: costuma voltar."
            } else {
                "Conclusão: empate, igual cara ou coroa."
            };
            let _ = write!(
                msg,
                " Nas {n} vezes em que apareceu uma vela assim no histórico, nas {FOLLOW_HOURS} horas seguintes o preço andou mais um tamanho de vela (~{range_pct}) na mesma direção antes de devolver a vela inteira em {:.0}% das vezes, e devolveu primeiro em {:.0}%. {verdict}",
                pct(f.continued),
                pct(f.reversed)
            );
        }
    }
    Some((format!("anomalia-{}", bar.open_time_ms), msg))
}

/// Alert when an hourly low in the last hours reached (or came within
/// [`NEAR_LEVEL`] of) one of the report's buy levels, with that level's result.
fn level_alerts(asset: &str, analysis: &Analysis, recent: &[Bar]) -> Vec<(String, String)> {
    let Some(low) = recent.iter().map(|b| b.low).reduce(f64::min) else {
        return Vec::new();
    };
    let price = recent.last().map_or(low, |b| b.close);
    let name = asset_name(asset);
    analysis
        .levels
        .iter()
        .filter(|(_, level, _)| low <= level * (1.0 + NEAR_LEVEL))
        .map(|(label, level, v)| {
            (
                format!("nivel-{label}"),
                format!(
                    "{name} chegou perto {label} em {} (mínima das últimas horas {}, agora {}). No histórico, comprar ali: {}",
                    fmt_pt(*level),
                    fmt_pt(low),
                    fmt_pt(price),
                    verdict_pt(*v).trim_start_matches("Resultado: ")
                ),
            )
        })
        .collect()
}

#[allow(clippy::too_many_lines)] // one pass per asset: levels, targets, message
fn alert_for(daily: &[Candle], hourly: &[Bar], asset: &str) -> Option<(String, String)> {
    let n = daily.len();
    let price = hourly.last().map(|b| b.close)?;
    if n < 120 {
        return None;
    }
    let atr = simple_atr(daily, 14);
    let a = atr[n - 1].filter(|a| *a > 0.0)?;
    let extremes = |days: usize| {
        let w = &daily[n - days..];
        (
            w.iter().map(Candle::high).fold(f64::MIN, f64::max),
            w.iter().map(Candle::low).fold(f64::MAX, f64::min),
        )
    };
    let (high20, low20) = extremes(20);
    let (high60, low60) = extremes(60);
    let (far_high, far_low) = extremes(120);
    let crossed = |level: f64, up: bool| {
        hourly.windows(2).any(|w| {
            if up {
                w[0].close <= level && w[1].close > level
            } else {
                w[0].close >= level && w[1].close < level
            }
        })
    };
    let breaks = [
        ("máxima de 60 dias", high60, true),
        ("máxima de 20 dias", high20, true),
        ("mínima de 60 dias", low60, false),
        ("mínima de 20 dias", low20, false),
    ];
    let Some(&(name, level, up)) = breaks.iter().find(|(_, l, u)| crossed(*l, *u)) else {
        println!(
            "{asset} {}: sem rompimento (máxima de 20 dias {}, mínima de 20 dias {})",
            fmt_price(price),
            fmt_price(high20),
            fmt_price(low20)
        );
        return None;
    };
    let mut targets: Vec<(&str, f64)> = if up {
        let r = round_above(price);
        vec![
            ("máxima de 60 dias", high60),
            ("máxima de 120 dias", far_high),
            ("número redondo", r),
            ("número redondo", round_above(r)),
        ]
    } else {
        let r = round_below(price);
        vec![
            ("mínima de 60 dias", low60),
            ("mínima de 120 dias", far_low),
            ("número redondo", r),
            ("número redondo", round_below(r)),
        ]
    };
    targets.retain(|(_, l)| {
        if up {
            *l > price
        } else {
            *l < price && *l > 0.0
        }
    });
    targets.sort_by(|x, y| {
        if up {
            x.1.total_cmp(&y.1)
        } else {
            y.1.total_cmp(&x.1)
        }
    });
    targets.dedup_by(|x, y| (x.1 - y.1).abs() <= 1e-9 * y.1.abs());
    let (_, cases) = odds_after_break(daily, &atr, up, 7, 0.0).unwrap_or((0.0, 0));
    let mut msg = format!(
        "{asset} rompeu a {name} ({}). Último fechamento de 1 hora: {}.\nPróximos pontos de liquidez {} e quantas vezes o preço chegou lá em até 7 dias depois de romper a {} de 20 dias ({cases} casos no histórico):",
        fmt_price(level),
        fmt_price(price),
        if up { "acima" } else { "abaixo" },
        if up { "máxima" } else { "mínima" }
    );
    for (label, target) in targets.iter().take(4) {
        let distance = (target - price).abs() / a;
        let (p, _) = odds_after_break(daily, &atr, up, 7, distance).unwrap_or((0.0, 0));
        let _ = write!(
            msg,
            "\n  {label} {} ({:+.1}%): {:.0}%",
            fmt_price(*target),
            (target / price - 1.0) * 100.0,
            p * 100.0
        );
    }
    if !up {
        msg.push_str(
            "\n  atenção: nos testes, romper a mínima de 20 dias não levou a queda extra na média",
        );
    }
    Some((name.to_owned(), msg))
}

async fn cmd_alerts(data_dir: &Path, notify: bool, dry_run: bool) -> CliResult {
    let since_epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    let now_ms = i64::try_from(since_epoch.as_millis())?;
    let today = DateTime::from_timestamp_millis(now_ms)
        .map(|d| d.date_naive())
        .ok_or("invalid clock")?;
    let path = data_dir.join("live").join("alerts_sent.csv");
    let sent: BTreeSet<String> = match std::fs::read_to_string(&path) {
        Ok(text) => text.lines().map(str::to_owned).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
        Err(e) => return Err(e.into()),
    };
    let mut messages = Vec::new();
    let mut keys = String::new();
    let mut hourly_by_asset = BTreeMap::new();
    let now = DateTime::from_timestamp_millis(now_ms).ok_or("invalid clock")?;
    let mut market_alerts: Vec<(String, String)> =
        event_alerts(now, &parse_events(&config_lines("config/events.csv")));
    market_alerts.extend(depeg_alert(now_ms).await);
    for (name, msg) in market_alerts {
        let key = format!("{today},mercado,{name}");
        if sent.contains(&key) {
            continue;
        }
        println!("{msg}");
        messages.push(msg);
        let _ = writeln!(keys, "{key}");
    }
    for asset in ["BTCUSDT", "ETHUSDT", "SOLUSDT"] {
        let (alerts, hourly) = alerts_for(data_dir, asset, now_ms).await?;
        hourly_by_asset.insert(asset.to_owned(), hourly);
        for (name, msg) in alerts {
            let key = format!("{today},{asset},{name}");
            if sent.contains(&key) {
                println!("{asset}: {name} já avisado hoje");
                continue;
            }
            println!("{msg}");
            messages.push(msg);
            let _ = writeln!(keys, "{key}");
        }
    }
    let news_path = data_dir.join("live").join("news_sent.txt");
    let news_sent: BTreeSet<String> = std::fs::read_to_string(&news_path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    let (news, new_links) = news_alerts(now, &news_sent, &hourly_by_asset).await;
    for msg in news {
        println!("{msg}");
        messages.push(msg);
    }
    if dry_run {
        println!("dry run: nothing saved, nothing sent");
        return Ok(ExitCode::SUCCESS);
    }
    if messages.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    if notify {
        let env = parse_env(&std::fs::read_to_string(".env")?);
        let (Some(token), Some(chat)) =
            (env.get("TELEGRAM_BOT_TOKEN"), env.get("TELEGRAM_CHAT_ID"))
        else {
            return Err("TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID must be set in .env".into());
        };
        send_telegram(token, chat, &messages.join("\n\n")).await?;
        println!("telegram: sent");
    }
    std::fs::create_dir_all(data_dir.join("live"))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    std::io::Write::write_all(&mut file, keys.as_bytes())?;
    if !new_links.is_empty() {
        let mut all: Vec<String> = std::fs::read_to_string(&news_path)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect();
        all.extend(new_links);
        let keep = all.len().saturating_sub(NEWS_MEMORY);
        std::fs::write(&news_path, all[keep..].join("\n") + "\n")?;
    }
    Ok(ExitCode::SUCCESS)
}

fn default_trend_start() -> Result<NaiveDate, Box<dyn Error>> {
    Ok(NaiveDate::from_ymd_opt(2019, 1, 1).ok_or("invalid date")?)
}

fn print_trend_row(
    asset: &str,
    model: &str,
    r: &AllocationResult,
    s: &AllocationStats,
    passes: Option<bool>,
) {
    println!(
        "{asset:<8} {model:<18} CAGR {:>+6.1}% | Sharpe {:>5.2} | maxDD {:>5.1}% | non-losing years {:>3.0}% | exposure {:>4.0}% | trades {:>4} | {}",
        s.cagr * 100.0,
        s.sharpe,
        s.max_drawdown * 100.0,
        s.non_losing_year_fraction * 100.0,
        r.average_exposure * 100.0,
        r.trades,
        passes.map_or("benchmark", |p| if p { "PASSES" } else { "fails" })
    );
}

fn cmd_trend(
    data_dir: &Path,
    assets: &[String],
    start: Option<NaiveDate>,
    end: Option<NaiveDate>,
) -> CliResult {
    let processed = processed_dir(data_dir);
    let start = start.map_or_else(default_trend_start, Ok)?;
    let end = end.map_or_else(held_out_start, Ok)?;
    let config = AllocationConfig::crypto();
    let vol = VolTarget::paper_spot();
    println!(
        "period {start} to {end} | 0.1% fee + 0.1% slippage per side | 25% vol target, 90-day vol | rebalance threshold 20%"
    );
    println!(
        "criteria: Sharpe >= 1.0, maxDD <= 25%, non-losing years >= 60%, Sharpe above buy & hold"
    );
    for asset in assets {
        let candles = read_series(&processed.join(format!("{asset}.csv")))?;
        if segments(&candles).len() > 1 {
            return Err(format!("{asset} has gaps; trend models need continuous data").into());
        }
        let closes = close_values(&candles);
        let hold = run_allocation(&candles, &vec![1.0; closes.len()], &config, start, end);
        let hold_stats = allocation_stats(&hold.equity, config.initial_capital);
        print_trend_row(asset, "buy & hold", &hold, &hold_stats, None);
        let mut models = vec![(
            "Donchian ensemble".to_owned(),
            donchian_ensemble_weights(&closes, &vol),
        )];
        for period in [40, 50, 100, 150] {
            models.push((
                format!("close > SMA{period}"),
                sma_trend_weights(&closes, period, &vol),
            ));
        }
        let mut sma_sharpes = Vec::new();
        for (name, weights) in &models {
            let r = run_allocation(&candles, weights, &config, start, end);
            let s = allocation_stats(&r.equity, config.initial_capital);
            if name.starts_with("close") {
                sma_sharpes.push(s.sharpe);
            }
            let passes = s.sharpe >= 1.0
                && s.max_drawdown <= 0.25
                && s.non_losing_year_fraction >= 0.6
                && s.sharpe > hold_stats.sharpe;
            print_trend_row(asset, name, &r, &s, Some(passes));
        }
        let robust = sma_sharpes.iter().all(|s| *s >= 0.8);
        println!(
            "{asset}: SMA family robustness (every Sharpe >= 0.8): {}",
            if robust { "yes" } else { "no" }
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn neighborhood_r(results: &[(Variant, Evaluation)], v: Variant) -> f64 {
    let near: Vec<f64> = results
        .iter()
        .filter(|(other, _)| other.fvg == v.fvg && other.distance(v) <= 1)
        .map(|(_, e)| e.summary.average_r)
        .collect();
    let count = f64::from(u32::try_from(near.len()).unwrap_or(u32::MAX));
    near.iter().sum::<f64>() / count.max(1.0)
}

fn print_grid(results: &[(Variant, Evaluation)]) {
    let mut csv = String::from(
        "variant,trades,win_rate,average_r,profit_factor,max_drawdown,total_return,non_losing_years,r_without_top3,r_without_best_symbol,neighborhood_r,passes\n",
    );
    println!(
        "{:<36} {:>6} {:>6} {:>7} {:>5} {:>6} {:>8} {:>6} {:>7} {:>7} {:>7}  verdict",
        "variant",
        "trades",
        "win%",
        "avgR",
        "PF",
        "maxDD",
        "return",
        "years",
        "-top3",
        "-best",
        "nbhdR"
    );
    for &(v, e) in results {
        let s = e.summary;
        let nbhd = neighborhood_r(results, v);
        println!(
            "{:<36} {:>6} {:>5.1}% {:>+7.3} {:>5.2} {:>5.1}% {:>+7.1}% {:>5.0}% {:>+7.1} {:>+7.1} {:>+7.3}  {}",
            v.label(),
            s.trades,
            s.win_rate * 100.0,
            s.average_r,
            s.profit_factor,
            s.max_drawdown * 100.0,
            s.total_return * 100.0,
            e.non_losing_year_fraction * 100.0,
            e.r_without_top3,
            e.r_without_best_symbol,
            nbhd,
            if e.passes { "PASSES" } else { "fails" }
        );
        let _ = writeln!(
            csv,
            "{},{},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.2},{:.2},{:.4},{}",
            v.label(),
            s.trades,
            s.win_rate,
            s.average_r,
            s.profit_factor,
            s.max_drawdown,
            s.total_return,
            e.non_losing_year_fraction,
            e.r_without_top3,
            e.r_without_best_symbol,
            nbhd,
            e.passes
        );
    }
    let best = results
        .iter()
        .filter(|(_, e)| e.passes)
        .max_by(|a, b| neighborhood_r(results, a.0).total_cmp(&neighborhood_r(results, b.0)));
    match best {
        Some((v, _)) => println!(
            "selected (passes, best neighborhood): {} | neighborhood avg R {:+.3}",
            v.label(),
            neighborhood_r(results, *v)
        ),
        None => println!("no variant passes the acceptance criteria"),
    }
    if std::fs::create_dir_all("reports").is_ok() && std::fs::write("reports/grid.csv", csv).is_ok()
    {
        println!("written: reports/grid.csv");
    }
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use chrono::Days;
    use tp_core::market::CandleFields;

    fn walk(days: u64) -> Vec<Candle> {
        // Deterministic zig-zag drift so every report branch has data.
        let start = NaiveDate::from_ymd_opt(2021, 1, 1).unwrap();
        let mut price = 100.0_f64;
        (0..days)
            .map(|i| {
                let step = f64::from(u32::try_from((i * 7919) % 13).unwrap()) - 6.0;
                let open = price;
                price = (price * (1.0 + step / 200.0)).max(1.0);
                Candle::try_new(CandleFields {
                    date: start.checked_add_days(Days::new(i)).unwrap(),
                    open,
                    high: open.max(price) * 1.01,
                    low: open.min(price) * 0.99,
                    close: price,
                    volume: 1.0,
                    quote_volume: 1.0,
                })
                .unwrap()
            })
            .collect()
    }

    #[test]
    fn prices_use_brazilian_notation() {
        assert_eq!(fmt_pt(77_453.86), "77.454");
        assert_eq!(fmt_pt(1_234_567.0), "1.234.567");
        assert_eq!(fmt_pt(95.5), "95,50");
        assert_eq!(fmt_pt(0.5), "0,500000");
        assert_eq!(pct_pt(-2.04), "-2,0%");
        assert_eq!(asset_name("SOLUSDT"), "SOL");
    }

    #[test]
    fn daily_report_covers_trend_candle_odds_and_action() {
        let candles = walk(900);
        let closes = close_values(&candles);
        let report = daily_report("SOLUSDT", &candles, &closes).lines;
        println!("{}", report.join("\n"));
        assert!(report[0].starts_with("SOL — dia fechado em "));
        assert!(report[1].starts_with("Tendência: "));
        assert!(
            report
                .iter()
                .any(|l| l.starts_with("Plano: ordem limite em"))
        );
        assert!(
            report
                .iter()
                .any(|l| l.starts_with("Plano: comprar se um dia fechar acima"))
        );
        assert!(
            report.last().unwrap().contains("O que fazer")
                || report.iter().any(|l| l.starts_with("O que fazer"))
        );
    }

    #[test]
    fn daily_report_needs_enough_history() {
        let candles = walk(30);
        let closes = close_values(&candles);
        assert!(daily_report("BTCUSDT", &candles, &closes).lines.is_empty());
    }

    fn path(points: &[(u64, f64)]) -> Vec<Candle> {
        // Piecewise path through (day, price) points with a daily wiggle.
        let start = NaiveDate::from_ymd_opt(2021, 1, 1).unwrap();
        let mut out = Vec::new();
        let mut prev = points[0].1;
        for w in points.windows(2) {
            let (d0, p0) = w[0];
            let (d1, p1) = w[1];
            for d in d0..d1 {
                let f = f64::from(u32::try_from(d - d0).unwrap())
                    / f64::from(u32::try_from(d1 - d0).unwrap());
                let wiggle = if d % 3 == 0 {
                    1.02
                } else if d % 3 == 1 {
                    0.985
                } else {
                    1.0
                };
                let close = (p0 + (p1 - p0) * f) * wiggle;
                let open = prev;
                out.push(
                    Candle::try_new(CandleFields {
                        date: start.checked_add_days(Days::new(d)).unwrap(),
                        open,
                        high: open.max(close) * 1.01,
                        low: open.min(close) * 0.99,
                        close,
                        volume: 1.0,
                        quote_volume: 1.0,
                    })
                    .unwrap(),
                );
                prev = close;
            }
        }
        out
    }

    #[test]
    fn report_sees_old_support_and_open_gap() {
        let mut points = vec![(0, 90.0)];
        // A long range between 80 and 95, a drop, a base, a jump and a pullback.
        for k in 0..12 {
            points.push((10 + k * 20, if k % 2 == 0 { 80.0 } else { 95.0 }));
        }
        points.extend([
            (260, 81.0),
            (280, 62.0),
            (300, 76.0),
            (330, 74.0),
            (336, 108.0),
            (380, 124.0),
            (420, 92.0),
        ]);
        let candles = path(&points);
        let closes = close_values(&candles);
        let report = daily_report("SOLUSDT", &candles, &closes).lines;
        println!("{}", report.join("\n"));
        assert!(report.iter().any(|l| l.contains("(suporte do último ano")));
    }

    #[test]
    fn paper_runs_over_new_days_and_saves_state() {
        let mut points = vec![(0, 90.0)];
        for k in 0..12 {
            points.push((10 + k * 20, if k % 2 == 0 { 80.0 } else { 95.0 }));
        }
        points.extend([
            (260, 81.0),
            (280, 62.0),
            (300, 76.0),
            (330, 74.0),
            (336, 108.0),
            (380, 124.0),
            (420, 92.0),
        ]);
        let candles = path(&points);
        let dir = std::env::temp_dir().join(format!("tp-paper-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let split = candles.len() - 5;
        let market_at = |n: usize| {
            let c = candles[..n].to_vec();
            let a = daily_report("SOLUSDT", &c, &close_values(&c));
            BTreeMap::from([("SOLUSDT".to_owned(), (c, a))])
        };
        let first = run_paper(&dir, 2200.0, &market_at(split), false).unwrap();
        println!("{first}");
        assert!(first.starts_with("Conta fictícia (começou em "));
        let saved =
            Paper::from_text(&std::fs::read_to_string(dir.join("paper.txt")).unwrap()).unwrap();
        assert_eq!(saved.last, candles[split - 1].date());
        let second = run_paper(&dir, 2200.0, &market_at(candles.len()), false).unwrap();
        println!("{second}");
        let saved =
            Paper::from_text(&std::fs::read_to_string(dir.join("paper.txt")).unwrap()).unwrap();
        assert_eq!(saved.last, candles[candles.len() - 1].date());
        assert_eq!(saved.capital, 2200.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn journal_results_read_exit_date_and_profit() {
        let text = "asset,entry,entry_date,entry_price,exit_date,exit_price,quantity,pnl,reason\nSOLUSDT,x,2026-11-01,90,2026-11-03,94,10,39.50,alvo\n";
        assert_eq!(
            journal_results(text),
            vec![(NaiveDate::from_ymd_opt(2026, 11, 3).unwrap(), 39.5)]
        );
    }

    #[test]
    fn paper_events_read_in_portuguese() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        let mut paper = Paper::new(2200.0, day.pred_opt().unwrap());
        let bars = BTreeMap::from([(
            "SOLUSDT".to_owned(),
            DayBar {
                open: 108.0,
                high: 111.0,
                low: 107.0,
                close: 110.0,
            },
        )]);
        let signal = Signal {
            asset: "SOLUSDT".to_owned(),
            label: "compra agora: o dia fechou acima da máxima de 20 dias".to_owned(),
            order: Order::AtClose,
            atr: 4.0,
            z: 2.8,
        };
        let events = paper.step(day, &bars, &[signal]);
        assert_eq!(
            event_pt(day, &events[0]),
            "05/11: comprou SOL a 110,00 (compra agora: o dia fechou acima da máxima de 20 dias). Alvo 114,00, stop 106,00, sai em no máximo 7 dias."
        );
        assert_eq!(money_pt(-12.5), "-$12,50");
    }

    fn hour_bar(i: usize, open: f64, high: f64, low: f64, close: f64, volume: f64) -> Bar {
        Bar {
            open_time_ms: 1_762_300_800_000 + i64::try_from(i).unwrap() * HOUR_MS,
            open,
            high,
            low,
            close,
            volume,
        }
    }

    #[test]
    fn anomaly_alert_describes_a_big_hour_and_its_history() {
        let mut hourly: Vec<Bar> = (0..720)
            .map(|i| hour_bar(i, 100.0, 100.5, 99.5, 100.0, 10.0))
            .collect();
        hourly.push(hour_bar(720, 100.0, 100.2, 96.0, 96.5, 60.0));
        let (key, msg) = anomaly_alert("SOLUSDT", &hourly, Some(&hourly)).unwrap();
        println!("{msg}");
        assert!(key.starts_with("anomalia-"));
        assert!(msg.starts_with("SOL: vela de 1 hora fora do normal"));
        assert!(msg.contains("caiu 3,5%"));
        assert!(msg.contains("volume 6,0 vezes"));
        // A normal last hour gives no alert.
        hourly.push(hour_bar(721, 96.5, 97.0, 96.0, 96.5, 10.0));
        assert!(anomaly_alert("SOLUSDT", &hourly, None).is_none());
    }

    #[test]
    fn level_alert_fires_when_the_low_comes_near_a_level() {
        let analysis = Analysis {
            lines: Vec::new(),
            signals: Vec::new(),
            levels: vec![("do suporte do último ano".to_owned(), 83.74, Verdict::Tie)],
            funding: None,
        };
        let near = [hour_bar(0, 85.0, 85.2, 84.1, 84.5, 1.0)];
        let alerts = level_alerts("SOLUSDT", &analysis, &near);
        assert_eq!(alerts.len(), 1);
        println!("{}", alerts[0].1);
        assert!(
            alerts[0]
                .1
                .starts_with("SOL chegou perto do suporte do último ano em 83,74")
        );
        let far = [hour_bar(0, 90.0, 90.2, 89.0, 89.5, 1.0)];
        assert!(level_alerts("SOLUSDT", &analysis, &far).is_empty());
    }

    #[test]
    fn funding_verdict_needs_both_periods() {
        let race = |up: usize, down: usize| {
            Some(Race {
                up_first: up,
                down_first: down,
                neither: 0,
            })
        };
        assert_eq!(funding_verdict(race(40, 15), race(8, 4)), Verdict::Up);
        assert_eq!(funding_verdict(race(40, 15), race(4, 8)), Verdict::Tie);
        assert_eq!(funding_verdict(race(15, 40), race(3, 9)), Verdict::Down);
        assert_eq!(funding_verdict(race(40, 15), race(2, 1)), Verdict::FewCases);
        assert_eq!(funding_verdict(None, race(8, 4)), Verdict::FewCases);
    }

    #[test]
    fn funding_lines_rank_today_and_judge_extremes() {
        let candles = walk(900);
        let atr = simple_atr(&candles, 14);
        let last = candles.len() - 1;
        // Funding grows every day, so the last day is the highest of its year.
        let funding: BTreeMap<NaiveDate, f64> = candles
            .iter()
            .enumerate()
            .map(|(i, c)| (c.date(), f64::from(u32::try_from(i).unwrap()) * 1e-6))
            .collect();
        let (lines, extreme) = funding_lines(&candles, &atr, &funding, last).unwrap();
        println!("{}", lines.join("\n"));
        assert!(
            lines[1].starts_with("Plano: comprar no fechamento de hoje, porque o funding está muito alto (mais alto que 100% dos dias"),
            "{}",
            lines[1]
        );
        assert!(extreme.is_some_and(|(_, _, high)| high));
        let flat: BTreeMap<NaiveDate, f64> = candles.iter().map(|c| (c.date(), 0.0001)).collect();
        let (lines, extreme) = funding_lines(&candles, &atr, &flat, last).unwrap();
        assert!(lines.is_empty());
        assert!(extreme.is_none());
    }

    #[test]
    fn events_parse_and_warn_shortly_before() {
        let lines = vec![
            "2026-11-10,13:30,Inflação dos EUA (CPI)".to_owned(),
            "bad line".to_owned(),
        ];
        let events = parse_events(&lines);
        assert_eq!(events.len(), 1);
        assert_eq!(brt_pt(events[0].0), "10/11 às 10h30");
        let before = |m: i64| events[0].0 - chrono::Duration::minutes(m);
        assert_eq!(event_alerts(before(60), &events).len(), 1);
        assert!(event_alerts(before(200), &events).is_empty());
        assert!(event_alerts(before(-5), &events).is_empty());
        assert!(
            event_alerts(before(60), &events)[0]
                .1
                .starts_with("Daqui a pouco: Inflação dos EUA (CPI) em 10/11 às 10h30")
        );
    }

    #[test]
    fn price_changes_and_domains_read_right() {
        let hourly: Vec<Bar> = (0..30)
            .map(|i| {
                hour_bar(
                    i,
                    100.0,
                    101.0,
                    99.0,
                    100.0 + f64::from(u32::try_from(i).unwrap()),
                    1.0,
                )
            })
            .collect();
        assert!((change_over(&hourly, 1).unwrap() - (129.0 / 128.0 - 1.0) * 100.0).abs() < 1e-9);
        assert!(change_over(&hourly[..10], 24).is_none());
        assert_eq!(
            domain_of("https://www.coindesk.com/markets/x"),
            "coindesk.com"
        );
        let map = BTreeMap::from([("SOLUSDT".to_owned(), hourly)]);
        let line = price_line(
            Coins {
                btc: false,
                eth: false,
                sol: true,
            },
            &map,
        );
        assert!(
            line.starts_with("\nPreço agora: SOL +0,8% na última hora"),
            "{line}"
        );
    }
}
