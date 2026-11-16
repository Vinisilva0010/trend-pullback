//! The cycle map of a crypto asset for the chart page: lines for how deep
//! its falls usually went from the top, how far the bounces inside a fall
//! usually rose, where a bounce more often turned into a rise, and the
//! same in plain sentences. History of the asset itself on daily candles;
//! not a buy or sell signal.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Datelike, Weekday};
use serde_json::{Value, json};
use tp_core::cycle::{Bounce, Now, bounces, falls, now, quantile, zigzag};
use tp_core::intraday::Bar;

use super::capitulation_live::Alert;
use super::{count_f64, fmt_pt};

/// A fall or a rise counts from 15%.
pub const THRESHOLD: f64 = 0.15;
/// Shares of the fall a bounce may take back, tested for turning.
const LEVELS: [f64; 4] = [0.382, 0.5, 0.618, 0.786];
/// Bounces needed at a level before its share is shown.
const MIN_AT_LEVEL: usize = 5;

fn date(bars: &[Bar], i: usize) -> String {
    bars.get(i)
        .and_then(|b| DateTime::from_timestamp_millis(b.open_time_ms))
        .map_or_else(String::new, |d| d.format("%d/%m/%Y").to_string())
}

fn whole(share: f64) -> String {
    format!("{:.0}%", share * 100.0)
}

fn level_name(k: f64) -> String {
    format!("{:.1}%", k * 100.0)
        .replace(".0%", "%")
        .replace('.', ",")
}

/// Share of the bounces that reached `k` of their fall and then went above
/// the top before a new bottom, with how many reached it.
fn turned_at(all: &[Bounce], k: f64) -> (f64, usize) {
    let reached: Vec<&Bounce> = all.iter().filter(|b| b.retrace >= k).collect();
    let turned = reached.iter().filter(|b| b.turned).count();
    (
        count_f64(turned) / count_f64(reached.len().max(1)),
        reached.len(),
    )
}

