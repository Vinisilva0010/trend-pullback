//! Public news feeds (RSS 2.0 and Atom), read without any account or key.

use chrono::{DateTime, Utc};

use crate::binance::download::Http;

/// One feed item.
#[derive(Debug, Clone, PartialEq)]
pub struct NewsItem {
    pub title: String,
    pub link: String,
    /// Summary text with HTML removed (may be empty).
    pub summary: String,
    pub published: Option<DateTime<Utc>>,
}

/// Errors while fetching a feed.
#[derive(Debug, thiserror::Error)]
pub enum NewsError {
    #[error("{url}: {reason}")]
    Fetch { url: String, reason: String },
}

/// Text between `<tag ...>` and `</tag>` in `block` (first occurrence).
fn tag_text<'a>(block: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let mut from = 0;
    while let Some(rel) = block[from..].find(&open) {
        let start = from + rel;
        let after = block[start + open.len()..].chars().next()?;
        if after == '>' || after.is_whitespace() {
            let body_start = start + block[start..].find('>')? + 1;
            if block[start..body_start].ends_with("/>") {
                return Some("");
            }
            let close = format!("</{tag}>");
            let end = body_start + block[body_start..].find(&close)?;
            return Some(&block[body_start..end]);
        }
        from = start + open.len();
    }
    None
}

/// `href` attribute of the first `<link ...>` in an Atom entry.
fn atom_link(block: &str) -> Option<String> {
    let start = block.find("<link")?;
    let tag = &block[start..start + block[start..].find('>')?];
    let h = tag.find("href=")?;
    let rest = &tag[h + 5..];
    let quote = rest.chars().next()?;
    let rest = &rest[1..];
    Some(decode(&rest[..rest.find(quote)?]))
}

/// Removes CDATA wrappers and HTML tags, decodes common entities and
/// collapses whitespace.
#[must_use]
pub fn decode(raw: &str) -> String {
    let mut text = raw.trim().to_owned();
    if let Some(inner) = text
        .strip_prefix("<![CDATA[")
        .and_then(|t| t.strip_suffix("]]>"))
    {
        text = inner.to_owned();
    }
    // Entities first, so encoded tags (&lt;p&gt;) are removed below too.
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail.find(';').filter(|s| *s <= 10) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        if let Some(c) = decoded {
            out.push(c);
            rest = &tail[semi + 1..];
        } else {
            out.push('&');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    let mut plain = String::with_capacity(out.len());
    let mut in_tag = false;
    for c in out.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                plain.push(' ');
            }
            _ if !in_tag => plain.push(c),
            _ => {}
        }
    }
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_date(raw: &str) -> Option<DateTime<Utc>> {
    let t = decode(raw);
    DateTime::parse_from_rfc2822(&t)
        .or_else(|_| DateTime::parse_from_rfc3339(&t))
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Items of an RSS 2.0 or Atom feed. Items without a title or link are skipped.
#[must_use]
pub fn parse_feed(xml: &str) -> Vec<NewsItem> {
    let (open, close) = if xml.contains("<item") {
        ("<item", "</item>")
    } else {
        ("<entry", "</entry>")
    };
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(open) {
        let after = &rest[start..];
        let Some(end) = after.find(close) else {
            break;
        };
        let block = &after[..end];
        rest = &after[end + close.len()..];
        let title = tag_text(block, "title").map(decode).unwrap_or_default();
        let link = tag_text(block, "link")
            .map(decode)
            .filter(|l| !l.is_empty())
            .or_else(|| atom_link(block))
            .unwrap_or_default();
        if title.is_empty() || link.is_empty() {
            continue;
        }
        let summary = tag_text(block, "description")
            .or_else(|| tag_text(block, "summary"))
            .map(decode)
            .unwrap_or_default();
        let published = tag_text(block, "pubDate")
            .or_else(|| tag_text(block, "published"))
            .or_else(|| tag_text(block, "updated"))
            .and_then(parse_date);
        out.push(NewsItem {
            title,
            link,
            summary: summary.chars().take(400).collect(),
            published,
        });
    }
    out
}

/// Downloads and parses one feed.
///
/// # Errors
///
/// Returns [`NewsError`] when the request fails or the body is not UTF-8.
pub async fn fetch_feed(url: &str) -> Result<Vec<NewsItem>, NewsError> {
    let fail = |reason: String| NewsError::Fetch {
        url: url.to_owned(),
        reason,
    };
    let http = Http::new(2).map_err(|e| fail(e.to_string()))?;
    let body = http.get_bytes(url).await.map_err(|e| fail(e.to_string()))?;
    let text = String::from_utf8(body).map_err(|_| fail("not UTF-8".to_owned()))?;
    Ok(parse_feed(&text))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn reads_rss_items_with_cdata_and_entities() {
        let xml = r#"<?xml version="1.0"?><rss><channel><title>Feed</title>
<item><title><![CDATA[Solana &amp; friends: network outage]]></title>
<link>https://example.com/a?x=1&amp;y=2</link>
<description>&lt;p&gt;Validators &#8217;restart&#x2019; the chain&lt;/p&gt;</description>
<pubDate>Wed, 04 Nov 2026 18:30:00 +0000</pubDate></item>
<item><title>No link</title></item>
</channel></rss>"#;
        let items = parse_feed(xml);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Solana & friends: network outage");
        assert_eq!(items[0].link, "https://example.com/a?x=1&y=2");
        assert_eq!(
            items[0].summary,
            "Validators \u{2019}restart\u{2019} the chain"
        );
        assert_eq!(
            items[0].published.unwrap().to_rfc3339(),
            "2026-11-04T18:30:00+00:00"
        );
    }

    #[test]
    fn reads_atom_entries() {
        let xml = r#"<feed><title>Status</title><entry><title type="html">Degraded performance</title>
<link rel="alternate" href="https://status.example.com/i/1"/>
<updated>2026-11-04T18:30:00Z</updated><summary>Investigating</summary></entry></feed>"#;
        let items = parse_feed(xml);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].link, "https://status.example.com/i/1");
        assert_eq!(items[0].summary, "Investigating");
        assert!(items[0].published.is_some());
    }

    #[test]
    fn decode_keeps_text_safe() {
        assert_eq!(decode("A &unknown; B & C"), "A &unknown; B & C");
        assert_eq!(decode("<b>bold</b>   text"), "bold text");
    }
}
