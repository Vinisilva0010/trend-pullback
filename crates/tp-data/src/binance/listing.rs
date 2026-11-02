//! Parsing of the S3 bucket listing behind the Binance public data archive.

use crate::month::YearMonth;

/// Bucket endpoint that lists archive folders and files.
pub const BUCKET_URL: &str = "https://s3-ap-northeast-1.amazonaws.com/data.binance.vision";

/// One page of a bucket listing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListingPage {
    /// Sub-folders (common prefixes) on this page.
    pub prefixes: Vec<String>,
    /// File keys on this page.
    pub keys: Vec<String>,
    /// Marker for the next page, present only when the listing is truncated.
    pub next_marker: Option<String>,
}

/// Errors while parsing a listing page.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ListingError {
    #[error("response is not a bucket listing document")]
    NotListing,
    #[error("listing is truncated but gives no marker for the next page")]
    MissingMarker,
}

/// Parses one page of an S3 `ListBucketResult` document.
///
/// # Errors
///
/// Returns [`ListingError`] when the document is not a bucket listing, or when
/// it is truncated without any way to request the next page.
pub fn parse_listing_page(xml: &str) -> Result<ListingPage, ListingError> {
    if !xml.contains("<ListBucketResult") {
        return Err(ListingError::NotListing);
    }
    let prefixes: Vec<String> = xml
        .split("<CommonPrefixes>")
        .skip(1)
        .filter_map(|chunk| tag_text(chunk, "Prefix"))
        .map(unescape)
        .collect();
    let keys: Vec<String> = xml
        .split("<Contents>")
        .skip(1)
        .filter_map(|chunk| tag_text(chunk, "Key"))
        .map(unescape)
        .collect();
    let truncated = tag_text(xml, "IsTruncated").is_some_and(|value| value.trim() == "true");
    let next_marker = if truncated {
        let marker = tag_text(xml, "NextMarker")
            .map(unescape)
            .filter(|marker| !marker.is_empty())
            .or_else(|| keys.last().or(prefixes.last()).cloned())
            .ok_or(ListingError::MissingMarker)?;
        Some(marker)
    } else {
        None
    };
    Ok(ListingPage {
        prefixes,
        keys,
        next_marker,
    })
}

/// True for names made only of uppercase ASCII letters and digits, as used by
/// Binance symbols. Anything else is refused before it can reach a file path.
#[must_use]
pub fn is_valid_symbol(symbol: &str) -> bool {
    !symbol.is_empty()
        && symbol.len() <= 32
        && symbol
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// True for pairs quoted in USDT (for example `BTCUSDT`).
#[must_use]
pub fn is_usdt_pair(symbol: &str) -> bool {
    symbol.len() > 4 && symbol.ends_with("USDT")
}

/// Extracts a valid symbol from a folder prefix such as
/// `data/spot/monthly/klines/BTCUSDT/`.
#[must_use]
pub fn symbol_from_prefix(prefix: &str) -> Option<&str> {
    prefix
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|symbol| is_valid_symbol(symbol))
}

/// Extracts the month from a daily kline zip key of `symbol`, such as
/// `data/spot/monthly/klines/BTCUSDT/1d/BTCUSDT-1d-2024-12.zip`.
#[must_use]
pub fn month_from_zip_key(symbol: &str, key: &str) -> Option<YearMonth> {
    let file = key.rsplit('/').next()?;
    let month = file
        .strip_prefix(symbol)?
        .strip_prefix("-1d-")?
        .strip_suffix(".zip")?;
    month.parse().ok()
}

