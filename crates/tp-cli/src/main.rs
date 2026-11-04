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
use tp_backtest::range::{RangeCosts, RangeParams, RangeResult, run_range};
use tp_backtest::{
    BacktestConfig, Criteria, Evaluation, Market, equity_csv, evaluate, run, trades_csv,
};
use tp_core::indicators::{close_values, segments};
use tp_core::intraday::aggregate;
use tp_core::liquidity::{LiquidityResult, mean_and_t, study_symbol};
use tp_core::liquidity::{odds_after_break, round_above, round_below, simple_atr};
use tp_core::market::Candle;
use tp_core::odds::{Forward, Shape, forward_stats, touch_odds, weekly, wick_shares};
use tp_core::regime::{Regime, average_run, classify};
use tp_core::strategy::{EntryMode, Frame, StrategyParams, market_ok, setups_on};
use tp_core::trend::{DONCHIAN_LOOKBACKS, donchian_positions};
use tp_core::trend::{VolTarget, donchian_ensemble_weights, sma_trend_weights};
use tp_data::binance::download::{DownloadConfig, DownloadSummary, download_spot_daily, kline_dir};
use tp_data::binance::rest::{fetch_daily_since, fetch_recent_hourly};
use tp_data::funding::parse_funding_csv;
use tp_data::history::{BuildSummary, build_all, processed_dir, read_series};
use tp_data::intraday::parse_bars_csv;
use tp_data::live::{State, extend_series, parse_env, parse_state, state_csv};
use tp_data::month::YearMonth;
use tp_data::notify::send_telegram;
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
        /// Kept so the scheduled script keeps working; the report no longer
        /// sizes orders.
        #[arg(long, default_value_t = 1000.0)]
        capital: f64,
        /// Send a Telegram message when an order is needed (reads .env).
        #[arg(long)]
        notify: bool,
        /// Show the signal without saving state or log and without sending.
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
            capital: _,
            notify,
            dry_run,
        } => run_daily(&cli.data_dir, notify, dry_run).await,
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