/// The cycle map, or `None` when the history is too short.
#[must_use]
#[allow(clippy::too_many_lines)] // the lines and the sentences, in reading order
pub fn cycle_json(name: &str, bars: &[Bar]) -> Option<Value> {
    let (swings, forming) = zigzag(bars, THRESHOLD);
    let state = now(&swings, forming)?;
    let close = bars.last()?.close;
    let depths = falls(&swings);
    let all = bounces(bars, &swings);
    let failed: Vec<f64> = all
        .iter()
        .filter(|b| !b.turned)
        .map(|b| b.retrace)
        .collect();
    let (top, top_i, bottom, code) = match state {
        Now::Falling { top, low } => (top.price, top.index, Some((low.price, low.index)), "queda"),
        Now::Bouncing { top, bottom, .. } => (
            top.price,
            top.index,
            Some((bottom.price, bottom.index)),
            "repique",
        ),
        Now::Rising { high } => (high.price, high.index, None, "alta"),
    };
    let mut lines = Vec::new();
    let mut text = Vec::new();
    let drop_now = 1.0 - close / top;
    match (state, bottom) {
        (Now::Falling { .. }, Some((low, low_i))) => text.push(format!(
            "{name} está em queda: o último topo foi {} em {}, e hoje o preço está {} abaixo dele. O fundo até agora é {} ({}), {} abaixo do topo.",
            fmt_pt(top),
            date(bars, top_i),
            whole(drop_now),
            fmt_pt(low),
            date(bars, low_i),
            whole(1.0 - low / top)
        )),
        (Now::Bouncing { .. }, Some((low, low_i))) => text.push(format!(
            "{name} está num repique dentro da queda: caiu de {} ({}) até {} ({}), uma queda de {}, e já devolveu {} dessa queda.",
            fmt_pt(top),
            date(bars, top_i),
            fmt_pt(low),
            date(bars, low_i),
            whole(1.0 - low / top),
            whole((close - low) / (top - low))
        )),
        _ => text.push(format!(
            "{name} está em alta: passou do topo da última queda. A máxima desde então é {} ({}), e hoje o preço está {} abaixo dela.",
            fmt_pt(top),
            date(bars, top_i),
            whole(drop_now.max(0.0))
        )),
    }
    let (q50, q75, q90) = (
        quantile(&depths, 0.5),
        quantile(&depths, 0.75),
        quantile(&depths, 0.9),
    );
    if let (Some(a), Some(b), Some(c)) = (q50, q75, q90) {
        let since = date(bars, 0);
        text.push(format!(
            "Desde {}, {name} teve {} quedas de 15% ou mais. Metade parou em até {} abaixo do topo, 3 em cada 4 em até {} e 9 em cada 10 em até {}.",
            since.get(6..).unwrap_or(&since),
            depths.len(),
            whole(a),
            whole(b),
            whole(c)
        ));
        for (d, title) in [
            (a, format!("Metade das quedas -{}", whole(a))),
            (b, format!("3 em 4 quedas -{}", whole(b))),
            (c, format!("9 em 10 quedas -{}", whole(c))),
        ] {
            let p = top * (1.0 - d);
            if p < close {
                lines.push(json!({ "price": p, "title": title, "kind": "fall" }));
            }
        }
    }
    if let (Some(a), Some(b)) = (quantile(&failed, 0.5), quantile(&failed, 0.75)) {
        text.push(format!(
            "Nos {} repiques que vieram depois de uma queda e acabaram em novo fundo, metade devolveu até {} da queda e 3 em cada 4 até {} antes de voltar a cair.",
            failed.len(),
            whole(a),
            whole(b)
        ));
        if let Some((low, _)) = bottom {
            lines.push(json!({ "price": low + a * (top - low), "title": "Metade dos repiques parou", "kind": "bounce" }));
            lines.push(json!({ "price": low + b * (top - low), "title": "3 em 4 repiques pararam", "kind": "bounce" }));
        }
    }
    let shown: Vec<(f64, f64, usize)> = LEVELS
        .iter()
        .map(|k| {
            let (share, n) = turned_at(&all, *k);
            (*k, share, n)
        })
        .filter(|(_, _, n)| *n >= MIN_AT_LEVEL)
        .collect();
    if !shown.is_empty() {
        let parts: Vec<String> = shown
            .iter()
            .map(|(k, share, n)| {
                format!(
                    "passando de {} da queda, virou em {} das {n} vezes",
                    level_name(*k),
                    whole(*share)
                )
            })
            .collect();
        text.push(format!(
            "Quando um repique devolve uma parte grande da queda, a chance de virar alta (passar do topo antes de fazer novo fundo) muda: {}.",
            parts.join("; ")
        ));
        if let Some((low, _)) = bottom
            && let Some((k, share, _)) = shown.iter().find(|(_, share, _)| *share >= 0.5)
        {
            lines.push(json!({
                "price": low + k * (top - low),
                "title": format!("Acima: virou em {}", whole(*share)),
                "kind": "turn",
            }));
        }
    }
    if bottom.is_some() {
        lines.push(json!({ "price": top, "title": "Topo da queda", "kind": "top" }));
    }
    // The bottom before this fall: staying above it makes a higher low.
    if let (Some((low, _)), Some(prev)) = (
        bottom,
        swings.iter().rev().find(|s| !s.top && s.index < top_i),
    ) {
        lines.push(json!({ "price": prev.price, "title": "Fundo anterior", "kind": "prevlow" }));
        let gap = (low / prev.price - 1.0) * 100.0;
        let side = if gap >= 0.0 {
            format!(
                "está {} acima dele. Se a queda parar aí, forma um fundo mais alto, o primeiro sinal de virada; a virada se confirma quando uma semana fechar acima do topo da queda ({}).",
                whole(gap / 100.0),
                fmt_pt(top)
            )
        } else {
            format!(
                "já está {} abaixo dele: a queda segue fazendo fundos mais baixos, o desenho de tendência de baixa.",
                whole(-gap / 100.0)
            )
        };
        text.push(format!(
            "O fundo anterior foi {} ({}). O fundo desta queda, {}, {side}",
            fmt_pt(prev.price),
            date(bars, prev.index),
            fmt_pt(low),
        ));
    }
    text.push(format!(
        "É o histórico do próprio {name} em candles diários, para ajudar a ler o gráfico. Não é sinal de compra ou venda e não passou por teste de resultado."
    ));
    Some(json!({ "state": code, "lines": lines, "text": text }))
}

