//! Live alerts from `tp capitulation-study`, on BTC, ETH and SOL:
//! - the capitulation wick with extreme fear (the day falls 15% or more
//!   below the previous close, closes above the middle of the fall, and the
//!   Fear & Greed Index is at 20 or less): passed at 90 days only, so it
//!   comes with the 90-day plan and follow-ups at 30 and 90 days;
//! - the three gauges together on BTC and ETH (Fear & Greed at 20 or less,
//!   the close 50% or more below the all-time high, a year or more after
//!   it): few past cases, no proof, a long-term reading.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, NaiveDate};
use serde_json::{Value, json};
use tp_core::intraday::Bar;
use tp_data::fear_greed::Reading;

use super::fmt_pt;

/// Sent alert keys, one per line.
pub const SENT_FILE: &str = "capitulation_sent.txt";
/// Capitulation signals being followed, one JSON per line.
pub const WATCH_FILE: &str = "capitulation_watch.jsonl";

const DAY_MS: i64 = 24 * 3600 * 1000;
const WICK: f64 = 0.15;
const FEAR_MAX: u8 = 20;
const FOLLOW_UPS: [i64; 2] = [30, 90];

/// One alert to send.
#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub key: String,
    pub asset: String,
    pub kind: &'static str,
    pub text: String,
}

/// A capitulation signal being followed.
#[derive(Debug, Clone, PartialEq)]
pub struct Watched {
    pub asset: String,
    pub signal_ms: i64,
    /// Close of the signal day.
    pub price: f64,
}

impl Watched {
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({ "asset": self.asset, "signal_ms": self.signal_ms, "price": self.price })
    }

    #[must_use]
    pub fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            asset: v["asset"].as_str()?.to_owned(),
            signal_ms: v["signal_ms"].as_i64()?,
            price: v["price"].as_f64().filter(|p| *p > 0.0)?,
        })
    }
}

fn date_of(ms: i64) -> Option<NaiveDate> {
    DateTime::from_timestamp_millis(ms).map(|d| d.date_naive())
}

fn date_pt(ms: i64) -> String {
    date_of(ms).map_or_else(String::new, |d| d.format("%d/%m/%Y").to_string())
}

fn whole(v: f64) -> String {
    format!("{v:.0}%")
}

/// Index of the last daily candle that has closed at `now_ms`.
fn last_closed(bars: &[Bar], now_ms: i64) -> Option<usize> {
    bars.iter().rposition(|b| b.open_time_ms + DAY_MS <= now_ms)
}

