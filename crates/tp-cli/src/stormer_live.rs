//! Live Stormer setups on BTC, ETH and SOL. After the comparison with
//! entering on any candle (same stop and target), no setup is proven on
//! these coins: the trap (failed breakdown or breakout, the liquidity sweep
//! with Stormer's entry) beat that base in both periods on the daily and
//! weekly charts, buys and sells, without enough trades yet. Those are "em
//! observação": Telegram, phone, dashboard and the paper book (daily ones).
//! The daily 1-2-3 buy with the trend did not beat the base and stays on the
//! dashboard only.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde_json::{Value, json};
use tp_core::intraday::Bar;
use tp_core::stormer::{Exit, Params, Pattern, Side, Signal, Trade, scan, weekly};

use super::{asset_name, count_f64, fmt_pt, money_pt, pct_pt, r_mean_t, r_pt};
use crate::smc_live::{Books, TradeRow, change_pct, position_line, ratio_pt, signed_money, when};

/// Paper book of the daily setups followed with alerts.
pub const BOOK: &str = "st1d";
const BOOK_LABEL: &str = "Stormer diário (trap)";
/// Sent stage keys, one per line.
pub const SENT_FILE: &str = "stormer_sent.txt";
const DAY_MS: i64 = 24 * 3600 * 1000;
const WEEK_MS: i64 = 7 * DAY_MS;

/// What the tool does with a pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Passed the test and beat the base: alerts and paper book.
    Candidate,
    /// Above the base in both periods without proof: alerts and paper book.
    Watch,
    /// Did not beat the base: dashboard only.
    Info,
}

/// One chart scanned for one asset.
pub struct Chart {
    pub tf: &'static str,
    pub step_ms: i64,
    pub bars: Vec<Bar>,
    pub trades: Vec<Trade>,
    pub signals: Vec<Signal>,
}

impl Chart {
    fn close_ms(&self, i: usize) -> i64 {
        self.bars
            .get(i)
            .map_or(0, |b| b.open_time_ms + self.step_ms)
    }
}

/// Daily and weekly charts with every trade and the open signals.
#[must_use]
pub fn scan_charts(daily: &[Bar]) -> Vec<Chart> {
    let week = weekly(daily);
    let (dt, ds) = scan(daily, Params::with_max_hold(30));
    let (wt, ws) = scan(&week, Params::with_max_hold(12));
    vec![
        Chart {
            tf: "1d",
            step_ms: DAY_MS,
            bars: daily.to_vec(),
            trades: dt,
            signals: ds,
        },
        Chart {
            tf: "1w",
            step_ms: WEEK_MS,
            bars: week,
            trades: wt,
            signals: ws,
        },
    ]
}

/// The role of a pattern on a chart, if the tool follows it at all.
#[must_use]
pub fn role(tf: &str, pattern: Pattern, side: Side, with_trend: bool) -> Option<Role> {
    match (tf, pattern, side) {
        ("1d" | "1w", Pattern::Trap, _) => Some(Role::Watch),
        ("1d", Pattern::OneTwoThreeInside, Side::Short) if with_trend => Some(Role::Watch),
        ("1d", Pattern::OneTwoThree, Side::Long) if with_trend => Some(Role::Info),
        _ => None,
    }
}

impl Role {
    /// Alerts on Telegram and phone (and, on the daily chart, the paper book).
    #[must_use]
    pub fn alerts(self) -> bool {
        self != Self::Info
    }

    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Candidate => "candidato",
            Self::Watch => "observacao",
            Self::Info => "acompanhar",
        }
    }

    fn line(self) -> &'static str {
        match self {
            Self::Candidate => {
                "CANDIDATO: passou no teste e ganhou de entrar em qualquer dia com o mesmo stop."
            }
            Self::Watch => {
                "EM OBSERVAÇÃO: ficou acima da base (entrar em qualquer dia com o mesmo stop) nos dois períodos, mas ainda sem trades suficientes para provar."
            }
            Self::Info => {
                "SÓ PARA ACOMPANHAR: no histórico não ganhou de entrar em qualquer dia com o mesmo stop."
            }
        }
    }
}

fn pattern_word(p: Pattern) -> &'static str {
    match p {
        Pattern::OneTwoThree => "123",
        Pattern::OneTwoThreeInside => "123 (3º dentro do 2º)",
        Pattern::Trap => "trap",
        Pattern::InsideBar => "inside bar",
        Pattern::ClosedCandle => "candle fechado",
        Pattern::GapTrap => "gap",
    }
}