fn tag_text<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(&text[start..end])
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRUNCATED: &str = r#"<?xml version="1.0" encoding="UTF-8"?><ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>data.binance.vision</Name><Prefix>data/spot/monthly/klines/</Prefix><Marker></Marker><NextMarker>data/spot/monthly/klines/ADAUSDT/</NextMarker><MaxKeys>1000</MaxKeys><Delimiter>/</Delimiter><IsTruncated>true</IsTruncated><CommonPrefixes><Prefix>data/spot/monthly/klines/1INCHUSDT/</Prefix></CommonPrefixes><CommonPrefixes><Prefix>data/spot/monthly/klines/ADAUSDT/</Prefix></CommonPrefixes></ListBucketResult>"#;

    const KEYS: &str = "<ListBucketResult><Prefix>data/spot/monthly/klines/BTCUSDT/1d/</Prefix><IsTruncated>false</IsTruncated><Contents><Key>data/spot/monthly/klines/BTCUSDT/1d/BTCUSDT-1d-2024-12.zip</Key><Size>1</Size></Contents><Contents><Key>data/spot/monthly/klines/BTCUSDT/1d/BTCUSDT-1d-2024-12.zip.CHECKSUM</Key></Contents></ListBucketResult>";

    #[test]
    fn parses_truncated_prefix_page() {
        let page = parse_listing_page(TRUNCATED).unwrap();
        assert_eq!(
            page.prefixes,
            vec![
                "data/spot/monthly/klines/1INCHUSDT/".to_owned(),
                "data/spot/monthly/klines/ADAUSDT/".to_owned()
            ]
        );
        assert!(page.keys.is_empty());
        assert_eq!(
            page.next_marker.as_deref(),
            Some("data/spot/monthly/klines/ADAUSDT/")
        );
    }

    #[test]
    fn parses_final_key_page() {
        let page = parse_listing_page(KEYS).unwrap();
        assert_eq!(page.keys.len(), 2);
        assert!(page.prefixes.is_empty());
        assert_eq!(page.next_marker, None);
    }

    #[test]
    fn falls_back_to_last_key_when_marker_missing() {
        let xml = KEYS.replace("<IsTruncated>false", "<IsTruncated>true");
        let page = parse_listing_page(&xml).unwrap();
        assert_eq!(
            page.next_marker.as_deref(),
            Some("data/spot/monthly/klines/BTCUSDT/1d/BTCUSDT-1d-2024-12.zip.CHECKSUM")
        );
    }

    #[test]
    fn rejects_truncated_page_without_any_marker() {
        let xml = "<ListBucketResult><IsTruncated>true</IsTruncated><NextMarker></NextMarker></ListBucketResult>";
        assert_eq!(parse_listing_page(xml), Err(ListingError::MissingMarker));
    }

    #[test]
    fn rejects_other_documents() {
        assert_eq!(
            parse_listing_page("<Error><Code>AccessDenied</Code></Error>"),
            Err(ListingError::NotListing)
        );
    }

    #[test]
    fn unescapes_entities() {
        let xml = "<ListBucketResult><Contents><Key>a&amp;b</Key></Contents></ListBucketResult>";
        assert_eq!(
            parse_listing_page(xml).unwrap().keys,
            vec!["a&b".to_owned()]
        );
    }

    #[test]
    fn validates_symbols() {
        assert!(is_valid_symbol("BTCUSDT"));
        assert!(is_valid_symbol("1INCHUSDT"));
        for bad in ["", "btcusdt", "BTC/USDT", "..", "BTC-USDT", "BTC USDT"] {
            assert!(!is_valid_symbol(bad), "{bad}");
        }
    }

    #[test]
    fn detects_usdt_pairs() {
        assert!(is_usdt_pair("BTCUSDT"));
        assert!(!is_usdt_pair("USDT"));
        assert!(!is_usdt_pair("BTCBUSD"));
    }

    #[test]
    fn extracts_symbol_from_prefix() {
        assert_eq!(
            symbol_from_prefix("data/spot/monthly/klines/1INCHUSDT/"),
            Some("1INCHUSDT")
        );
        assert_eq!(symbol_from_prefix("data/spot/monthly/klines/../"), None);
        assert_eq!(symbol_from_prefix("data/spot/monthly/klines/btc/"), None);
    }

    #[test]
    fn extracts_month_from_zip_key() {
        let key = "data/spot/monthly/klines/BTCUSDT/1d/BTCUSDT-1d-2024-12.zip";
        assert_eq!(
            month_from_zip_key("BTCUSDT", key),
            Some(YearMonth::new(2024, 12).unwrap())
        );
        assert_eq!(
            month_from_zip_key("BTCUSDT", &format!("{key}.CHECKSUM")),
            None
        );
        assert_eq!(month_from_zip_key("ETHUSDT", key), None);
        assert_eq!(
            month_from_zip_key(
                "BTCUSDT",
                "data/spot/monthly/klines/BTCUSDT/1h/BTCUSDT-1h-2024-12.zip"
            ),
            None
        );
    }
}
