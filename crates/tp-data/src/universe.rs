//! Point-in-time trading universe: the most traded USDT pairs at the start of
//! each month, excluding stablecoins, fiat, wrapped duplicates, leveraged
//! tokens, tokenized stocks and assets whose price barely moves.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use chrono::{Datelike, Days, NaiveDate};
use tp_core::market::Candle;

use crate::binance::listing::is_valid_symbol;
use crate::history::{SeriesError, from_csv, io_err, write_atomic};
use crate::month::YearMonth;

/// Median absolute daily move below which a price counts as pegged.
const PEG_MAX_MEDIAN_MOVE: f64 = 0.002;
/// Minimum number of daily moves needed before judging a peg.
const PEG_MIN_MOVES: usize = 30;
/// Binance launched tokenized US stocks (bStocks, tickers ending in "B") on
/// this date. Pairs ending in "B" first traded on or after it are excluded.
const BSTOCKS_LAUNCH: (i32, u32, u32) = (2026, 6, 11);

/// Base assets excluded by name.
#[derive(Debug, Clone, Default)]
pub struct Exclusions {
    bases: BTreeSet<String>,
}

/// An entry in the exclusions file that is not a valid asset name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("exclusions file, line {line}: {value:?} is not a valid asset name")]
pub struct InvalidExclusion {
    pub line: usize,
    pub value: String,
}

impl Exclusions {
    /// Parses one base asset per line; `#` starts a comment.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidExclusion`] for an entry that is not uppercase letters
    /// and digits after normalization.
    pub fn parse(text: &str) -> Result<Self, InvalidExclusion> {
        let mut bases = BTreeSet::new();
        for (index, raw) in text.lines().enumerate() {
            let entry = raw.split('#').next().unwrap_or_default().trim();
            if entry.is_empty() {
                continue;
            }
            let base = entry.to_ascii_uppercase();
            if !is_valid_symbol(&base) {
                return Err(InvalidExclusion {
                    line: index + 1,
                    value: entry.to_owned(),
                });
            }
            bases.insert(base);
        }
        Ok(Self { bases })
    }

    #[must_use]
    pub fn contains(&self, base: &str) -> bool {
        self.bases.contains(base)
    }
}

/// Why a symbol is kept out of the universe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExclusionReason {
    Listed,
    LeveragedToken,
    TokenizedStock,
    Pegged,
}

impl ExclusionReason {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Listed => "listed",
            Self::LeveragedToken => "leveraged_token",
            Self::TokenizedStock => "tokenized_stock",
            Self::Pegged => "pegged_price",
        }
    }
}

/// Excluded symbols with the reason and, for pegged prices, the month detected.
pub type ExcludedMap = BTreeMap<String, (ExclusionReason, Option<YearMonth>)>;

/// One symbol selected for one month.
#[derive(Debug, Clone, PartialEq)]
pub struct UniverseEntry {
    pub month: YearMonth,
    pub rank: usize,
    pub symbol: String,
    /// USDT volume summed over the ranking window.
    pub quote_volume: f64,
    /// Days with data inside the ranking window.
    pub days: usize,
}

/// Parameters of the monthly selection.
#[derive(Debug, Clone, Copy)]
pub struct UniverseRules {
    pub size: usize,
    pub window_days: u64,
    pub min_days: usize,
    pub peg_window_days: u64,
}

impl Default for UniverseRules {
    fn default() -> Self {
        Self {
            size: 20,
            window_days: 30,
            min_days: 25,
            peg_window_days: 90,
        }
    }
}

/// Selected symbols per month and every excluded symbol.
#[derive(Debug, Clone, Default)]
pub struct UniverseResult {
    pub entries: Vec<UniverseEntry>,
    pub excluded: ExcludedMap,
}