fn pattern_code(p: Pattern) -> &'static str {
    match p {
        Pattern::OneTwoThree => "123",
        Pattern::OneTwoThreeInside => "123i",
        Pattern::Trap => "trap",
        Pattern::InsideBar => "ib",
        Pattern::ClosedCandle => "cf",
        Pattern::GapTrap => "gap",
    }
}

fn side_code(s: Side) -> &'static str {
    match s {
        Side::Long => "long",
        Side::Short => "short",
    }
}

fn side_word(s: Side) -> &'static str {
    match s {
        Side::Long => "COMPRA",
        Side::Short => "VENDA (short)",
    }
}

fn chart_word(tf: &str) -> &'static str {
    if tf == "1w" { "semanal" } else { "diário" }
}

fn key(asset: &str, c: &Chart, pattern: Pattern, side: Side, signal_index: usize) -> String {
    format!(
        "{asset}|{}|{}|{}|{}",
        c.tf,
        pattern_code(pattern),
        side_code(side),
        c.bars.get(signal_index).map_or(0, |b| b.open_time_ms)
    )
}

/// History of this pattern and side on this chart of this asset.
fn history_line(asset: &str, c: &Chart, pattern: Pattern, side: Side, trend_only: bool) -> String {
    let same = |t: &&Trade| {
        t.how != Exit::Open
            && t.pattern == pattern
            && t.side == side
            && (!trend_only || t.with_trend)
    };
    let rs: Vec<f64> = c.trades.iter().filter(same).map(|t| t.r).collect();
    if rs.is_empty() {
        return format!(
            "Histórico em {}: nenhum trade deste setup.",
            asset_name(asset)
        );
    }
    let hits = c
        .trades
        .iter()
        .filter(same)
        .filter(|t| t.how == Exit::Target)
        .count();
    let (mean, _) = r_mean_t(&rs);
    format!(
        "Histórico em {} neste gráfico: {} trades, {}% no alvo, média {} por trade.",
        asset_name(asset),
        rs.len(),
        ratio_pt(count_f64(hits) / count_f64(rs.len()) * 100.0),
        r_pt(mean)
    )
}

/// A message to send (or show on the dashboard only), with its key.
pub struct StageAlert {
    pub key: String,
    pub message: String,
    pub notify: bool,
}