/// Alerts of a higher low on the daily candles that have closed: when a
/// bottom confirmed by a 15% rise is above the bottom before it, and then
/// when a week closes (Sunday's close) above the top between them. A
/// chart reading with no test behind it.
#[must_use]
pub fn turn_alerts(
    dailies: &BTreeMap<String, Vec<Bar>>,
    now_ms: i64,
    sent: &BTreeSet<String>,
) -> Vec<Alert> {
    const DAY_MS: i64 = 24 * 3600 * 1000;
    let mut out = Vec::new();
    for (asset, all) in dailies {
        let closed: Vec<Bar> = all
            .iter()
            .filter(|b| b.open_time_ms + DAY_MS <= now_ms)
            .copied()
            .collect();
        let Some(last) = closed.last() else {
            continue;
        };
        let (swings, _) = zigzag(&closed, THRESHOLD);
        let bottoms: Vec<usize> = (0..swings.len()).filter(|i| !swings[*i].top).collect();
        let [.., b1, b2] = bottoms[..] else {
            continue;
        };
        let (old, new) = (swings[b1], swings[b2]);
        let Some(top) = swings[b1..b2].iter().find(|s| s.top) else {
            continue;
        };
        // Only a recent bottom, so the first run does not repeat old history.
        if new.price <= old.price || closed.len() - 1 - new.index > 60 {
            continue;
        }
        let ms = closed[new.index].open_time_ms;
        let key = format!("{asset}|fundo-mais-alto|{ms}");
        if !sent.contains(&key) {
            out.push(Alert {
                key,
                asset: asset.clone(),
                kind: "ciclo",
                text: format!(
                    "FUNDO MAIS ALTO NO {asset} (leitura do gráfico, sem teste).\nO {asset} subiu 15% a partir do fundo de {} ({}), que ficou {} acima do fundo anterior, {} ({}). É o primeiro sinal de virada de uma queda.\nA virada se confirma quando uma semana fechar acima do topo da queda, {}. Se o preço voltar abaixo de {}, o sinal cai.",
                    fmt_pt(new.price),
                    date(&closed, new.index),
                    whole(new.price / old.price - 1.0),
                    fmt_pt(old.price),
                    date(&closed, old.index),
                    fmt_pt(top.price),
                    fmt_pt(new.price),
                ),
            });
        }
        let weekly_close = DateTime::from_timestamp_millis(last.open_time_ms)
            .is_some_and(|d| d.weekday() == Weekday::Sun);
        let key = format!("{asset}|virada|{ms}");
        if weekly_close && last.close > top.price && !sent.contains(&key) {
            out.push(Alert {
                key,
                asset: asset.clone(),
                kind: "ciclo",
                text: format!(
                    "VIRADA CONFIRMADA NO {asset} (leitura do gráfico, sem teste).\nA semana fechou em {}, acima do topo da queda ({}), depois de um fundo mais alto ({} contra {} do fundo anterior). É o desenho de começo de tendência de alta: fundo mais alto e topo mais alto.\nA entrada testada do painel continua sendo o rompimento da máxima de 20 dias.",
                    fmt_pt(last.close),
                    fmt_pt(top.price),
                    fmt_pt(new.price),
                    fmt_pt(old.price),
                ),
            });
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn day(d: i64, high: f64, low: f64) -> Bar {
        Bar {
            open_time_ms: d * 86_400_000,
            open: low,
            high,
            low,
            close: f64::midpoint(high, low),
            volume: 1.0,
        }
    }

    #[test]
    fn a_higher_low_and_then_a_weekly_close_above_the_top_alert() {
        // Bottom at 70, top at 100, higher bottom at 80, rise to 95.
        // 1970-01-04 (day 3) was a Sunday: day 3 + 7k are Sundays.
        let mut bars = vec![
            day(0, 120.0, 118.0),
            day(1, 80.0, 70.0),
            day(2, 100.0, 95.0),
        ];
        bars.extend((3..9).map(|d| day(d, 90.0, 85.0)));
        bars.push(day(9, 84.0, 80.0));
        bars.push(day(10, 95.0, 92.0));
        let dailies: BTreeMap<String, Vec<Bar>> = [("BTC".to_owned(), bars.clone())].into();
        let now = 11 * 86_400_000;
        let a = turn_alerts(&dailies, now, &BTreeSet::new());
        assert_eq!(a.len(), 1, "{a:?}");
        assert!(
            a[0].text.starts_with("FUNDO MAIS ALTO NO BTC"),
            "{}",
            a[0].text
        );
        // Sunday (day 17) closing above 100.
        bars.extend((11..17).map(|d| day(d, 98.0, 94.0)));
        bars.push(day(17, 106.0, 102.0));
        let dailies: BTreeMap<String, Vec<Bar>> = [("BTC".to_owned(), bars)].into();
        let sent: BTreeSet<String> = a.iter().map(|x| x.key.clone()).collect();
        let v = turn_alerts(&dailies, 18 * 86_400_000, &sent);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(
            v[0].text.starts_with("VIRADA CONFIRMADA NO BTC"),
            "{}",
            v[0].text
        );
    }

    fn bar(day: i64, high: f64, low: f64, close: f64) -> Bar {
        Bar {
            open_time_ms: day * 86_400_000,
            open: close,
            high,
            low,
            close,
            volume: 1.0,
        }
    }

    #[test]
    fn a_fall_gets_depth_and_bounce_lines() {
        // Rises and falls of about 30%, then a fall from 130 still going on.
        let mut bars = Vec::new();
        let mut day = 0;
        for _ in 0..6 {
            for (h, l) in [
                (100.0, 98.0),
                (80.0, 70.0),
                (85.0, 78.0),
                (75.0, 68.0),
                (130.0, 100.0),
            ] {
                bars.push(bar(day, h, l, f64::midpoint(h, l)));
                day += 1;
            }
        }
        bars.push(bar(day, 110.0, 100.0, 104.0));
        let v = cycle_json("BTC", &bars).unwrap();
        assert_eq!(v["state"], "queda");
        let text: Vec<&str> = v["text"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap())
            .collect();
        assert!(
            text[0].starts_with("BTC está em queda: o último topo foi 130"),
            "{}",
            text[0]
        );
        assert!(text[1].contains("quedas de 15% ou mais"), "{}", text[1]);
        let kinds: Vec<&str> = v["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["kind"].as_str().unwrap())
            .collect();
        assert!(kinds.contains(&"fall"));
        assert!(kinds.contains(&"top"));
        assert!(
            text.last()
                .unwrap()
                .contains("Não é sinal de compra ou venda")
        );
    }
}