/// Errors while building the universe.
#[derive(Debug, thiserror::Error)]
pub enum UniverseError {
    #[error(transparent)]
    Series(#[from] SeriesError),
    #[error(transparent)]
    Exclusions(#[from] InvalidExclusion),
    #[error("cannot read {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
}

fn is_tokenized_stock(base: &str, first_date: Option<NaiveDate>) -> bool {
    let (year, month, day) = BSTOCKS_LAUNCH;
    let launch = NaiveDate::from_ymd_opt(year, month, day);
    base.len() > 1
        && base.ends_with('B')
        && first_date
            .zip(launch)
            .is_some_and(|(first, launch)| first >= launch)
}

/// Rule-based exclusion that does not depend on the month: listed base
/// assets, tokenized stocks, and leveraged tokens whose underlying asset is
/// also traded against USDT. `first_date` is the first day the pair traded.
#[must_use]
pub fn static_exclusion(
    symbol: &str,
    exclusions: &Exclusions,
    symbols: &BTreeSet<String>,
    first_date: Option<NaiveDate>,
) -> Option<ExclusionReason> {
    let base = symbol.strip_suffix("USDT")?;
    if exclusions.contains(base) {
        return Some(ExclusionReason::Listed);
    }
    if is_tokenized_stock(base, first_date) {
        return Some(ExclusionReason::TokenizedStock);
    }
    for suffix in ["UP", "DOWN", "BULL", "BEAR"] {
        if let Some(underlying) = base.strip_suffix(suffix)
            && !underlying.is_empty()
            && symbols.contains(&format!("{underlying}USDT"))
        {
            return Some(ExclusionReason::LeveragedToken);
        }
    }
    None
}

/// True when the median absolute daily move of the closes is tiny, which is
/// how stablecoins and other pegged assets behave.
#[must_use]
pub fn looks_pegged(window: &[Candle]) -> bool {
    let mut moves: Vec<f64> = window
        .windows(2)
        .filter_map(|pair| {
            let [a, b] = pair else {
                return None;
            };
            Some((b.close() / a.close()).ln().abs())
        })
        .collect();
    if moves.len() < PEG_MIN_MOVES {
        return false;
    }
    moves.sort_by(f64::total_cmp);
    moves[moves.len() / 2] < PEG_MAX_MEDIAN_MOVE
}

fn slice_between(candles: &[Candle], from: NaiveDate, until: NaiveDate) -> &[Candle] {
    let start = candles.partition_point(|c| c.date() < from);
    let end = candles.partition_point(|c| c.date() < until);
    &candles[start..end.max(start)]
}

/// Months to rank: from the month after the first data point to the month
/// after the last one (the last month is the live universe).
#[must_use]
pub fn month_range(series: &BTreeMap<String, Vec<Candle>>) -> Vec<YearMonth> {
    let first = series
        .values()
        .filter_map(|candles| candles.first().map(Candle::date))
        .min();
    let last = series
        .values()
        .filter_map(|candles| candles.last().map(Candle::date))
        .max();
    let (Some(first), Some(last)) = (first, last) else {
        return Vec::new();
    };
    let (Ok(start), Ok(end)) = (
        YearMonth::new(first.year(), first.month()),
        YearMonth::new(last.year(), last.month()),
    ) else {
        return Vec::new();
    };
    YearMonth::range_inclusive(start.following(), end.following())
}

/// Ranks symbols by USDT volume before each month and keeps the top ones.
#[must_use]
pub fn build_universe(
    series: &BTreeMap<String, Vec<Candle>>,
    exclusions: &Exclusions,
    months: &[YearMonth],
    rules: UniverseRules,
) -> UniverseResult {
    let symbols: BTreeSet<String> = series.keys().cloned().collect();
    let mut result = UniverseResult::default();
    for (symbol, candles) in series {
        let first_date = candles.first().map(Candle::date);
        if let Some(reason) = static_exclusion(symbol, exclusions, &symbols, first_date) {
            result.excluded.insert(symbol.clone(), (reason, None));
        }
    }
    for &month in months {
        let Some(start) = NaiveDate::from_ymd_opt(month.year(), month.month(), 1) else {
            continue;
        };
        let (Some(volume_from), Some(peg_from), Some(previous_day)) = (
            start.checked_sub_days(Days::new(rules.window_days)),
            start.checked_sub_days(Days::new(rules.peg_window_days)),
            start.pred_opt(),
        ) else {
            continue;
        };
        let mut ranked: Vec<(f64, usize, &String)> = Vec::new();
        for (symbol, candles) in series {
            if result.excluded.contains_key(symbol) {
                continue;
            }
            let window = slice_between(candles, volume_from, start);
            let traded_yesterday = window.last().is_some_and(|c| c.date() == previous_day);
            if !traded_yesterday || window.len() < rules.min_days {
                continue;
            }
            if looks_pegged(slice_between(candles, peg_from, start)) {
                result
                    .excluded
                    .insert(symbol.clone(), (ExclusionReason::Pegged, Some(month)));
                continue;
            }
            let volume: f64 = window.iter().map(Candle::quote_volume).sum();
            ranked.push((volume, window.len(), symbol));
        }
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.2.cmp(b.2)));
        for (index, (volume, days, symbol)) in ranked.into_iter().take(rules.size).enumerate() {
            result.entries.push(UniverseEntry {
                month,
                rank: index + 1,
                symbol: symbol.clone(),
                quote_volume: volume,
                days,
            });
        }
    }
    result
}