/// New stages for one asset, and the closed trades for the paper book
/// (which `books` is updated with).
#[allow(clippy::too_many_lines)] // one pass over signals, entries, exits and expiries
pub fn stage_alerts(
    asset: &str,
    charts: &[Chart],
    now_ms: i64,
    sent: &BTreeSet<String>,
    books: &mut Books,
) -> (Vec<StageAlert>, Vec<TradeRow>) {
    let mut out = Vec::new();
    let mut rows = Vec::new();
    let name = asset_name(asset);
    for c in charts {
        let recent = |i: usize| c.close_ms(i) >= now_ms - 2 * c.step_ms - 4 * 3600 * 1000;
        for s in &c.signals {
            let Some(r) = role(c.tf, s.pattern, s.side, s.with_trend) else {
                continue;
            };
            let base = key(asset, c, s.pattern, s.side, s.signal_index);
            let k = format!("{base}|sinal");
            if sent.contains(&k) {
                continue;
            }
            let alerts = r.alerts();
            let deadline = c.close_ms(s.signal_index) + c.step_ms;
            let (verb, beyond) = match s.side {
                Side::Long => ("comprar", "passar de"),
                Side::Short => ("vender", "cair abaixo de"),
            };
            let mut text = format!(
                "{name} {}: SINAL DE {} ({}, Stormer){}.",
                chart_word(c.tf),
                side_word(s.side),
                pattern_word(s.pattern),
                if s.with_trend {
                    ", a favor da tendência"
                } else {
                    ""
                }
            );
            let _ = write!(text, "\n{}", r.line());
            let _ = write!(
                text,
                "\nPlano: {verb} se o preço {beyond} {} até {}. Se não acontecer, o sinal vence.\nStop {} ({}), alvo {} ({}), 2 vezes o risco.",
                fmt_pt(s.trigger),
                when(deadline),
                fmt_pt(s.stop),
                pct_pt(change_pct(s.trigger, s.stop)),
                fmt_pt(s.target),
                pct_pt(change_pct(s.trigger, s.target)),
            );
            let trend_only =
                s.pattern == Pattern::OneTwoThreeInside || s.pattern == Pattern::OneTwoThree;
            let _ = write!(
                text,
                "\n{}",
                history_line(asset, c, s.pattern, s.side, trend_only)
            );
            if alerts && c.tf == "1d" {
                let _ = write!(
                    text,
                    "\n{}",
                    position_line(BOOK_LABEL, books_equity(books), s.trigger, s.stop)
                );
            }
            out.push(StageAlert {
                key: k,
                message: text,
                notify: alerts,
            });
        }
        let start = books.starts.get(BOOK).copied().unwrap_or(i64::MAX);
        let mut closed: Vec<&Trade> = Vec::new();
        for t in &c.trades {
            let Some(r) = role(c.tf, t.pattern, t.side, t.with_trend) else {
                continue;
            };
            let alerts = r.alerts();
            let base = key(asset, c, t.pattern, t.side, t.signal_index);
            let in_book = alerts && c.tf == "1d" && c.close_ms(t.entry_index) > start;
            let k_in = format!("{base}|entrou");
            if !sent.contains(&k_in) && (t.how == Exit::Open || recent(t.entry_index)) {
                let mut text = format!(
                    "{name} {}: ENTROU {} ({}) a {} (candle que fechou em {}).\nStop {} ({}), alvo {} ({}). Se nenhum bater, fecha pelo prazo.",
                    chart_word(c.tf),
                    if t.side == Side::Long {
                        "na compra"
                    } else {
                        "na venda (short)"
                    },
                    pattern_word(t.pattern),
                    fmt_pt(t.entry),
                    when(c.close_ms(t.entry_index)),
                    fmt_pt(t.stop),
                    pct_pt(change_pct(t.entry, t.stop)),
                    fmt_pt(t.target),
                    pct_pt(change_pct(t.entry, t.target)),
                );
                if in_book {
                    let _ = write!(
                        text,
                        "\n{}",
                        position_line(BOOK_LABEL, books_equity(books), t.entry, t.stop)
                    );
                }
                out.push(StageAlert {
                    key: k_in,
                    message: text,
                    notify: alerts,
                });
            }
            if t.how == Exit::Open {
                continue;
            }
            if in_book && !books.has_trade_key(&base) {
                closed.push(t);
                continue;
            }
            let k_out = format!("{base}|saiu");
            if !sent.contains(&k_out) && recent(t.exit_index) {
                out.push(StageAlert {
                    key: k_out,
                    message: closed_text(name, c, t, None),
                    notify: alerts,
                });
            }
        }
        closed.sort_by_key(|t| t.exit_index);
        for t in closed {
            let base = key(asset, c, t.pattern, t.side, t.signal_index);
            let before = books_equity(books);
            let row = TradeRow {
                key: base.clone(),
                tf: BOOK.to_owned(),
                asset: asset.to_owned(),
                side: side_code(t.side).to_owned(),
                fill_ms: c.close_ms(t.entry_index),
                exit_ms: c.close_ms(t.exit_index),
                entry: t.entry,
                stop: t.stop,
                exit: t.exit,
                r: t.r,
                status: match t.how {
                    Exit::Target => "target",
                    Exit::Stop => "stop",
                    _ => "time",
                }
                .to_owned(),
            };
            books.trades.push(row.clone());
            rows.push(row);
            let after = books_equity(books);
            let k_out = format!("{base}|saiu");
            if !sent.contains(&k_out) {
                out.push(StageAlert {
                    key: k_out,
                    message: closed_text(name, c, t, Some((before, after))),
                    notify: true,
                });
            }
        }
        // Signals sent earlier that the next candle did not trigger.
        let head = format!("{asset}|{}|", c.tf);
        let waiting: BTreeSet<String> = c
            .signals
            .iter()
            .map(|s| key(asset, c, s.pattern, s.side, s.signal_index))
            .collect();
        let triggered: BTreeSet<String> = c
            .trades
            .iter()
            .map(|t| key(asset, c, t.pattern, t.side, t.signal_index))
            .collect();
        for k in sent.range(head.clone()..) {
            if !k.starts_with(&head) {
                break;
            }
            let Some(base) = k.strip_suffix("|sinal") else {
                continue;
            };
            if waiting.contains(base)
                || triggered.contains(base)
                || sent.contains(&format!("{base}|venceu"))
            {
                continue;
            }
            // Only the dashboard-only 1-2-3 buy expires silently.
            let alerts = !base.contains("|123|long|");
            out.push(StageAlert {
                key: format!("{base}|venceu"),
                message: format!(
                    "{name} {}: o sinal de {} não foi acionado (o preço não rompeu o gatilho no candle seguinte). Nada a fazer.",
                    chart_word(c.tf),
                    if base.contains("|long|") { "compra" } else { "venda" }
                ),
                notify: alerts,
            });
        }
    }
    (out, rows)
}

