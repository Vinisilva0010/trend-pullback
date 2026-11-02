//! Download of Binance spot daily klines with verification and resume.
//!
//! Every file is verified against its CHECKSUM and fully parsed before it is
//! written, so files on disk are always valid. Files already on disk and valid
//! are skipped, which makes interrupted runs resumable.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::listing::{self, BUCKET_URL, ListingPage};
use super::{DecodeError, decode_monthly_file};
use crate::month::YearMonth;

const FILES_URL: &str = "https://data.binance.vision/data/spot/monthly/klines";
const KLINES_PREFIX: &str = "data/spot/monthly/klines/";
const MAX_LISTING_PAGES: usize = 50;
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// Errors from a single HTTP request.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("not found (HTTP 404)")]
    NotFound,
    #[error("HTTP status {0}")]
    Status(u16),
    #[error("response is larger than the size limit")]
    TooLarge,
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
}

/// Errors that stop a download step.
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error(transparent)]
    Fetch(#[from] FetchError),
    #[error(transparent)]
    Listing(#[from] listing::ListingError),
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error("file system error: {0}")]
    Io(#[from] std::io::Error),
    #[error("refusing unsafe name {0:?}")]
    UnsafeName(String),
    #[error("listing has too many pages")]
    TooManyPages,
    #[error("listing marker did not advance")]
    ListingStuck,
    #[error("response is not valid UTF-8")]
    Utf8,
    #[error("failed to build HTTP client: {0}")]
    Client(reqwest::Error),
}

/// Settings for one download run.
#[derive(Debug, Clone)]
pub struct DownloadConfig {
    /// Root data directory (files go under `raw/binance/spot/klines/1d`).
    pub data_dir: PathBuf,
    /// First month to download.
    pub from: YearMonth,
    /// Last month to download.
    pub to: YearMonth,
    /// Symbols to download; all USDT pairs when `None`.
    pub symbols: Option<Vec<String>>,
    /// Symbols processed in parallel.
    pub concurrency: usize,
    /// Attempts per request, including the first one.
    pub max_attempts: u32,
}

/// One file or symbol that could not be synced.
#[derive(Debug, Clone)]
pub struct FileFailure {
    pub symbol: String,
    /// The month, or `None` when the whole symbol failed (for example its listing).
    pub month: Option<YearMonth>,
    pub reason: String,
}

/// Result of syncing one symbol.
#[derive(Debug, Clone, Default)]
pub struct SymbolReport {
    pub symbol: String,
    /// Monthly files published by Binance inside the requested range.
    pub files_available: usize,
    pub downloaded: usize,
    pub already_valid: usize,
    pub failures: Vec<FileFailure>,
}

impl SymbolReport {
    fn failed(symbol: String, month: Option<YearMonth>, reason: String) -> Self {
        Self {
            failures: vec![FileFailure {
                symbol: symbol.clone(),
                month,
                reason,
            }],
            symbol,
            ..Self::default()
        }
    }
}

/// Totals for a whole run.
#[derive(Debug, Clone, Default)]
pub struct DownloadSummary {
    pub symbols: usize,
    pub files_available: usize,
    pub downloaded: usize,
    pub already_valid: usize,
    pub failures: Vec<FileFailure>,
}

impl DownloadSummary {
    fn absorb(&mut self, report: SymbolReport) {
        self.files_available += report.files_available;
        self.downloaded += report.downloaded;
        self.already_valid += report.already_valid;
        self.failures.extend(report.failures);
    }
}

/// Folder that holds the daily kline archives, one sub-folder per symbol.
#[must_use]
pub fn kline_dir(data_dir: &Path) -> PathBuf {
    data_dir
        .join("raw")
        .join("binance")
        .join("spot")
        .join("klines")
        .join("1d")
}

/// File name of a monthly daily-kline archive.
#[must_use]
pub fn zip_name(symbol: &str, month: YearMonth) -> String {
    format!("{symbol}-1d-{month}.zip")
}

/// Downloads and verifies daily klines for all requested symbols.
///
/// `on_symbol` is called once per finished symbol with (done, total, report).
///
/// # Errors
///
/// Returns [`DownloadError`] when the HTTP client cannot be built, a symbol in
/// the config is not a valid symbol, or the list of symbols cannot be read.
/// Failures of single files do not stop the run; they are collected in
/// [`DownloadSummary::failures`].
pub async fn download_spot_daily(
    config: &DownloadConfig,
    mut on_symbol: impl FnMut(usize, usize, &SymbolReport),
) -> Result<DownloadSummary, DownloadError> {
    let http = Arc::new(Http::new(config.max_attempts)?);
    let symbols = if let Some(list) = &config.symbols {
        validated_symbols(list)?
    } else {
        usdt_symbols(&http).await?
    };
    let total = symbols.len();
    let root = Arc::new(kline_dir(&config.data_dir));
    let semaphore = Arc::new(Semaphore::new(config.concurrency.max(1)));
    let mut tasks = JoinSet::new();
    for symbol in symbols {
        let http = Arc::clone(&http);
        let root = Arc::clone(&root);
        let semaphore = Arc::clone(&semaphore);
        let (from, to) = (config.from, config.to);
        tasks.spawn(async move {
            let Ok(_permit) = semaphore.acquire_owned().await else {
                return SymbolReport::failed(symbol, None, "download pool closed".to_owned());
            };
            sync_symbol(&http, &root, symbol, from, to).await
        });
    }
    let mut summary = DownloadSummary {
        symbols: total,
        ..DownloadSummary::default()
    };
    let mut done = 0;
    while let Some(joined) = tasks.join_next().await {
        done += 1;
        let report = joined.unwrap_or_else(|error| {
            SymbolReport::failed("?".to_owned(), None, format!("task failed: {error}"))
        });
        on_symbol(done, total, &report);
        summary.absorb(report);
    }
    summary
        .failures
        .sort_by(|a, b| (&a.symbol, a.month).cmp(&(&b.symbol, b.month)));
    Ok(summary)
}

fn validated_symbols(list: &[String]) -> Result<Vec<String>, DownloadError> {
    let mut symbols = Vec::with_capacity(list.len());
    for raw in list {
        let symbol = raw.trim().to_ascii_uppercase();
        if !listing::is_valid_symbol(&symbol) {
            return Err(DownloadError::UnsafeName(raw.clone()));
        }
        symbols.push(symbol);
    }
    symbols.sort_unstable();
    symbols.dedup();
    Ok(symbols)
}

async fn usdt_symbols(http: &Http) -> Result<Vec<String>, DownloadError> {
    let page = list_prefix(http, KLINES_PREFIX).await?;
    let mut symbols: Vec<String> = page
        .prefixes
        .iter()
        .map(String::as_str)
        .filter_map(listing::symbol_from_prefix)
        .filter(|symbol| listing::is_usdt_pair(symbol))
        .map(str::to_owned)
        .collect();
    symbols.sort_unstable();
    symbols.dedup();
    Ok(symbols)
}

async fn list_prefix(http: &Http, prefix: &str) -> Result<ListingPage, DownloadError> {
    let mut all = ListingPage::default();
    let mut marker = String::new();
    for _ in 0..MAX_LISTING_PAGES {
        for part in [prefix, marker.as_str()] {
            if !is_url_safe(part) {
                return Err(DownloadError::UnsafeName(part.to_owned()));
            }
        }
        let url = format!("{BUCKET_URL}?delimiter=/&prefix={prefix}&marker={marker}");
        let body = http.get_bytes(&url).await?;
        let xml = String::from_utf8(body).map_err(|_| DownloadError::Utf8)?;
        let page = listing::parse_listing_page(&xml)?;
        all.prefixes.extend(page.prefixes);
        all.keys.extend(page.keys);
        let Some(next) = page.next_marker else {
            return Ok(all);
        };
        if next == marker {
            return Err(DownloadError::ListingStuck);
        }
        marker = next;
    }
    Err(DownloadError::TooManyPages)
}

fn is_url_safe(text: &str) -> bool {
    text.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.'))
}

async fn sync_symbol(
    http: &Http,
    root: &Path,
    symbol: String,
    from: YearMonth,
    to: YearMonth,
) -> SymbolReport {
    let prefix = format!("{KLINES_PREFIX}{symbol}/1d/");
    let page = match list_prefix(http, &prefix).await {
        Ok(page) => page,
        Err(error) => return SymbolReport::failed(symbol, None, error.to_string()),
    };
    let mut months: Vec<YearMonth> = page
        .keys
        .iter()
        .filter_map(|key| listing::month_from_zip_key(&symbol, key))
        .filter(|month| (from..=to).contains(month))
        .collect();
    months.sort_unstable();
    months.dedup();
    let dir = root.join(&symbol);
    let mut report = SymbolReport {
        symbol,
        files_available: months.len(),
        ..SymbolReport::default()
    };
    for month in months {
        match sync_file(http, &dir, &report.symbol, month).await {
            Ok(Outcome::Downloaded) => report.downloaded += 1,
            Ok(Outcome::AlreadyValid) => report.already_valid += 1,
            Err(error) => report.failures.push(FileFailure {
                symbol: report.symbol.clone(),
                month: Some(month),
                reason: error.to_string(),
            }),
        }
    }
    report
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Downloaded,
    AlreadyValid,
}

async fn sync_file(
    http: &Http,
    dir: &Path,
    symbol: &str,
    month: YearMonth,
) -> Result<Outcome, DownloadError> {
    let name = zip_name(symbol, month);
    let zip_path = dir.join(&name);
    let checksum_path = dir.join(format!("{name}.CHECKSUM"));
    if local_file_is_valid(&zip_path, &checksum_path, &name).await {
        return Ok(Outcome::AlreadyValid);
    }
    let url = format!("{FILES_URL}/{symbol}/1d/{name}");
    let zip = http.get_bytes(&url).await?;
    let checksum_bytes = http.get_bytes(&format!("{url}.CHECKSUM")).await?;
    let checksum = String::from_utf8(checksum_bytes).map_err(|_| DownloadError::Utf8)?;
    decode_monthly_file(&name, &zip, &checksum)?;
    tokio::fs::create_dir_all(dir).await?;
    write_atomic(&zip_path, &zip).await?;
    write_atomic(&checksum_path, checksum.as_bytes()).await?;
    Ok(Outcome::Downloaded)
}

async fn local_file_is_valid(zip_path: &Path, checksum_path: &Path, name: &str) -> bool {
    let (Ok(zip), Ok(checksum)) = (
        tokio::fs::read(zip_path).await,
        tokio::fs::read_to_string(checksum_path).await,
    ) else {
        return false;
    };
    decode_monthly_file(name, &zip, &checksum).is_ok()
}

async fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".part");
    let temp = PathBuf::from(temp);
    tokio::fs::write(&temp, bytes).await?;
    tokio::fs::rename(&temp, path).await
}