async fn run_daily(data_dir: &Path, notify: bool, dry_run: bool) -> CliResult {
    let result = cmd_daily(data_dir, notify, dry_run).await;
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

async fn cmd_daily(data_dir: &Path, notify: bool, dry_run: bool) -> CliResult {
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
        lines.extend(daily_report(asset, &candles, &closes));
        sections.push(lines.join("\n"));
        // The tested model weight is still tracked for the record and the
        // paper account; it is no longer part of the message.
        let (held_date, held) = state.get(asset).copied().unwrap_or((NaiveDate::MIN, 0.0));
        if held_date >= signal_date {
            continue;
        }
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
    let header = "Relatório diário (candle fechado às 21h de Brasília). Chance = quantas vezes aconteceu no histórico da moeda; não é certeza.".to_owned();
    let lines = sections;
    println!("{header}");
    for line in &lines {
        println!("{line}");
    }
    if dry_run {
        println!("dry run: nothing saved, nothing sent");
        return Ok(ExitCode::SUCCESS);
    }
    std::fs::create_dir_all(&live_dir)?;
    std::fs::write(&state_path, state_csv(&state))?;
    let log_path = append_signal_log(&live_dir, &log)?;
    println!("saved: {} and {}", state_path.display(), log_path.display());
    if notify {
        let env = parse_env(&std::fs::read_to_string(".env")?);
        let (Some(token), Some(chat)) =
            (env.get("TELEGRAM_BOT_TOKEN"), env.get("TELEGRAM_CHAT_ID"))
        else {
            return Err("TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID must be set in .env".into());
        };
        // One message per asset keeps each under Telegram's 4096-character limit.
        for (i, section) in lines.iter().enumerate() {
            let text = if i == 0 {
                format!("{header}\n\n{section}")
            } else {
                section.clone()
            };
            send_telegram(token, chat, &text).await?;
        }
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

/// Horizons, in days, of the touch chances in the daily report.
const REPORT_HORIZONS: [usize; 3] = [1, 3, 7];
/// Past days in today's trend state needed before the touch chances use only them.
const MIN_SAME_TREND_DAYS: usize = 200;
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

fn forward_line(what: &str, horizon: &str, f: Option<Forward>) -> String {
    let Some(f) = f else {
        return format!("  Histórico de {what}: poucos casos para medir");
    };
    let verdict = if !f.is_edge() {
        "Sem vantagem: igual a um dia qualquer."
    } else if f.mean_atr > 0.0 {
        "Vantagem de alta comprovada (mais que um dia qualquer)."
    } else {
        "Vantagem de queda comprovada (pior que um dia qualquer)."
    };
    format!(
        "  Histórico de {what} ({} casos): {horizon} depois, subiu em {:.0}% das vezes (em qualquer dia: {:.0}%). {verdict}",
        f.cases,
        f.up_rate * 100.0,
        f.base_up_rate * 100.0
    )
}

/// Highest high and lowest low of the `days` candles that end at `last` (inclusive).
fn extremes_until(candles: &[Candle], last: usize, days: usize) -> (f64, f64) {
    let w = &candles[last + 1 - days.min(last + 1)..=last];
    (
        w.iter().map(Candle::high).fold(f64::MIN, f64::max),
        w.iter().map(Candle::low).fold(f64::MAX, f64::min),
    )
}

/// Signal lines from the 20-day channel: confirmed breakout, failed breakout,
/// close below the low, or nothing; always ends with what to do.
fn signal_lines(candles: &[Candle], atr: &[Option<f64>], last: usize) -> Vec<String> {
    let prior = |t: usize| extremes_until(candles, t - 1, 20);
    let mask = |test: &dyn Fn(usize) -> bool| -> Vec<bool> {
        (0..candles.len()).map(|t| t > 20 && test(t)).collect()
    };
    let broke_up = |t: usize| candles[t].close() > prior(t).0;
    let failed_up = |t: usize| candles[t].high() > prior(t).0 && candles[t].close() <= prior(t).0;
    let broke_down = |t: usize| candles[t].close() < prior(t).1;
    let (high20, low20) = prior(last);
    let (next_high, _) = extremes_until(candles, last, 20);
    let today = &candles[last];
    let mut out = Vec::new();
    if broke_up(last) {
        out.push(format!(
            "Sinal: fechou acima da máxima de 20 dias ({}).",
            fmt_pt(high20)
        ));
        out.push(forward_line(
            "fechamentos acima da máxima de 20 dias",
            "7 dias",
            forward_stats(candles, atr, &mask(&broke_up), CANDLE_HORIZON),
        ));
        out.push("O que fazer: sinal de compra confirmado no fechamento.".to_owned());
    } else if failed_up(last) {
        out.push(format!(
            "Rompeu a máxima de 20 dias ({}) durante o dia, mas fechou abaixo: rompimento não confirmado.",
            fmt_pt(high20)
        ));
        out.push(forward_line(
            "rompimentos que não fecharam acima",
            "7 dias",
            forward_stats(candles, atr, &mask(&failed_up), CANDLE_HORIZON),
        ));
        out.push(format!(
            "O que fazer: esperar. Compra só se um dia fechar acima de {}.",
            fmt_pt(next_high)
        ));
    } else if broke_down(last) {
        out.push(format!(
            "Fechou abaixo da mínima de 20 dias ({}).",
            fmt_pt(low20)
        ));
        out.push(forward_line(
            "fechamentos abaixo da mínima de 20 dias",
            "7 dias",
            forward_stats(candles, atr, &mask(&broke_down), CANDLE_HORIZON),
        ));
        out.push(format!(
            "O que fazer: sem sinal de compra. Compra só se um dia fechar acima de {}.",
            fmt_pt(next_high)
        ));
    } else {
        out.push(format!(
            "O que fazer: sem sinal. Compra só se um dia fechar acima de {} (máxima de 20 dias, {} do fechamento).",
            fmt_pt(next_high),
            pct_pt((next_high / today.close() - 1.0) * 100.0)
        ));
    }
    out
}

/// Touch chances for the two nearest levels above and below the close.
fn touch_lines(
    candles: &[Candle],
    atr: &[Option<f64>],
    regimes: &[Option<Regime>],
    last: usize,
    a: f64,
) -> Vec<String> {
    let close = candles[last].close();
    let regime = regimes[last];
    let same: Vec<bool> = regimes
        .iter()
        .map(|r| r.is_some() && *r == regime)
        .collect();
    let usable = same.iter().filter(|s| **s).count();
    let (mask, basis) = if regime.is_some() && usable >= MIN_SAME_TREND_DAYS {
        (
            same,
            format!("contando só dias de tendência de {}", trend_pt(regime)),
        )
    } else {
        (
            vec![true; candles.len()],
            "contando todos os dias".to_owned(),
        )
    };
    let (high20, low20) = extremes_until(candles, last, 20);
    let (high60, low60) = extremes_until(candles, last, 60);
    let mut above = vec![
        ("máxima de 20 dias", high20),
        ("máxima de 60 dias", high60),
        ("número redondo", round_above(close)),
    ];
    let mut below = vec![
        ("mínima de 20 dias", low20),
        ("mínima de 60 dias", low60),
        ("número redondo", round_below(close)),
    ];
    above.retain(|(_, p)| *p > close);
    below.retain(|(_, p)| *p < close && *p > 0.0);
    above.sort_by(|x, y| x.1.total_cmp(&y.1));
    below.sort_by(|x, y| y.1.total_cmp(&x.1));
    above.dedup_by(|x, y| (x.1 - y.1).abs() <= 1e-9 * y.1.abs());
    below.dedup_by(|x, y| (x.1 - y.1).abs() <= 1e-9 * y.1.abs());
    let mut out = vec![format!("Chance de tocar em 1 / 3 / 7 dias ({basis}):")];
    for (up, levels) in [(true, &above), (false, &below)] {
        for (label, price) in levels.iter().take(2) {
            let distance = (price - close).abs() / a;
            let chances: Vec<String> = REPORT_HORIZONS
                .iter()
                .map(|&h| {
                    touch_odds(candles, atr, &mask, h, distance, up)
                        .map_or_else(|| "-".to_owned(), |(p, _)| format!("{:.0}%", p * 100.0))
                })
                .collect();
            out.push(format!(
                "  {} {label} {} ({}): {}",
                if up { "acima:" } else { "abaixo:" },
                fmt_pt(*price),
                pct_pt((price / close - 1.0) * 100.0),
                chances.join(" / ")
            ));
        }
    }
    out
}

/// Weekly candle reading, shown when the day that closed is a Sunday.
fn weekly_lines(candles: &[Candle], last: usize) -> Vec<String> {
    let date = candles[last].date();
    if date.weekday() != chrono::Weekday::Sun {
        return Vec::new();
    }
    let weeks = weekly(&candles[..=last]);
    let Some(week) = weeks.last().filter(|w| w.date() == date) else {
        return Vec::new();
    };
    let watr = simple_atr(&weeks, 14);
    let shape = Shape::of(week);
    let (upper, lower) = wick_shares(week);
    let same: Vec<bool> = weeks.iter().map(|w| Shape::of(w) == shape).collect();
    let change = (week.close() / week.open() - 1.0) * 100.0;
    vec![
        format!(
            "Candle da semana: {} ({}), pavio em cima {:.0}% e embaixo {:.0}% da semana",
            shape.label_pt(),
            pct_pt(change),
            upper * 100.0,
            lower * 100.0
        ),
        forward_line(
            "semanas assim",
            "2 semanas",
            forward_stats(&weeks, &watr, &same, WEEK_HORIZON),
        ),
    ]
}

fn daily_report(asset: &str, candles: &[Candle], closes: &[f64]) -> Vec<String> {
    let mut out = Vec::new();
    let Some(last) = candles.len().checked_sub(1).filter(|l| *l > 60) else {
        return out;
    };
    let atr = simple_atr(candles, 14);
    let Some(a) = atr[last].filter(|a| *a > 0.0) else {
        return out;
    };
    let today = &candles[last];
    let close = today.close();
    let regimes = classify(closes);
    let votes = DONCHIAN_LOOKBACKS
        .iter()
        .filter(|&&l| donchian_positions(closes, l)[last])
        .count();
    out.push(format!(
        "{} — fechou {} em {} ({} no dia)",
        asset_name(asset),
        today.date().format("%d/%m"),
        fmt_pt(close),
        pct_pt((close / candles[last - 1].close() - 1.0) * 100.0)
    ));
    out.push(format!(
        "Tendência: {} ({votes} de 9 canais para cima)",
        trend_pt(regimes[last])
    ));
    let shape = Shape::of(today);
    let (upper, lower) = wick_shares(today);
    out.push(format!(
        "Candle do dia: {}, pavio em cima {:.0}% e embaixo {:.0}% do dia, tamanho {} ATR",
        shape.label_pt(),
        upper * 100.0,
        lower * 100.0,
        format!("{:.1}", (today.high() - today.low()) / a).replace('.', ",")
    ));
    let same_shape: Vec<bool> = candles.iter().map(|c| Shape::of(c) == shape).collect();
    out.push(forward_line(
        "candles assim",
        "7 dias",
        forward_stats(candles, &atr, &same_shape, CANDLE_HORIZON),
    ));
    out.extend(weekly_lines(candles, last));
    out.extend(touch_lines(candles, &atr, &regimes, last, a));
    out.extend(signal_lines(candles, &atr, last));
    out
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

#[allow(clippy::too_many_lines)] // one pass per asset: data, levels, targets, message
async fn alert_for(
    data_dir: &Path,
    asset: &str,
    now_ms: i64,
) -> Result<Option<(String, String)>, Box<dyn Error>> {
    let mut daily = read_series(&processed_dir(data_dir).join(format!("{asset}.csv")))?;
    let last = daily.last().map(Candle::date).ok_or("empty history")?;
    let since = last.succ_opt().ok_or("date overflow")?;
    let recent = fetch_daily_since(asset, since, now_ms).await?;
    extend_series(&mut daily, &recent)?;
    let hourly = fetch_recent_hourly(asset, 3, now_ms).await?;
    let n = daily.len();
    let Some(price) = hourly.last().map(|b| b.close) else {
        return Ok(None);
    };
    if n < 120 {
        return Ok(None);
    }
    let atr = simple_atr(&daily, 14);
    let Some(a) = atr[n - 1].filter(|a| *a > 0.0) else {
        return Ok(None);
    };
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
        return Ok(None);
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
    let (_, cases) = odds_after_break(&daily, &atr, up, 7, 0.0).unwrap_or((0.0, 0));
    let mut msg = format!(
        "{asset} rompeu a {name} ({}). Último fechamento de 1 hora: {}.\nPróximos pontos de liquidez {} e quantas vezes o preço chegou lá em até 7 dias depois de romper a {} de 20 dias ({cases} casos no histórico):",
        fmt_price(level),
        fmt_price(price),
        if up { "acima" } else { "abaixo" },
        if up { "máxima" } else { "mínima" }
    );
    for (label, target) in targets.iter().take(4) {
        let distance = (target - price).abs() / a;
        let (p, _) = odds_after_break(&daily, &atr, up, 7, distance).unwrap_or((0.0, 0));
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
    Ok(Some((name.to_owned(), msg)))
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
    for asset in ["BTCUSDT", "ETHUSDT", "SOLUSDT"] {
        let Some((name, msg)) = alert_for(data_dir, asset, now_ms).await? else {
            continue;
        };
        let key = format!("{today},{asset},{name}");
        if sent.contains(&key) {
            println!("{asset}: {name} já avisado hoje");
            continue;
        }
        println!("{msg}");
        messages.push(msg);
        let _ = writeln!(keys, "{key}");
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
#[allow(clippy::unwrap_used)]
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
        let report = daily_report("SOLUSDT", &candles, &closes);
        println!("{}", report.join("\n"));
        assert!(report[0].starts_with("SOL — fechou "));
        assert!(report[1].starts_with("Tendência: "));
        assert!(report.iter().any(|l| l.starts_with("Candle do dia: ")));
        assert!(report.iter().any(|l| l.starts_with("Chance de tocar")));
        assert!(
            report.last().unwrap().contains("O que fazer")
                || report.iter().any(|l| l.starts_with("O que fazer"))
        );
    }

    #[test]
    fn daily_report_needs_enough_history() {
        let candles = walk(30);
        let closes = close_values(&candles);
        assert!(daily_report("BTCUSDT", &candles, &closes).is_empty());
    }
}
