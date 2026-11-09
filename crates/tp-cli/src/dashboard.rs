//! Dashboard state: everything the web app shows, written to `data/live`
//! as JSON and published to the web app when `.env` has `DASHBOARD_URL` and
//! `DASHBOARD_TOKEN`.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};
use tp_core::smc::{Setup, Side, Status, nearest_liquidity};
use tp_data::dashboard::publish;
use tp_data::live::parse_env;

use crate::smc_live::{BOOK_CAPITAL, BOOK_FRAMES, Books, FrameScan, is_candidate};

/// Events kept for the dashboard feed.
pub const EVENT_MEMORY: usize = 1000;
/// Events sent with each live snapshot.
const EVENTS_IN_SNAPSHOT: usize = 300;
/// Past setups per timeframe shown on the charts.
const SETUPS_PER_FRAME: usize = 40;
pub const EVENTS_FILE: &str = "events.jsonl";
pub const LIVE_FILE: &str = "dashboard_live.json";
pub const DAILY_FILE: &str = "dashboard_daily.json";

/// One alert as the dashboard shows it.
#[derive(Debug, Clone)]
pub struct Event {
    pub id: String,
    pub ms: i64,
    /// Asset name (`BTC`) or `mercado`.
    pub asset: String,
    /// `smc`, `nivel`, `anomalia`, `noticia`, `mercado`.
    pub kind: &'static str,
    pub text: String,
    /// Also sent to Telegram and as a phone notification.
    pub notify: bool,
}

impl Event {
    #[must_use]
    pub fn to_json(&self) -> Value {
        let title = self.text.lines().next().unwrap_or("").to_owned();
        json!({
            "id": self.id,
            "ms": self.ms,
            "asset": self.asset,
            "kind": self.kind,
            "title": title,
            "text": self.text,
            "notify": self.notify,
        })
    }
}

/// Appends events to the feed file, keeping the last [`EVENT_MEMORY`].
///
/// # Errors
///
/// Returns an I/O error when the file cannot be written.
pub fn append_events(live_dir: &Path, events: &[Event]) -> std::io::Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    let path = live_dir.join(EVENTS_FILE);
    let mut lines: Vec<String> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    lines.extend(events.iter().map(|e| e.to_json().to_string()));
    let keep = lines.len().saturating_sub(EVENT_MEMORY);
    std::fs::write(path, lines[keep..].join("\n") + "\n")
}

/// The most recent events of the feed file, newest first.
#[must_use]
pub fn recent_events(live_dir: &Path, limit: usize) -> Vec<Value> {
    let text = std::fs::read_to_string(live_dir.join(EVENTS_FILE)).unwrap_or_default();
    let mut out: Vec<Value> = text
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str(l).ok())
        .take(limit)
        .collect();
    out.shrink_to_fit();
    out
}

fn side_code(side: Side) -> &'static str {
    match side {
        Side::Long => "long",
        Side::Short => "short",
    }
}

fn status_code(status: Status) -> &'static str {
    match status {
        Status::Waiting => "armado",
        Status::Open => "aberto",
        Status::Target => "alvo",
        Status::Stop => "stop",
        Status::Time => "prazo",
        Status::Cancelled => "cancelado",
    }
}

fn opt_ms(f: &FrameScan, i: Option<usize>) -> Value {
    i.map_or(Value::Null, |i| json!(f.close_ms(i)))
}

fn setup_json(asset: &str, f: &FrameScan, s: &Setup) -> Value {
    json!({
        "key": format!("{asset}|{}|{}|{}", f.tf(), side_code(s.side), f.open_ms(s.sweep_index)),
        "side": side_code(s.side),
        "status": status_code(s.status),
        "candidate": is_candidate(f.hours, s.wyckoff_range),
        "wyckoff": s.wyckoff_range,
        "climax": s.climax_volume,
        "entry": s.entry,
        "stop": s.stop,
        "target": s.target,
        "rr": s.reward_to_risk(),
        "swept_level": s.swept_level,
        "sweep_extreme": s.sweep_extreme,
        "choch_level": s.choch_level,
        "sweep_ms": f.open_ms(s.sweep_index),
        "choch_ms": f.close_ms(s.choch_index),
        "valid_until_ms": f.close_ms(s.choch_index + f.params.order_valid),
        "fill_ms": opt_ms(f, s.fill_index),
        "exit_ms": opt_ms(f, s.exit_index),
        "exit_price": s.exit_price,
        "r": s.r,
    })
}

fn frame_json(asset: &str, f: &FrameScan) -> Value {
    let (above, below) = nearest_liquidity(&f.bars, f.params.wing);
    let start = f.setups.len().saturating_sub(SETUPS_PER_FRAME);
    let setups: Vec<Value> = f.setups[start..]
        .iter()
        .map(|s| setup_json(asset, f, s))
        .collect();
    let pending: Vec<Value> = f
        .pending
        .iter()
        .map(|p| {
            json!({
                "key": format!("{asset}|{}|{}|{}", f.tf(), side_code(p.side), f.open_ms(p.sweep_index)),
                "side": side_code(p.side),
                "candidate": is_candidate(f.hours, p.wyckoff_range),
                "wyckoff": p.wyckoff_range,
                "climax": p.climax_volume,
                "swept_level": p.swept_level,
                "sweep_extreme": p.sweep_extreme,
                "choch_level": p.choch_level,
                "sweep_ms": f.open_ms(p.sweep_index),
                "deadline_ms": f.close_ms(p.deadline_index),
            })
        })
        .collect();
    json!({
        "tf": f.tf(),
        "label": f.label(),
        "liquidity": { "above": above, "below": below },
        "setups": setups,
        "pending": pending,
    })
}

