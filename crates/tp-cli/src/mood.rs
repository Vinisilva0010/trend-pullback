//! The news side of the market thermometer: every headline of the last 24
//! hours, for crypto and for US stocks, read for which way it pushes prices.
//! A keyword reading, so the dashboard shows it next to the price trend and
//! never alone.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use tp_core::news::{Kind, Lean, classify_headline, classify_stock_headline, lean_of, level_of};
use tp_data::news::NewsItem;

/// Hours of news the thermometer reads.
pub const MOOD_HOURS: i64 = 24;
/// Headlines sent per market, the most important first.
const SHOWN: usize = 12;

/// Which list a feed belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Market {
    Crypto,
    Stocks,
}

impl Market {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Crypto => "cripto",
            Self::Stocks => "acoes",
        }
    }

    /// Kind of a headline for this market, or None when it is not news
    /// for it.
    #[must_use]
    pub fn classify(self, item: &NewsItem) -> Option<Kind> {
        match self {
            Self::Crypto => classify_headline(&item.title, &item.summary).map(|c| c.kind),
            Self::Stocks => classify_stock_headline(&item.title, &item.summary),
        }
    }
}

/// Host of a link without `www.`.
#[must_use]
pub fn source_of(link: &str) -> &str {
    let rest = link
        .strip_prefix("https://")
        .or_else(|| link.strip_prefix("http://"))
        .unwrap_or(link);
    let host = rest.split('/').next().unwrap_or(rest);
    host.strip_prefix("www.").unwrap_or(host)
}

struct Read<'a> {
    item: &'a NewsItem,
    kind: Kind,
    lean: Lean,
    ms: i64,
}

fn weight(level: u8) -> f64 {
    f64::from(level)
}

/// The thermometer reading of one market.
#[must_use]
pub fn market_mood(market: Market, items: &[NewsItem], now: DateTime<Utc>) -> Value {
    let mut seen_links = BTreeSet::new();
    let mut seen_titles = BTreeSet::new();
    let mut reads: Vec<Read> = Vec::new();
    for item in items {
        let Some(published) = item.published else {
            continue;
        };
        if published > now || now - published > chrono::Duration::hours(MOOD_HOURS) {
            continue;
        }
        let title_key = item.title.trim().to_lowercase();
        if !seen_links.insert(item.link.as_str()) || !seen_titles.insert(title_key) {
            continue;
        }
        let Some(kind) = market.classify(item) else {
            continue;
        };
        reads.push(Read {
            item,
            kind,
            lean: lean_of(kind, &item.title, &item.summary),
            ms: published.timestamp_millis(),
        });
    }
    // Sources that carried the same kind of news with the same lean.
    let mut sources: BTreeMap<(Kind, &str), BTreeSet<&str>> = BTreeMap::new();
    for r in &reads {
        sources
            .entry((r.kind, r.lean.code()))
            .or_default()
            .insert(source_of(&r.item.link));
    }
    let level = |r: &Read| level_of(r.kind, sources[&(r.kind, r.lean.code())].len());
    let (mut up, mut down, mut unclear) = (0.0, 0.0, 0.0);
    let (mut n_up, mut n_down, mut n_unclear) = (0_usize, 0_usize, 0_usize);
    for r in &reads {
        let w = weight(level(r));
        match r.lean {
            Lean::Up => {
                up += w;
                n_up += 1;
            }
            Lean::Down => {
                down += w;
                n_down += 1;
            }
            Lean::Unclear => {
                unclear += w;
                n_unclear += 1;
            }
        }
    }
    let total = up + down + unclear;
    let score = if total > 0.0 {
        (up - down) / total
    } else {
        0.0
    };
    let mut ranked: Vec<&Read> = reads.iter().collect();
    ranked.sort_by(|a, b| level(b).cmp(&level(a)).then(b.ms.cmp(&a.ms)));
    let shown: Vec<Value> = ranked
        .iter()
        .take(SHOWN)
        .map(|r| {
            json!({
                "title": r.item.title.chars().take(300).collect::<String>(),
                "link": r.item.link,
                "source": source_of(&r.item.link),
                "ms": r.ms,
                "kind": r.kind.label_pt(),
                "lean": r.lean.code(),
                "level": level(r),
            })
        })
        .collect();
    json!({
        "hours": MOOD_HOURS,
        "up": n_up,
        "down": n_down,
        "unclear": n_unclear,
        "weight": total,
        "score": score,
        "items": shown,
    })
}

/// Both markets, for the live snapshot.
#[must_use]
pub fn mood_json(crypto: &[NewsItem], stocks: &[NewsItem], now: DateTime<Utc>) -> Value {
    json!({
        "cripto": market_mood(Market::Crypto, crypto, now),
        "acoes": market_mood(Market::Stocks, stocks, now),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    fn item(title: &str, link: &str, hours_ago: i64, now: DateTime<Utc>) -> NewsItem {
        NewsItem {
            title: title.to_owned(),
            link: link.to_owned(),
            summary: String::new(),
            published: Some(now - chrono::Duration::hours(hours_ago)),
        }
    }

    #[test]
    fn weighs_the_lean_of_the_last_day() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let items = vec![
            item(
                "Fed signals rate cut as inflation cools",
                "https://a.com/1",
                2,
                now,
            ),
            item("Powell hints at rate cut", "https://www.b.com/2", 3, now),
            item("Stocks slide as tariffs return", "https://c.com/3", 5, now),
            // Same title again, and one too old: both left out.
            item("Powell hints at rate cut", "https://d.com/4", 1, now),
            item("Fed hikes rates", "https://e.com/5", 30, now),
        ];
        let m = market_mood(Market::Stocks, &items, now);
        assert_eq!(m["up"], 2);
        assert_eq!(m["down"], 1);
        // Two sources on the rate cut: level 3 each; the tariff one is level 2.
        assert_eq!(m["items"][0]["level"], 3);
        let score = m["score"].as_f64().unwrap();
        assert!((score - (6.0 - 2.0) / 8.0).abs() < 1e-9, "{score}");
        assert_eq!(m["items"][2]["source"], "c.com");
    }

    #[test]
    fn no_news_reads_as_zero() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let m = market_mood(Market::Crypto, &[], now);
        assert_eq!(m["score"], 0.0);
        assert_eq!(m["items"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn source_is_the_host() {
        assert_eq!(source_of("https://www.cnbc.com/2026/x.html"), "cnbc.com");
        assert_eq!(source_of("http://a.b.com"), "a.b.com");
    }
}