/// Loads every processed daily CSV in `dir`, keyed by symbol.
///
/// # Errors
///
/// Returns [`SeriesError`] when the folder cannot be read or a file is invalid.
pub fn load_processed(dir: &Path) -> Result<BTreeMap<String, Vec<Candle>>, SeriesError> {
    let mut series = BTreeMap::new();
    for entry in fs::read_dir(dir).map_err(|e| io_err(dir, e))? {
        let path = entry.map_err(|e| io_err(dir, e))?.path();
        let is_csv = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("csv"));
        let Some(symbol) = path
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .map(str::to_owned)
        else {
            continue;
        };
        if !is_csv || !is_valid_symbol(&symbol) {
            continue;
        }
        let text = fs::read_to_string(&path).map_err(|e| io_err(&path, e))?;
        let candles = from_csv(&path.display().to_string(), &text)?;
        series.insert(symbol, candles);
    }
    Ok(series)
}

/// Universe as CSV.
#[must_use]
pub fn universe_csv(entries: &[UniverseEntry]) -> String {
    let mut out = String::from("month,rank,symbol,quote_volume_30d,days_in_window\n");
    for e in entries {
        let _ = writeln!(
            out,
            "{},{},{},{:.0},{}",
            e.month, e.rank, e.symbol, e.quote_volume, e.days
        );
    }
    out
}

/// Excluded symbols as CSV.
#[must_use]
pub fn excluded_csv(excluded: &ExcludedMap) -> String {
    let mut out = String::from("symbol,reason,detected_month\n");
    for (symbol, (reason, month)) in excluded {
        let month = month.map_or_else(String::new, |m| m.to_string());
        let _ = writeln!(out, "{symbol},{},{month}", reason.label());
    }
    out
}

/// Reads processed series and the exclusions file, builds the universe and
/// writes `universe.csv` and `universe_excluded.csv` to `out_dir`.
///
/// # Errors
///
/// Returns [`UniverseError`] when an input cannot be read or parsed, or an
/// output cannot be written.
pub fn build_and_write(
    processed: &Path,
    exclusions_path: &Path,
    out_dir: &Path,
    rules: UniverseRules,
) -> Result<UniverseResult, UniverseError> {
    let text = fs::read_to_string(exclusions_path).map_err(|source| UniverseError::Read {
        path: exclusions_path.display().to_string(),
        source,
    })?;
    let exclusions = Exclusions::parse(&text)?;
    let series = load_processed(processed)?;
    let months = month_range(&series);
    let result = build_universe(&series, &exclusions, &months, rules);
    fs::create_dir_all(out_dir).map_err(|e| io_err(out_dir, e))?;
    write_atomic(
        &out_dir.join("universe.csv"),
        universe_csv(&result.entries).as_bytes(),
    )?;
    write_atomic(
        &out_dir.join("universe_excluded.csv"),
        excluded_csv(&result.excluded).as_bytes(),
    )?;
    Ok(result)
}