/// Live state for the dashboard: assets, setups, books and recent events.
#[must_use]
pub fn live_json(
    now_ms: i64,
    assets: &[(String, Vec<FrameScan>)],
    stormer: &BTreeMap<String, Value>,
    books: &Books,
    events: &[Value],
) -> Value {
    let assets: Vec<Value> = assets
        .iter()
        .map(|(asset, frames)| {
            let hourly = frames.iter().find(|f| f.hours == 1);
            let price = hourly.and_then(|f| f.bars.last()).map(|b| b.close);
            let day_ago =
                hourly.and_then(|f| f.bars.len().checked_sub(25).map(|i| f.bars[i].close));
            let change = match (price, day_ago) {
                (Some(p), Some(d)) if d > 0.0 => Some((p / d - 1.0) * 100.0),
                _ => None,
            };
            json!({
                "symbol": asset,
                "name": asset.strip_suffix("USDT").unwrap_or(asset),
                "price": price,
                "change_24h": change,
                "frames": frames.iter().map(|f| frame_json(asset, f)).collect::<Vec<_>>(),
                "stormer": stormer.get(asset).cloned().unwrap_or_else(|| json!([])),
            })
        })
        .collect();
    let books: Vec<Value> = BOOK_FRAMES
        .iter()
        .map(|(tf, label)| {
            let mut equity = BOOK_CAPITAL;
            let trades: Vec<Value> = books
                .trades
                .iter()
                .filter(|t| t.tf == *tf)
                .map(|t| {
                    equity *= t.growth();
                    json!({
                        "key": t.key, "asset": t.asset, "side": t.side,
                        "fill_ms": t.fill_ms, "exit_ms": t.exit_ms,
                        "entry": t.entry, "stop": t.stop, "exit": t.exit,
                        "r": t.r, "status": t.status, "equity_after": equity,
                    })
                })
                .collect();
            json!({
                "tf": tf,
                "label": label,
                "start_ms": books.starts.get(*tf),
                "capital": BOOK_CAPITAL,
                "equity": equity,
                "trades": trades,
            })
        })
        .collect();
    json!({
        "version": 1,
        "generated_ms": now_ms,
        "assets": assets,
        "books": books,
        "events": events,
    })
}

/// Live snapshot with the last events from the feed file.
#[must_use]
pub fn live_snapshot(
    live_dir: &Path,
    now_ms: i64,
    assets: &[(String, Vec<FrameScan>)],
    stormer: &BTreeMap<String, Value>,
    books: &Books,
) -> Value {
    live_json(
        now_ms,
        assets,
        stormer,
        books,
        &recent_events(live_dir, EVENTS_IN_SNAPSHOT),
    )
}

/// Writes `data` to `file` in `live_dir` and publishes it when the
/// dashboard is configured in `.env`. A failed publish is reported and
/// does not fail the run: the next run sends the full state again.
///
/// # Errors
///
/// Returns an I/O error when the local file cannot be written.
pub async fn save_and_publish(
    live_dir: &Path,
    file: &str,
    kind: &str,
    data: &Value,
    new_events: &[Event],
) -> std::io::Result<()> {
    std::fs::create_dir_all(live_dir)?;
    std::fs::write(live_dir.join(file), data.to_string())?;
    let env = parse_env(&std::fs::read_to_string(".env").unwrap_or_default());
    let (Some(url), Some(token)) = (env.get("DASHBOARD_URL"), env.get("DASHBOARD_TOKEN")) else {
        println!("dashboard: not configured in .env, saved locally only");
        return Ok(());
    };
    let body = json!({
        "kind": kind,
        "data": data,
        "new_events": new_events.iter().map(Event::to_json).collect::<Vec<_>>(),
    })
    .to_string();
    match publish(url, token, body).await {
        Ok(()) => println!("dashboard: published {kind}"),
        Err(e) => eprintln!("warning: dashboard: {e}"),
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn events_keep_only_the_last_ones_and_come_back_newest_first() {
        let dir = std::env::temp_dir().join(format!("tp-events-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(dir.join(EVENTS_FILE));
        let make = |i: i64| Event {
            id: format!("e{i}"),
            ms: i,
            asset: "BTC".to_owned(),
            kind: "smc",
            text: format!("título {i}\ncorpo"),
            notify: i % 2 == 0,
        };
        let events: Vec<Event> = (0..5).map(make).collect();
        append_events(&dir, &events).unwrap();
        let back = recent_events(&dir, 2);
        assert_eq!(back.len(), 2);
        assert_eq!(back[0]["id"], "e4");
        assert_eq!(back[0]["title"], "título 4");
        assert_eq!(back[1]["notify"], false);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