/// New alerts and new signals to follow.
#[must_use]
#[allow(clippy::too_many_lines)] // the two alerts and the follow-ups, in order
pub fn check(
    dailies: &BTreeMap<String, Vec<Bar>>,
    fear: &[Reading],
    now_ms: i64,
    sent: &BTreeSet<String>,
    watched: &[Watched],
) -> (Vec<Alert>, Vec<Watched>) {
    let fear_on: BTreeMap<NaiveDate, u8> = fear.iter().map(|r| (r.date, r.value)).collect();
    let mut alerts = Vec::new();
    let mut follow = Vec::new();
    for (asset, bars) in dailies {
        let Some(t) = last_closed(bars, now_ms).filter(|t| *t >= 1) else {
            continue;
        };
        let (prev, day) = (bars[t - 1].close, &bars[t]);
        let Some(fg) = date_of(day.open_time_ms).and_then(|d| fear_on.get(&d).copied()) else {
            continue;
        };
        let date = date_pt(day.open_time_ms);
        // The capitulation wick with extreme fear.
        let fall = prev - day.low;
        let key = format!("{asset}|capitulacao|{}", day.open_time_ms);
        if fg <= FEAR_MAX
            && day.low <= prev * (1.0 - WICK)
            && day.close >= day.low + 0.5 * fall
            && !sent.contains(&key)
        {
            alerts.push(Alert {
                key,
                asset: asset.clone(),
                kind: "capitulacao",
                text: format!(
                    "CAPITULAÇÃO NO {asset}, com medo extremo (candidato: passou só em 90 dias).\nOntem ({date}) o {asset} caiu até {} ({} abaixo do fechamento anterior) e fechou em {}, acima da metade da queda. O Medo e Ganância estava em {fg}.\nO que o teste mostrou (100 casos em 30 dias diferentes do mercado, desde 2018): em 90 dias a compra ganhou do dia típico da moeda em 84% dos casos, com resultado típico de +21,5%. Mas em 30 dias ainda estava perdendo (típico -5,7%), e depois da compra o preço caiu tipicamente mais 22% (1 em cada 10 casos, mais de 38%). Em 1 ano voltou a perder.\nComo usar: compra pequena, sem alavancagem, contando com mais queda, e plano de vender em cerca de 90 dias. O painel avisa de novo em 30 e em 90 dias com o resultado.",
                    fmt_pt(day.low),
                    whole(fall / prev * 100.0),
                    fmt_pt(day.close),
                ),
            });
            follow.push(Watched {
                asset: asset.clone(),
                signal_ms: day.open_time_ms,
                price: day.close,
            });
        }
        // The three gauges, BTC and ETH, once a month at most.
        if asset == "BTC" || asset == "ETH" {
            let (mut ath, mut ath_ms) = (f64::MIN, 0);
            for b in &bars[..=t] {
                if b.high > ath {
                    ath = b.high;
                    ath_ms = b.open_time_ms;
                }
            }
            let below = 1.0 - day.close / ath;
            let days = (day.open_time_ms - ath_ms) / DAY_MS;
            let prefix = format!("{asset}|reguas|");
            let recent = sent
                .iter()
                .filter_map(|k| k.strip_prefix(&prefix))
                .any(|ms| {
                    ms.parse::<i64>()
                        .is_ok_and(|ms| day.open_time_ms - ms < 30 * DAY_MS)
                });
            if fg <= FEAR_MAX && below >= 0.5 && days >= 365 && !recent {
                let past = if asset == "BTC" {
                    "9 no BTC; 1 ano depois a compra ganhou do dia típico em 7 de 9"
                } else {
                    "10 no ETH; 1 ano depois a compra ganhou do dia típico em 8 de 10"
                };
                alerts.push(Alert {
                    key: format!("{prefix}{}", day.open_time_ms),
                    asset: asset.clone(),
                    kind: "reguas",
                    text: format!(
                        "TRÊS RÉGUAS ALINHADAS NO {asset} (sem prova: poucos casos).\nOntem ({date}) o {asset} fechou em {}, {} abaixo da máxima histórica de {} ({}), {days} dias depois dela, com o Medo e Ganância em {fg}.\nNas outras vezes desde 2018 ({past}; na prática, 3 fundos de ciclo). Mas em 30 dias perdeu na maioria das vezes, e em março de 2020 o preço ainda caiu 52% depois da compra.\nComo usar: compra de longo prazo, em partes, sem alavancagem. Não é trade.",
                        fmt_pt(day.close),
                        whole(below * 100.0),
                        fmt_pt(ath),
                        date_pt(ath_ms),
                    ),
                });
            }
        }
    }
    // Follow-ups at 30 and 90 days.
    for w in watched {
        let Some(bars) = dailies.get(&w.asset) else {
            continue;
        };
        let Some(now) = bars.last() else {
            continue;
        };
        let since: Vec<&Bar> = bars
            .iter()
            .filter(|b| b.open_time_ms > w.signal_ms)
            .collect();
        let low = since.iter().map(|b| b.low).fold(f64::MAX, f64::min);
        for days in FOLLOW_UPS {
            let key = format!("{}|capitulacao|{}|{days}", w.asset, w.signal_ms);
            if sent.contains(&key) || now_ms < w.signal_ms + (days + 1) * DAY_MS {
                continue;
            }
            let change = (now.close / w.price - 1.0) * 100.0;
            alerts.push(Alert {
                key,
                asset: w.asset.clone(),
                kind: "capitulacao",
                text: format!(
                    "CAPITULAÇÃO NO {}, {days} dias depois do sinal de {}: o preço foi de {} para {} ({}{}). O ponto mais baixo no caminho foi {} ({} abaixo do preço do sinal).{}",
                    w.asset,
                    date_pt(w.signal_ms),
                    fmt_pt(w.price),
                    fmt_pt(now.close),
                    if change >= 0.0 { "+" } else { "" },
                    format!("{change:.1}%").replace('.', ","),
                    fmt_pt(low.min(w.price)),
                    whole((1.0 - low.min(w.price) / w.price) * 100.0),
                    if days == 90 {
                        " Pelo plano do teste, é a hora de vender."
                    } else {
                        " O plano do teste segue até os 90 dias."
                    }
                ),
            });
        }
    }
    (alerts, follow)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    fn bar(day: i64, open: f64, high: f64, low: f64, close: f64) -> Bar {
        Bar {
            open_time_ms: day * DAY_MS,
            open,
            high,
            low,
            close,
            volume: 1.0,
        }
    }

    fn fear(day: i64, value: u8) -> Reading {
        Reading {
            date: date_of(day * DAY_MS).unwrap(),
            value,
            class: String::new(),
        }
    }

    #[test]
    fn a_wick_with_extreme_fear_alerts_once_and_is_followed() {
        let mut bars: Vec<Bar> = (0..10).map(|d| bar(d, 100.0, 101.0, 99.0, 100.0)).collect();
        bars.push(bar(10, 99.0, 99.0, 80.0, 93.0));
        bars.push(bar(11, 93.0, 95.0, 92.0, 94.0));
        let dailies: BTreeMap<String, Vec<Bar>> = [("SOL".to_owned(), bars)].into();
        let now = 11 * DAY_MS + 3_600_000;
        let (a, w) = check(&dailies, &[fear(10, 18)], now, &BTreeSet::new(), &[]);
        assert_eq!(a.len(), 1, "{a:?}");
        assert!(a[0].text.starts_with("CAPITULAÇÃO NO SOL"), "{}", a[0].text);
        assert!(
            a[0].text.contains("(20% abaixo do fechamento anterior)"),
            "{}",
            a[0].text
        );
        assert_eq!(w[0].price, 93.0);
        // Same day again: nothing; with fear at 40: nothing.
        let sent: BTreeSet<String> = a.iter().map(|x| x.key.clone()).collect();
        assert!(
            check(&dailies, &[fear(10, 18)], now, &sent, &[])
                .0
                .is_empty()
        );
        assert!(
            check(&dailies, &[fear(10, 40)], now, &BTreeSet::new(), &[])
                .0
                .is_empty()
        );
        // 31 days later the follow-up comes.
        let later = (10 + 31) * DAY_MS + 3_600_000;
        let (f, _) = check(&dailies, &[], later, &sent, &w);
        assert_eq!(f.len(), 1);
        assert!(f[0].text.contains("30 dias depois"), "{}", f[0].text);
    }

    #[test]
    fn the_three_gauges_need_all_three() {
        // A high of 200 on day 0, then 400 days near 90 (55% below).
        let mut bars = vec![bar(0, 150.0, 200.0, 140.0, 190.0)];
        bars.extend((1..=400).map(|d| bar(d, 90.0, 91.0, 89.0, 90.0)));
        let dailies: BTreeMap<String, Vec<Bar>> = [("BTC".to_owned(), bars)].into();
        let now = 400 * DAY_MS + DAY_MS + 1;
        let (a, _) = check(&dailies, &[fear(400, 15)], now, &BTreeSet::new(), &[]);
        assert_eq!(a.len(), 1);
        assert!(
            a[0].text.starts_with("TRÊS RÉGUAS ALINHADAS NO BTC"),
            "{}",
            a[0].text
        );
        assert!(a[0].text.contains("55% abaixo"), "{}", a[0].text);
        // Fear at 30: nothing.
        assert!(
            check(&dailies, &[fear(400, 30)], now, &BTreeSet::new(), &[])
                .0
                .is_empty()
        );
        // Already sent 10 days before: nothing for a month.
        let sent: BTreeSet<String> = [format!("BTC|reguas|{}", 390 * DAY_MS)].into();
        assert!(
            check(&dailies, &[fear(400, 15)], now, &sent, &[])
                .0
                .is_empty()
        );
    }
}
