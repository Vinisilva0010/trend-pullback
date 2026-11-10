//! Intraday watch of the US stock trades, run by the hourly alerts during
//! the New York session. Nomad (and many brokers for Brazilians) take only
//! market and limit orders, so the trap's entry (a buy stop at the trigger)
//! and its exits (stop and target) cannot be left as orders: these alerts
//! tell when the price passes the trigger, hits the stop or reaches the
//! target, with the number of whole shares to buy.
//!
//! `tp markets` writes the list of signals and open trades to
//! [`WATCH_FILE`] after each close; the paper book still records entries
//! and exits from closed candles only.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use serde_json::{Value, json};
use tp_data::yahoo::{Intraday, fetch_intraday};

use super::{fmt_pt, money_pt, pct_pt};
use crate::smc_live::{StageAlert, change_pct, ratio_pt, when};
use crate::stock_trades::POSITION_CAP;

/// Signals and open trades followed during the session.
pub const WATCH_FILE: &str = "stock_watch.json";
/// Share of the account risked per trade.
const RISK: f64 = 0.02;
const DAY_MS: i64 = 24 * 3600 * 1000;
const HOUR_MS: i64 = 3600 * 1000;
/// Below this reward to risk, buying late is outside what the test covered.
const LATE_RR: f64 = 1.5;

/// One signal or open trade.
#[derive(Debug, Clone, PartialEq)]
pub struct Watched {
    /// Stage key of the setup (asset, chart, pattern, side, signal candle).
    pub key: String,
    pub symbol: String,
    pub name: String,
    /// `1d` or `1w`.
    pub tf: String,
    /// `sinal` (waiting for the trigger) or `aberto` (entered).
    pub status: String,
    pub trigger: f64,
    pub stop: f64,
    pub target: f64,
    /// Watch from this time on: the signal's close, or the entry's close.
    pub from_ms: i64,
    /// A signal expires here (none for an open trade).
    pub until_ms: Option<i64>,
    pub in_book: bool,
}