fn books_equity(books: &Books) -> f64 {
    books.equity_of(BOOK)
}

fn closed_text(name: &str, c: &Chart, t: &Trade, book: Option<(f64, f64)>) -> String {
    let how = match t.how {
        Exit::Target => "no alvo",
        Exit::Stop => "no stop",
        _ => "pelo prazo",
    };
    let dir = if t.side == Side::Long { 1.0 } else { -1.0 };
    let mut text = format!(
        "{name} {}: SAIU {how} a {} (candle que fechou em {}). Entrada foi {}. Resultado do trade: {} ({}, já com custos).",
        chart_word(c.tf),
        fmt_pt(t.exit),
        when(c.close_ms(t.exit_index)),
        fmt_pt(t.entry),
        pct_pt(dir * change_pct(t.entry, t.exit)),
        r_pt(t.r)
    );
    if let Some((before, after)) = book {
        let _ = write!(
            text,
            "\nConta fictícia {BOOK_LABEL}: {} no trade, saldo agora {}.",
            signed_money(after - before),
            money_pt(after)
        );
    }
    text
}

/// Dashboard view of one asset: open signals and the last trades of each
/// followed pattern.
#[must_use]
pub fn dashboard_json(asset: &str, charts: &[Chart]) -> Value {
    items_json(asset, charts, &role)
}