/// Parses `universe.csv` into the ordered members of each month.
///
/// # Errors
///
/// Returns [`SeriesError::Processed`] on a wrong header or a malformed row.
pub fn parse_universe_csv(
    label: &str,
    text: &str,
) -> Result<BTreeMap<YearMonth, Vec<String>>, SeriesError> {
    let bad = |line: usize, reason: String| SeriesError::Processed {
        path: label.to_owned(),
        line,
        reason,
    };
    let mut lines = text.lines().enumerate();
    let header_ok = lines
        .next()
        .is_some_and(|(_, header)| header.starts_with("month,rank,symbol"));
    if !header_ok {
        return Err(bad(1, "missing or wrong header".to_owned()));
    }
    let mut out: BTreeMap<YearMonth, Vec<String>> = BTreeMap::new();
    for (index, raw) in lines {
        let row = raw.trim();
        if row.is_empty() {
            continue;
        }
        let mut fields = row.split(',');
        let (Some(month), Some(_rank), Some(symbol)) =
            (fields.next(), fields.next(), fields.next())
        else {
            return Err(bad(index + 1, "expected month,rank,symbol".to_owned()));
        };
        let month: YearMonth = month
            .parse()
            .map_err(|_| bad(index + 1, format!("invalid month {month:?}")))?;
        if !is_valid_symbol(symbol) {
            return Err(bad(index + 1, format!("invalid symbol {symbol:?}")));
        }
        out.entry(month).or_default().push(symbol.to_owned());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tp_core::market::CandleFields;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn candle(date: NaiveDate, close: f64, quote_volume: f64) -> Candle {
        Candle::try_new(CandleFields {
            date,
            open: close,
            high: close,
            low: close,
            close,
            volume: 1.0,
            quote_volume,
        })
        .unwrap()
    }

    /// Daily candles from `from` to `to` inclusive, alternating between two prices.
    fn daily(from: NaiveDate, to: NaiveDate, prices: (f64, f64), volume: f64) -> Vec<Candle> {
        let mut out = Vec::new();
        let mut date = from;
        let mut flip = false;
        while date <= to {
            out.push(candle(date, if flip { prices.1 } else { prices.0 }, volume));
            flip = !flip;
            date = date.succ_opt().unwrap();
        }
        out
    }

    fn symbols(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn parses_exclusions_file() {
        let exclusions = Exclusions::parse("# comment\nusdc\n\nFDUSD # inline\n").unwrap();
        assert!(exclusions.contains("USDC"));
        assert!(exclusions.contains("FDUSD"));
        assert!(!exclusions.contains("BTC"));
        assert_eq!(
            Exclusions::parse("BTC/USD\n").unwrap_err(),
            InvalidExclusion {
                line: 1,
                value: "BTC/USD".to_owned()
            }
        );
    }

    #[test]
    fn classifies_static_exclusions() {
        let exclusions = Exclusions::parse("USDC\n").unwrap();
        let all = symbols(&["BTCUSDT", "BTCUPUSDT", "JUPUSDT", "USDCUSDT", "ETHBEARUSDT"]);
        let old = Some(d(2020, 1, 1));
        assert_eq!(
            static_exclusion("USDCUSDT", &exclusions, &all, old),
            Some(ExclusionReason::Listed)
        );
        assert_eq!(
            static_exclusion("BTCUPUSDT", &exclusions, &all, old),
            Some(ExclusionReason::LeveragedToken)
        );
        assert_eq!(static_exclusion("JUPUSDT", &exclusions, &all, old), None);
        assert_eq!(
            static_exclusion("ETHBEARUSDT", &exclusions, &all, old),
            None
        );
        assert_eq!(static_exclusion("BTCUSDT", &exclusions, &all, old), None);
    }

    #[test]
    fn excludes_tokenized_stocks_by_suffix_and_listing_date() {
        let none = Exclusions::default();
        let all = BTreeSet::new();
        assert_eq!(
            static_exclusion("NVDABUSDT", &none, &all, Some(d(2026, 6, 11))),
            Some(ExclusionReason::TokenizedStock)
        );
        assert_eq!(
            static_exclusion("NVDABUSDT", &none, &all, Some(d(2026, 6, 10))),
            None
        );
        assert_eq!(
            static_exclusion("BNBUSDT", &none, &all, Some(d(2019, 1, 1))),
            None
        );
        assert_eq!(static_exclusion("NVDABUSDT", &none, &all, None), None);
    }

    #[test]
    fn detects_pegged_prices() {
        let stable = daily(d(2024, 1, 1), d(2024, 3, 1), (1.0, 1.0005), 1.0);
        let volatile = daily(d(2024, 1, 1), d(2024, 3, 1), (100.0, 103.0), 1.0);
        assert!(looks_pegged(&stable));
        assert!(!looks_pegged(&volatile));
        assert!(!looks_pegged(&stable[..10]));
    }

    #[test]
    fn ranks_by_volume_with_eligibility_rules() {
        let from = d(2024, 1, 1);
        let to = d(2024, 2, 29);
        let mut series = BTreeMap::new();
        series.insert("AAAUSDT".to_owned(), daily(from, to, (100.0, 103.0), 10.0));
        series.insert("BBBUSDT".to_owned(), daily(from, to, (100.0, 103.0), 20.0));
        series.insert(
            "CCCUSDT".to_owned(),
            daily(from, d(2024, 2, 27), (100.0, 103.0), 100.0),
        );
        series.insert(
            "DDDUSDT".to_owned(),
            daily(d(2024, 2, 20), to, (100.0, 103.0), 1_000.0),
        );
        series.insert("XYZUSDT".to_owned(), daily(from, to, (1.0, 1.0005), 500.0));
        let rules = UniverseRules {
            size: 2,
            ..UniverseRules::default()
        };
        let month = YearMonth::new(2024, 3).unwrap();
        let result = build_universe(&series, &Exclusions::default(), &[month], rules);
        let picked: Vec<&str> = result.entries.iter().map(|e| e.symbol.as_str()).collect();
        assert_eq!(picked, vec!["BBBUSDT", "AAAUSDT"]);
        assert_eq!(result.entries[0].rank, 1);
        assert_eq!(result.entries[0].days, 30);
        assert_eq!(
            result.excluded.get("XYZUSDT"),
            Some(&(ExclusionReason::Pegged, Some(month)))
        );
    }

    #[test]
    fn month_range_spans_after_first_to_after_last() {
        let mut series = BTreeMap::new();
        series.insert(
            "AAAUSDT".to_owned(),
            vec![
                candle(d(2019, 1, 1), 1.0, 1.0),
                candle(d(2019, 3, 31), 1.0, 1.0),
            ],
        );
        let months = month_range(&series);
        assert_eq!(months.first(), Some(&YearMonth::new(2019, 2).unwrap()));
        assert_eq!(months.last(), Some(&YearMonth::new(2019, 4).unwrap()));
    }
}