impl Watched {
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "key": self.key,
            "symbol": self.symbol,
            "name": self.name,
            "tf": self.tf,
            "status": self.status,
            "trigger": self.trigger,
            "stop": self.stop,
            "target": self.target,
            "from_ms": self.from_ms,
            "until_ms": self.until_ms,
            "in_book": self.in_book,
        })
    }

    #[must_use]
    pub fn from_json(v: &Value) -> Option<Self> {
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        let number = |k: &str| v.get(k).and_then(Value::as_f64).filter(|x| x.is_finite());
        Some(Self {
            key: text("key")?,
            symbol: text("symbol")?,
            name: text("name")?,
            tf: text("tf")?,
            status: text("status")?,
            trigger: number("trigger")?,
            stop: number("stop")?,
            target: number("target")?,
            from_ms: v.get("from_ms").and_then(Value::as_i64)?,
            until_ms: v.get("until_ms").and_then(Value::as_i64),
            in_book: v.get("in_book").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

/// Account the share count is worked out for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Account {
    /// `STOCK_ACCOUNT_USD` in `.env`.
    Real(f64),
    /// The paper book's balance.
    Paper(f64),
}

impl Account {
    fn value(self) -> f64 {
        match self {
            Self::Real(v) | Self::Paper(v) => v,
        }
    }

    fn label(self) -> String {
        match self {
            Self::Real(v) => format!("a tua conta de {} (STOCK_ACCOUNT_USD)", money_pt(v)),
            Self::Paper(v) => format!(
                "o saldo da conta fictícia, {} (para usar o teu saldo real, coloque STOCK_ACCOUNT_USD=valor no .env)",
                money_pt(v)
            ),
        }
    }
}

/// True from Monday to Friday between 13:00 and 22:00 UTC, which covers the
/// New York session (09:30 to 16:00) in summer and winter time.
#[must_use]
pub fn in_session(now_ms: i64) -> bool {
    // 1970-01-01 was a Thursday: 0 Thursday, 1 Friday, 2 Saturday, 3 Sunday.
    let weekday = now_ms.div_euclid(DAY_MS).rem_euclid(7);
    let hour = now_ms.rem_euclid(DAY_MS) / HOUR_MS;
    !matches!(weekday, 2 | 3) && (13..22).contains(&hour)
}

fn chart_pt(tf: &str) -> &'static str {
    if tf == "1w" { "semanal" } else { "diário" }
}

/// Whole shares for a buy at `price` with `stop`: 2% of the account at
/// risk, the position at most a third of it.
#[must_use]
pub fn shares(account: f64, price: f64, stop: f64) -> u64 {
    let risk_per_share = price - stop;
    if risk_per_share <= 0.0 || price <= 0.0 || account <= 0.0 {
        return 0;
    }
    let budget = (RISK * account / risk_per_share * price).min(account * POSITION_CAP);
    let n = (budget / price).floor();
    if n.is_finite() && n > 0.0 {
        // Floor of a positive finite value that fits an account size.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let n = n as u64;
        n
    } else {
        0
    }
}

fn exits(w: &Watched, bars: &[tp_core::intraday::Bar], price: f64, out: &mut Vec<StageAlert>) {
    let stop_at = bars.iter().find(|b| b.low <= w.stop);
    let target_at = bars.iter().find(|b| b.high >= w.target);
    let book = if w.in_book {
        "\nA conta fictícia registra a saída pelo candle fechado, no próximo aviso de fechamento."
    } else {
        ""
    };
    let first = match (stop_at, target_at) {
        (Some(s), Some(t)) if t.open_time_ms < s.open_time_ms => Some((false, t)),
        (Some(s), _) => Some((true, s)),
        (None, Some(t)) => Some((false, t)),
        (None, None) => None,
    };
    let Some((stopped, bar)) = first else {
        return;
    };
    let (key, message) = if stopped {
        (
            format!("{}|stopdia", w.key),
            format!(
                "{} {}: BATEU NO STOP {} às {} (trap, Stormer). Preço agora {}.\nSe você comprou, venda agora: na Nomad, ordem a mercado.{book}",
                w.name,
                chart_pt(&w.tf),
                fmt_pt(w.stop),
                when(bar.open_time_ms),
                fmt_pt(price)
            ),
        )
    } else {
        (
            format!("{}|alvodia", w.key),
            format!(
                "{} {}: CHEGOU NO ALVO {} às {} (trap, Stormer). Preço agora {}.\nSe você comprou, venda agora para garantir o lucro: na Nomad, ordem a mercado ou limitada no alvo.{book}",
                w.name,
                chart_pt(&w.tf),
                fmt_pt(w.target),
                when(bar.open_time_ms),
                fmt_pt(price)
            ),
        )
    };
    out.push(StageAlert {
        key,
        message,
        notify: true,
    });
}

fn entry_text(w: &Watched, at_ms: i64, price: f64, delay_min: i64, account: Account) -> String {
    let mut text = format!(
        "{} {}: PASSOU DO GATILHO {} às {} (trap, Stormer, compra).\nPreço agora {} ({} do gatilho",
        w.name,
        chart_pt(&w.tf),
        fmt_pt(w.trigger),
        when(at_ms),
        fmt_pt(price),
        pct_pt(change_pct(w.trigger, price))
    );
    if delay_min > 0 {
        let _ = write!(text, "; a fonte tem {delay_min} min de atraso");
    }
    text.push_str(").");
    if price <= w.stop {
        let _ = write!(
            text,
            "\nJá voltou para o stop {} ou abaixo: não entrar.",
            fmt_pt(w.stop)
        );
        return text;
    }
    let rr = (w.target - price) / (price - w.stop);
    let _ = write!(
        text,
        "\nComprando agora: stop {} ({} abaixo), alvo {} ({} acima), {} vezes o risco. O teste comprou no gatilho, com 2 vezes.",
        fmt_pt(w.stop),
        pct_pt(change_pct(price, w.stop).abs()).trim_start_matches('+'),
        fmt_pt(w.target),
        pct_pt(change_pct(price, w.target)).trim_start_matches('+'),
        ratio_pt(rr)
    );
    if rr < LATE_RR {
        let _ = write!(
            text,
            "\nO preço já correu: abaixo de {} vezes o risco, o teste não cobre essa entrada.",
            ratio_pt(LATE_RR)
        );
    }
    let n = shares(account.value(), price, w.stop);
    if n == 0 {
        let _ = write!(
            text,
            "\nCom {}, uma ação passa do limite por trade (um terço da conta).",
            account.label()
        );
    } else {
        #[allow(clippy::cast_precision_loss)] // share counts are small
        let count = n as f64;
        let _ = write!(
            text,
            "\nQuantidade: {n} {} (cerca de {}), perde {} se bater o stop. Conta: {}, risco de 2%.",
            if n == 1 { "ação" } else { "ações" },
            money_pt(count * price),
            money_pt(count * (price - w.stop)),
            account.label()
        );
    }
    text.push_str(
        "\nNa Nomad não existe ordem stop: compre com ordem a mercado ou limitada. O stop e o alvo ficam por conta destes avisos, de hora em hora.",
    );
    text
}

/// New alerts for one watched setup from its intraday candles.
#[must_use]
pub fn alerts_for(
    w: &Watched,
    d: &Intraday,
    sent: &BTreeSet<String>,
    account: Account,
) -> Vec<StageAlert> {
    let mut out = Vec::new();
    let bars: Vec<tp_core::intraday::Bar> = d
        .bars
        .iter()
        .filter(|b| b.open_time_ms >= w.from_ms)
        .copied()
        .collect();
    let Some(price) = d.price.or_else(|| bars.last().map(|b| b.close)) else {
        return out;
    };
    let after: &[tp_core::intraday::Bar] = if w.status == "aberto" {
        &bars
    } else {
        let valid = |b: &&tp_core::intraday::Bar| w.until_ms.is_none_or(|u| b.open_time_ms < u);
        let Some(at) = bars.iter().filter(valid).position(|b| b.high >= w.trigger) else {
            return out;
        };
        let key = format!("{}|gatilho", w.key);
        if !sent.contains(&key) {
            out.push(StageAlert {
                key,
                message: entry_text(w, bars[at].open_time_ms, price, d.delay_min, account),
                notify: true,
            });
        }
        &bars[at..]
    };
    let mut exit = Vec::new();
    exits(w, after, price, &mut exit);
    out.extend(exit.into_iter().filter(|a| !sent.contains(&a.key)));
    out
}

/// Reads the watch list written by `tp markets` (empty when missing).
#[must_use]
pub fn read_watch(live_dir: &Path) -> Vec<Watched> {
    let text = std::fs::read_to_string(live_dir.join(WATCH_FILE)).unwrap_or_default();
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(Watched::from_json)
        .collect()
}

/// Every new intraday alert, during the New York session only.
pub async fn check(
    live_dir: &Path,
    now_ms: i64,
    sent: &BTreeSet<String>,
    account: Account,
) -> Vec<(String, StageAlert)> {
    let watch = read_watch(live_dir);
    if watch.is_empty() || !in_session(now_ms) {
        return Vec::new();
    }
    let symbols: BTreeSet<&str> = watch.iter().map(|w| w.symbol.as_str()).collect();
    let mut out = Vec::new();
    for symbol in symbols {
        let data = match fetch_intraday(symbol).await {
            Ok(d) => d,
            Err(e) => {
                eprintln!("warning: {symbol} intraday skipped: {e}");
                continue;
            }
        };
        for w in watch.iter().filter(|w| w.symbol == symbol) {
            out.extend(
                alerts_for(w, &data, sent, account)
                    .into_iter()
                    .map(|a| (w.name.clone(), a)),
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tp_core::intraday::Bar;

    fn bar(ms: i64, high: f64, low: f64) -> Bar {
        Bar {
            open_time_ms: ms,
            open: low,
            high,
            low,
            close: high,
            volume: 1.0,
        }
    }

    fn watched(status: &str) -> Watched {
        Watched {
            key: "AAPL|1d|trap|long|0".to_owned(),
            symbol: "AAPL".to_owned(),
            name: "Apple".to_owned(),
            tf: "1d".to_owned(),
            status: status.to_owned(),
            trigger: 100.0,
            stop: 95.0,
            target: 110.0,
            from_ms: 1000,
            until_ms: Some(10_000),
            in_book: true,
        }
    }

    fn data(bars: Vec<Bar>, price: f64) -> Intraday {
        Intraday {
            bars,
            price: Some(price),
            delay_min: 0,
        }
    }

    #[test]
    fn session_hours_skip_weekends() {
        // 2025-11-07 is a Friday; 2025-11-08 a Saturday.
        let friday = 1_762_473_600_000;
        assert!(in_session(friday + 15 * HOUR_MS));
        assert!(!in_session(friday + 23 * HOUR_MS));
        assert!(!in_session(friday + DAY_MS + 15 * HOUR_MS));
    }

    #[test]
    fn whole_shares_risk_two_percent_and_a_third_at_most() {
        // $3,000 account, $5 risk a share: $60 risk is 12 shares, but a
        // third of the account ($1,000) buys 9 at $101.
        assert_eq!(shares(3000.0, 101.0, 96.0), 9);
        // Wide stop: the risk decides (2% of $10,000 = $200, $20 a share).
        assert_eq!(shares(10_000.0, 100.0, 80.0), 10);
        assert_eq!(shares(100.0, 500.0, 490.0), 0);
        assert_eq!(shares(1000.0, 100.0, 100.0), 0);
    }

    #[test]
    fn the_trigger_alerts_once_with_the_share_count() {
        let w = watched("sinal");
        let d = data(
            vec![
                bar(500, 120.0, 90.0),
                bar(2000, 99.0, 97.0),
                bar(3000, 101.0, 98.0),
            ],
            101.0,
        );
        let a = alerts_for(&w, &d, &BTreeSet::new(), Account::Real(3000.0));
        assert_eq!(a.len(), 1, "{a:?}");
        assert!(a[0].key.ends_with("|gatilho"));
        assert!(
            a[0].message.contains("PASSOU DO GATILHO"),
            "{}",
            a[0].message
        );
        assert!(
            a[0].message.contains("Quantidade: 9 ações"),
            "{}",
            a[0].message
        );
        let sent: BTreeSet<String> = [a[0].key.clone()].into();
        assert!(alerts_for(&w, &d, &sent, Account::Real(3000.0)).is_empty());
    }

    #[test]
    fn a_late_entry_is_flagged() {
        let w = watched("sinal");
        let d = data(vec![bar(2000, 106.0, 99.0)], 106.0);
        let a = alerts_for(&w, &d, &BTreeSet::new(), Account::Paper(2200.0));
        assert!(a[0].message.contains("já correu"), "{}", a[0].message);
        assert!(
            a[0].message.contains("STOCK_ACCOUNT_USD=valor"),
            "{}",
            a[0].message
        );
    }

    #[test]
    fn no_alert_before_the_trigger_or_after_the_deadline() {
        let w = watched("sinal");
        let d = data(vec![bar(2000, 99.0, 97.0), bar(12_000, 105.0, 99.0)], 104.0);
        assert!(alerts_for(&w, &d, &BTreeSet::new(), Account::Paper(2200.0)).is_empty());
    }

    #[test]
    fn open_trades_alert_the_stop_before_the_target() {
        let w = watched("aberto");
        let d = data(vec![bar(2000, 104.0, 94.0), bar(3000, 111.0, 100.0)], 108.0);
        let a = alerts_for(&w, &d, &BTreeSet::new(), Account::Paper(2200.0));
        assert_eq!(a.len(), 1);
        assert!(a[0].key.ends_with("|stopdia"));
        assert!(a[0].message.contains("BATEU NO STOP"), "{}", a[0].message);
        let d = data(vec![bar(3000, 111.0, 100.0)], 110.5);
        let a = alerts_for(&w, &d, &BTreeSet::new(), Account::Paper(2200.0));
        assert!(a[0].key.ends_with("|alvodia"));
    }

    #[test]
    fn the_watch_list_round_trips() {
        let w = watched("sinal");
        assert_eq!(Watched::from_json(&w.to_json()), Some(w));
    }
}