/// Like [`dashboard_json`] with the roles decided by `role_of`.
#[must_use]
pub fn items_json(
    asset: &str,
    charts: &[Chart],
    role_of: &dyn Fn(&str, Pattern, Side, bool) -> Option<Role>,
) -> Value {
    let mut items = Vec::new();
    for c in charts {
        for s in &c.signals {
            let Some(r) = role_of(c.tf, s.pattern, s.side, s.with_trend) else {
                continue;
            };
            items.push(json!({
                "key": key(asset, c, s.pattern, s.side, s.signal_index),
                "tf": c.tf,
                "pattern": pattern_word(s.pattern),
                "side": side_code(s.side),
                "role": r.code(),
                "status": "sinal",
                "trigger": s.trigger,
                "stop": s.stop,
                "target": s.target,
                "signal_ms": c.bars.get(s.signal_index).map_or(0, |b| b.open_time_ms),
                "deadline_ms": c.close_ms(s.signal_index) + c.step_ms,
                "entry_ms": null, "exit_ms": null, "exit": null, "r": null,
            }));
        }
        let followed: Vec<&Trade> = c
            .trades
            .iter()
            .filter(|t| role_of(c.tf, t.pattern, t.side, t.with_trend).is_some())
            .collect();
        let skip = followed.len().saturating_sub(10);
        for t in &followed[skip..] {
            let r = role_of(c.tf, t.pattern, t.side, t.with_trend);
            items.push(json!({
                "key": key(asset, c, t.pattern, t.side, t.signal_index),
                "tf": c.tf,
                "pattern": pattern_word(t.pattern),
                "side": side_code(t.side),
                "role": r.map_or("acompanhar", Role::code),
                "status": match t.how { Exit::Open => "aberto", Exit::Target => "alvo", Exit::Stop => "stop", Exit::Time => "prazo" },
                "trigger": t.entry,
                "stop": t.stop,
                "target": t.target,
                "signal_ms": c.bars.get(t.signal_index).map_or(0, |b| b.open_time_ms),
                "deadline_ms": null,
                "entry_ms": c.close_ms(t.entry_index),
                "exit_ms": if t.how == Exit::Open { None } else { Some(c.close_ms(t.exit_index)) },
                "exit": if t.how == Exit::Open { None } else { Some(t.exit) },
                "r": if t.how == Exit::Open { None } else { Some(t.r) },
            }));
        }
    }
    Value::Array(items)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn bar(i: usize, open: f64, high: f64, low: f64, close: f64) -> Bar {
        Bar {
            open_time_ms: i64::try_from(i).unwrap() * DAY_MS,
            open,
            high,
            low,
            close,
            volume: 1.0,
        }
    }

    /// 120 rising days (trend up), then a 1-2-3 whose trigger is 140.6.
    fn story() -> Vec<Bar> {
        let mut bars: Vec<Bar> = (0..120)
            .map(|i| {
                let p = 20.0 + f64::from(u32::try_from(i).unwrap());
                bar(i, p, p + 0.8, p - 0.6, p + 0.5)
            })
            .collect();
        bars.push(bar(120, 140.4, 140.8, 139.6, 140.0)); // 1
        bars.push(bar(121, 140.0, 140.2, 139.0, 139.8)); // 2: lowest low
        bars.push(bar(122, 139.8, 140.6, 139.5, 140.5)); // 3: signal
        bars
    }

    /// Flat days with a swing low of 95 on day 20, then day 30 trades down
    /// to 94 and closes back above 95: a trap buy with trigger 101.
    fn trap_story() -> Vec<Bar> {
        let mut bars: Vec<Bar> = (0..30)
            .map(|i| bar(i, 100.0, 101.0, if i == 20 { 95.0 } else { 98.0 }, 100.0))
            .collect();
        bars.push(bar(30, 99.0, 101.0, 94.0, 99.5));
        bars
    }

    #[test]
    fn a_daily_trap_goes_from_signal_to_entry_to_target_and_into_the_book() {
        let mut bars = trap_story();
        let mut books = Books::default();
        books.starts.insert(BOOK.to_owned(), 0);
        let now = bars.last().unwrap().open_time_ms + DAY_MS + 60_000;
        let (alerts, rows) = stage_alerts(
            "BTCUSDT",
            &scan_charts(&bars),
            now,
            &BTreeSet::new(),
            &mut books,
        );
        let signal = alerts
            .iter()
            .find(|a| a.key == "BTCUSDT|1d|trap|long|2592000000|sinal")
            .unwrap();
        assert!(signal.notify);
        assert!(
            signal.message.contains("EM OBSERVAÇÃO"),
            "{}",
            signal.message
        );
        assert!(rows.is_empty());
        let sent: BTreeSet<String> = alerts.iter().map(|a| a.key.clone()).collect();
        // Fills at 101 (risk 7), then the target 115 is hit.
        bars.push(bar(31, 100.5, 102.0, 99.0, 101.5));
        bars.push(bar(32, 101.5, 116.0, 100.0, 115.5));
        let now = bars.last().unwrap().open_time_ms + DAY_MS + 60_000;
        let (alerts, rows) = stage_alerts("BTCUSDT", &scan_charts(&bars), now, &sent, &mut books);
        assert!(
            alerts
                .iter()
                .any(|a| a.key.ends_with("trap|long|2592000000|entrou"))
        );
        let out = alerts
            .iter()
            .find(|a| a.key.ends_with("trap|long|2592000000|saiu"))
            .unwrap();
        assert!(out.message.contains("SAIU no alvo"), "{}", out.message);
        assert!(out.message.contains("Conta fictícia"), "{}", out.message);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tf, BOOK);
        assert!(books.equity_of(BOOK) > 2200.0);
    }

    #[test]
    fn an_untriggered_signal_expires_once() {
        let mut bars = story();
        let mut books = Books::default();
        let now = bars.last().unwrap().open_time_ms + DAY_MS + 60_000;
        let (alerts, _) = stage_alerts(
            "BTCUSDT",
            &scan_charts(&bars),
            now,
            &BTreeSet::new(),
            &mut books,
        );
        let sent: BTreeSet<String> = alerts.iter().map(|a| a.key.clone()).collect();
        bars.push(bar(123, 140.5, 140.55, 139.9, 140.1)); // never above 140.6
        let now = now + DAY_MS;
        let (alerts, _) = stage_alerts("BTCUSDT", &scan_charts(&bars), now, &sent, &mut books);
        assert!(
            alerts
                .iter()
                .any(|a| a.key.ends_with("|123|long|10540800000|venceu")),
            "{:?}",
            alerts.iter().map(|a| &a.key).collect::<Vec<_>>()
        );
    }

    #[test]
    fn only_the_tested_patterns_are_followed() {
        assert_eq!(
            role("1d", Pattern::OneTwoThree, Side::Long, true),
            Some(Role::Info)
        );
        assert_eq!(
            role("1d", Pattern::Trap, Side::Short, false),
            Some(Role::Watch)
        );
        assert_eq!(role("1d", Pattern::OneTwoThree, Side::Long, false), None);
        assert_eq!(role("1d", Pattern::OneTwoThree, Side::Short, true), None);
        assert_eq!(
            role("1w", Pattern::Trap, Side::Short, false),
            Some(Role::Watch)
        );
        assert_eq!(role("1d", Pattern::InsideBar, Side::Long, true), None);
    }
}