#[derive(Debug)]
struct Http {
    client: reqwest::Client,
    max_attempts: u32,
}

impl Http {
    fn new(max_attempts: u32) -> Result<Self, DownloadError> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("trend-pullback/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(DownloadError::Client)?;
        Ok(Self {
            client,
            max_attempts: max_attempts.max(1),
        })
    }

    async fn get_bytes(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        let mut attempt = 1;
        loop {
            match self.try_get(url).await {
                Ok(bytes) => return Ok(bytes),
                Err(error) if attempt < self.max_attempts && is_retryable(&error) => {
                    tokio::time::sleep(backoff(attempt)).await;
                    attempt += 1;
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn try_get(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        let response = self.client.get(url).send().await?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(FetchError::NotFound);
        }
        if !status.is_success() {
            return Err(FetchError::Status(status.as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_FILE_BYTES)
        {
            return Err(FetchError::TooLarge);
        }
        let body = response.bytes().await?;
        if u64::try_from(body.len()).unwrap_or(u64::MAX) > MAX_FILE_BYTES {
            return Err(FetchError::TooLarge);
        }
        Ok(body.to_vec())
    }
}

fn is_retryable(error: &FetchError) -> bool {
    match error {
        FetchError::NotFound | FetchError::TooLarge => false,
        FetchError::Status(code) => *code == 429 || *code >= 500,
        FetchError::Network(_) => true,
    }
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(500_u64.saturating_mul(1_u64 << attempt.min(6)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_only_transient_errors() {
        assert!(!is_retryable(&FetchError::NotFound));
        assert!(!is_retryable(&FetchError::TooLarge));
        assert!(!is_retryable(&FetchError::Status(403)));
        assert!(is_retryable(&FetchError::Status(429)));
        assert!(is_retryable(&FetchError::Status(503)));
    }

    #[test]
    fn backoff_doubles_and_is_capped() {
        assert_eq!(backoff(1), Duration::from_secs(1));
        assert_eq!(backoff(2), Duration::from_secs(2));
        assert_eq!(backoff(4), Duration::from_secs(8));
        assert_eq!(backoff(20), Duration::from_secs(32));
    }

    #[test]
    fn validates_and_normalizes_symbols() {
        let list = vec![
            " btcusdt ".to_owned(),
            "BTCUSDT".to_owned(),
            "XMRUSDT".to_owned(),
        ];
        assert_eq!(
            validated_symbols(&list).unwrap(),
            vec!["BTCUSDT".to_owned(), "XMRUSDT".to_owned()]
        );
        assert!(matches!(
            validated_symbols(&["../etc".to_owned()]),
            Err(DownloadError::UnsafeName(_))
        ));
    }

    #[test]
    fn builds_paths_and_names() {
        let month = YearMonth::new(2024, 12).unwrap();
        assert_eq!(zip_name("BTCUSDT", month), "BTCUSDT-1d-2024-12.zip");
        assert_eq!(
            kline_dir(Path::new("data")),
            PathBuf::from("data/raw/binance/spot/klines/1d")
        );
    }

    #[test]
    fn checks_url_safety() {
        assert!(is_url_safe("data/spot/monthly/klines/BTCUSDT/1d/"));
        assert!(is_url_safe(""));
        assert!(!is_url_safe("a&b"));
        assert!(!is_url_safe("a b"));
        assert!(!is_url_safe("a?b"));
    }
}
