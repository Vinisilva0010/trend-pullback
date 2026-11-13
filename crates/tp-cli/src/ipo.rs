//! IPOs: the radar (what is coming, what just listed, with alerts for the
//! large ones) and `tp ipo-study`, which tests ways of buying a new listing
//! on the large IPOs of the last years before any "how to enter" advice.
//!
//! A buyer without an allocation (any retail broker account that only
//! trades after the listing) pays the opening price, not the IPO price, so
//! every rule here starts at the first day's open or later.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use chrono::{Datelike, NaiveDate};
use serde_json::{Value, json};
use tp_core::intraday::Bar;
use tp_data::ipo::{Calendar, Priced, Upcoming, fetch_calendar};
use tp_data::yahoo::{Interval, fetch_chart};

use super::{CliResult, count_f64, fmt_pt, held_out_start, pct_pt, r_mean_t, r_pt};
use crate::markets::{Item, Loaded};
use crate::smc_live::StageAlert;

/// Sent IPO alert keys, one per line.
pub const SENT_FILE: &str = "ipo_sent.txt";
/// Market group of the recent large IPOs on the Mercados tab.
pub const GROUP: &str = "IPO";
/// Offer size from which an IPO gets alerts.
const ALERT_AMOUNT: f64 = 1e9;
/// Offer size from which a recent IPO is tracked on the dashboard.
const TRACK_AMOUNT: f64 = 5e8;
/// Offer size from which an upcoming IPO is listed.
const LIST_AMOUNT: f64 = 1e8;
/// Days a priced IPO stays tracked.
const TRACK_DAYS: i64 = 90;
const DAY_MS: i64 = 24 * 3600 * 1000;
const COST: f64 = 0.002;

/// What the history of IPOs says, shown with every alert until a tested
/// rule replaces it.
const HISTORY: &str = "O que o histórico mostra: em 45 anos de IPOs nos EUA, a alta média do preço do IPO até o fechamento do primeiro dia foi de 19% (10% nas empresas grandes), e ela fica com quem recebeu ações antes da abertura. Quem comprou no fechamento do primeiro dia e segurou 3 anos rendeu em média 21% menos que o mercado (4% menos nas grandes). Pela corretora só dá para comprar depois que abre.";

/// What `tp ipo-study` found (run of 12 Nov 2026), shown with the alerts
/// of the days around a large listing.
const TESTED: &str = "O que o teste do painel mostrou (85 IPOs de $1 bilhão ou mais desde 2019): comprar no fechamento do primeiro dia e vender 5 pregões depois rendeu em média 3,6% acima do S&P 500 e deu certo em 59% dos casos, positivo nos dois períodos do teste. Foi a única forma que passou. Segurar 20 ou 60 pregões não teve prova, comprar no rompimento da máxima do primeiro dia perdeu no período recente e comprar na volta ao preço do IPO perdeu. Limites: quase metade dos IPOs da lista ficou de fora por falta de preço na fonte, foram testadas 9 formas (uma pode passar por sorte) e a forma que passou não tem stop: o pior caso perdeu 32,5% em 5 pregões e 1 em cada 10 perdeu mais de 10,6%. Por isso, se for operar, posição pequena.";

fn date_pt(d: NaiveDate) -> String {
    format!("{:02}/{:02}/{}", d.day(), d.month(), d.year())
}

fn date_ms(d: NaiveDate) -> i64 {
    d.and_hms_opt(0, 0, 0)
        .map_or(0, |t| t.and_utc().timestamp_millis())
}

fn billions(v: f64) -> String {
    if v >= 1e9 {
        format!("${} bilhões", format!("{:.1}", v / 1e9).replace('.', ","))
    } else {
        format!("${:.0} milhões", v / 1e6)
    }
}

/// Blank-check companies list at $10 and hold cash: not what this follows.
fn is_spac(company: &str) -> bool {
    let c = company.to_lowercase();
    c.contains("acquisition") || c.contains("blank check") || c.contains("spac")
}

/// This month, the next one and the previous one, merged.
pub async fn fetch_around(today: NaiveDate) -> Calendar {
    let mut out = Calendar::default();
    let first = today.with_day(1).unwrap_or(today);
    let months = [
        first.pred_opt().unwrap_or(first),
        first,
        first + chrono::Days::new(32),
    ];
    for m in months {
        match fetch_calendar(m.year(), m.month()).await {
            Ok(c) => {
                out.upcoming.extend(c.upcoming);
                out.priced.extend(c.priced);
            }
            Err(e) => eprintln!("warning: IPO calendar {}-{:02}: {e}", m.year(), m.month()),
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    // A month's answer can repeat a deal of the next one.
    let mut seen = BTreeSet::new();
    out.upcoming.retain(|u| seen.insert(u.symbol.clone()));
    let mut seen = BTreeSet::new();
    out.priced.retain(|p| seen.insert(p.symbol.clone()));
    out
}

/// Recent large IPOs to load as market assets (group [`GROUP`]).
#[must_use]
pub fn tracked(calendar: &Calendar, today: NaiveDate) -> Vec<(Item, Priced)> {
    calendar
        .priced
        .iter()
        .filter(|p| {
            p.amount >= TRACK_AMOUNT
                && !is_spac(&p.company)
                && (today - p.date).num_days() <= TRACK_DAYS
                && p.date <= today
        })
        .map(|p| {
            (
                Item {
                    group: GROUP.to_owned(),
                    symbol: p.symbol.clone(),
                    name: p.company.chars().take(40).collect(),
                },
                p.clone(),
            )
        })
        .collect()
}

/// The first trading day of a priced IPO: the first candle on or after the
/// pricing date, at most a week later.
fn first_day<'a>(p: &Priced, daily: &'a [Bar]) -> Option<(usize, &'a Bar)> {
    let from = date_ms(p.date);
    daily
        .iter()
        .enumerate()
        .find(|(_, b)| b.open_time_ms >= from)
        .filter(|(_, b)| b.open_time_ms < from + 7 * DAY_MS)
}

/// The IPO section of the Mercados data.
#[must_use]
pub fn ipos_json(calendar: &Calendar, loaded: &[Loaded], today: NaiveDate) -> Value {
    let mut upcoming: Vec<&Upcoming> = calendar
        .upcoming
        .iter()
        .filter(|u| u.amount >= LIST_AMOUNT && !is_spac(&u.company) && u.date >= today)
        .collect();
    upcoming.sort_by_key(|u| u.date);
    let recent: Vec<Value> = tracked(calendar, today)
        .iter()
        .map(|(item, p)| {
            let bars = loaded
                .iter()
                .find(|l| l.item.symbol == item.symbol)
                .map(|l| l.daily.as_slice())
                .unwrap_or_default();
            let first = first_day(p, bars);
            let last = bars.last().map(|b| b.close);
            json!({
                "symbol": p.symbol,
                "company": p.company,
                "exchange": p.exchange,
                "price": p.price,
                "amount": p.amount,
                "date_ms": date_ms(p.date),
                "first": first.map(|(_, b)| json!([b.open, b.high, b.low, b.close])),
                "days": first.map(|(i, _)| bars.len() - i),
                "last": last,
                "from_ipo": last.map(|l| (l / p.price - 1.0) * 100.0),
            })
        })
        .collect();
    json!({
        "upcoming": upcoming.iter().map(|u| json!({
            "symbol": u.symbol,
            "company": u.company,
            "exchange": u.exchange,
            "price_low": u.price_low,
            "price_high": u.price_high,
            "shares": u.shares,
            "amount": u.amount,
            "date_ms": date_ms(u.date),
            "big": u.amount >= ALERT_AMOUNT,
        })).collect::<Vec<_>>(),
        "recent": recent,
        "history": HISTORY,
    })
}

fn range_pt(u: &Upcoming) -> String {
    if (u.price_high - u.price_low).abs() < f64::EPSILON {
        format!("${}", fmt_pt(u.price_low))
    } else {
        format!("${} a ${}", fmt_pt(u.price_low), fmt_pt(u.price_high))
    }
}

/// New alerts for the large IPOs: on the radar, date or range changed,
/// three days and one day before, priced, and the first day's result.
#[must_use]
#[allow(clippy::too_many_lines)] // one message per stage, in the order they happen
pub fn alerts(
    calendar: &Calendar,
    loaded: &[Loaded],
    today: NaiveDate,
    sent: &BTreeSet<String>,
) -> Vec<(String, StageAlert)> {
    let mut out = Vec::new();
    let mut push = |name: &str, key: String, message: String| {
        if !sent.contains(&key) {
            out.push((
                name.chars().take(40).collect(),
                StageAlert {
                    key,
                    message,
                    notify: true,
                },
            ));
        }
    };
    for u in calendar
        .upcoming
        .iter()
        .filter(|u| u.amount >= ALERT_AMOUNT && !is_spac(&u.company) && u.date >= today)
    {
        let name = u.company.as_str();
        let days = (u.date - today).num_days();
        let facts = format!(
            "{name} ({}): estreia prevista para {} na {}. Faixa de preço {}, oferta de até {}.",
            u.symbol,
            date_pt(u.date),
            u.exchange,
            range_pt(u),
            billions(u.amount)
        );
        let radar = format!("{}|radar", u.symbol);
        if sent.contains(&radar) {
            push(
                name,
                format!("{}|data|{}", u.symbol, u.date),
                format!("IPO: a data mudou. {facts}"),
            );
            push(
                name,
                format!("{}|faixa|{}-{}", u.symbol, u.price_low, u.price_high),
                format!("IPO: a faixa de preço mudou. {facts}"),
            );
        } else {
            push(
                name,
                radar,
                format!("IPO GRANDE NO RADAR. {facts}\n{HISTORY}"),
            );
            // The first sight already carries the date and the range.
            push(name, format!("{}|data|{}", u.symbol, u.date), String::new());
            push(
                name,
                format!("{}|faixa|{}-{}", u.symbol, u.price_low, u.price_high),
                String::new(),
            );
        }
        if (2..=3).contains(&days) {
            push(
                name,
                format!("{}|d3|{}", u.symbol, u.date),
                format!("IPO em {days} dias. {facts}\n{HISTORY}\n{TESTED}"),
            );
        }
        if days <= 1 {
            push(
                name,
                format!("{}|d1|{}", u.symbol, u.date),
                format!(
                    "IPO {}. {facts}\nO preço final sai na véspera, à noite; a negociação costuma abrir horas depois do pregão começar. No primeiro dia o painel mostra o preço ao vivo na aba Mercados, grupo IPO, assim que o código aparecer na fonte.\n{HISTORY}\n{TESTED}",
                    if days == 1 { "amanhã" } else { "hoje" }
                ),
            );
        }
    }
    for (item, p) in tracked(calendar, today)
        .iter()
        .filter(|(_, p)| p.amount >= ALERT_AMOUNT)
    {
        let name = p.company.as_str();
        push(
            name,
            format!("{}|preco", p.symbol),
            format!(
                "IPO com preço definido: {name} ({}) saiu a ${} por ação em {}, oferta de {}. Quem compra pela corretora paga o preço de abertura, não este.",
                p.symbol,
                fmt_pt(p.price),
                date_pt(p.date),
                billions(p.amount)
            ),
        );
        let bars = loaded
            .iter()
            .find(|l| l.item.symbol == item.symbol)
            .map(|l| l.daily.as_slice())
            .unwrap_or_default();
        if let Some((_, b)) = first_day(p, bars) {
            push(
                name,
                format!("{}|estreia", p.symbol),
                format!(
                    "IPO, primeiro dia de {name} ({}): preço do IPO ${}, abriu a {} ({} sobre o IPO), máxima {}, mínima {}, fechou a {} ({} sobre o IPO e {} sobre a abertura).\nNíveis para acompanhar: máxima e mínima do primeiro dia, e o preço do IPO.\n{TESTED}\nPela forma que passou, a compra seria perto do fechamento do primeiro dia (por volta de {}) e a venda no fechamento do 5º pregão seguinte.",
                    p.symbol,
                    fmt_pt(p.price),
                    fmt_pt(b.open),
                    pct_pt((b.open / p.price - 1.0) * 100.0),
                    fmt_pt(b.high),
                    fmt_pt(b.low),
                    fmt_pt(b.close),
                    pct_pt((b.close / p.price - 1.0) * 100.0),
                    pct_pt((b.close / b.open - 1.0) * 100.0),
                    fmt_pt(b.close),
                ),
            );
        }
    }
    out
}

// ---------------------------------------------------------------- study

/// One past IPO with its candles from the first trading day on.
struct Past {
    /// The first open is between half and 3.5 times the IPO price. Outside
    /// that, the source's prices were adjusted by a later split and cannot
    /// be compared with the IPO price.
    comparable: bool,
    price: f64,
    amount: f64,
    bars: Vec<Bar>,
}

/// (first day time, value) rows of one rule.
type Rows = Vec<(i64, f64)>;

/// Stop first, then target; closes at the close after `hold` candles.
/// Returns the result in R with costs.
fn follow(bars: &[Bar], k: usize, entry: f64, stop: f64, target: f64, hold: usize) -> Option<f64> {
    let risk = entry - stop;
    if risk <= 0.0 || k >= bars.len() {
        return None;
    }
    let last = (k + hold).min(bars.len() - 1);
    let mut exit = bars[last].close;
    for (j, b) in bars.iter().enumerate().take(last + 1).skip(k) {
        if b.low <= stop {
            exit = if j > k { stop.min(b.open) } else { stop };
            break;
        }
        // No target on the entry candle: the order of its prices is unknown.
        if j > k && b.high >= target {
            exit = target.max(b.open);
            break;
        }
    }
    Some((exit - entry) / risk - COST * entry / risk)
}

/// Close-to-close return in percent from candle `a` to `b`, with costs,
/// minus the S&P 500 over the same days.
fn excess(bars: &[Bar], from: usize, to: usize, spy: &BTreeMap<i64, f64>) -> Option<f64> {
    let (first, last) = (bars.get(from)?, bars.get(to)?);
    let own = (last.close / first.close - 1.0 - COST) * 100.0;
    let index = |bar: &Bar| spy.get(&bar.open_time_ms.div_euclid(DAY_MS)).copied();
    Some(own - (index(last)? / index(first)? - 1.0) * 100.0)
}

fn summary(label: &str, unit_r: bool, rows: &Rows, split_ms: i64, good: &mut Vec<String>) {
    if rows.is_empty() {
        println!("   {label}: nenhum caso.");
        return;
    }
    let show = |v: f64| if unit_r { r_pt(v) } else { pct_pt(v) };
    let all: Vec<f64> = rows.iter().map(|r| r.1).collect();
    let dev: Vec<f64> = rows
        .iter()
        .filter(|r| r.0 <= split_ms)
        .map(|r| r.1)
        .collect();
    let fin: Vec<f64> = rows
        .iter()
        .filter(|r| r.0 > split_ms)
        .map(|r| r.1)
        .collect();
    let (mean, t) = r_mean_t(&all);
    let (dev_mean, _) = r_mean_t(&dev);
    let (fin_mean, _) = r_mean_t(&fin);
    let mut sorted = all.clone();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    let wins = all.iter().filter(|v| **v > 0.0).count();
    let worst = sorted[0];
    let tenth = sorted[sorted.len() / 10];
    let both = dev.len() >= 5 && fin.len() >= 5 && dev_mean > 0.0 && fin_mean > 0.0;
    let verdict = if both && all.len() >= 30 && t >= 2.0 {
        good.push(label.to_owned());
        "GANHOU NOS DOIS PERÍODOS, COM PROVA"
    } else if both {
        "positivo nos dois períodos, mas sem prova (pode ser sorte)"
    } else if mean > 0.0 {
        "positivo na média, mas não nos dois períodos"
    } else {
        "PERDEU"
    };
    println!(
        "   {label}: {} casos. Média {}, caso típico {}, deram certo {:.0}%. Pior caso {}; 1 em cada 10 foi pior que {}. Até out/2024: {} ({} casos); depois: {} ({}). {verdict}.",
        all.len(),
        show(mean),
        show(median),
        count_f64(wins) / count_f64(all.len()) * 100.0,
        show(worst),
        show(tenth),
        show(dev_mean),
        dev.len(),
        show(fin_mean),
        fin.len(),
    );
}

/// Every rule over a set of past IPOs.
#[allow(clippy::too_many_lines)] // one block per rule, in the order of the report
fn study(
    title: &str,
    ipos: &[&Past],
    spy: &BTreeMap<i64, f64>,
    split_ms: i64,
    good: &mut Vec<String>,
) {
    println!();
    println!("{title} ({} IPOs)", ipos.len());
    let rows = |f: &dyn Fn(&Past) -> Option<f64>| -> Rows {
        ipos.iter()
            .filter_map(|p| Some((p.bars.first()?.open_time_ms, f(p)?)))
            .collect()
    };
    println!(" O que quem recebeu ações no IPO ganhou (não dá para ter pela corretora):");
    summary(
        "do preço do IPO até a abertura do 1º dia",
        false,
        &rows(&|p| {
            p.comparable
                .then_some((p.bars.first()?.open / p.price - 1.0) * 100.0)
        }),
        split_ms,
        &mut Vec::new(),
    );
    summary(
        "do preço do IPO até o fechamento do 1º dia",
        false,
        &rows(&|p| {
            p.comparable
                .then_some((p.bars.first()?.close / p.price - 1.0) * 100.0)
        }),
        split_ms,
        &mut Vec::new(),
    );
    println!(
        " Comprando depois que abre (retorno já com custos; os de vários dias são acima ou abaixo do S&P 500 no mesmo período):"
    );
    summary(
        &format!("{title}: comprar na abertura do 1º dia e vender no fechamento do mesmo dia"),
        false,
        &rows(&|p| {
            let b = p.bars.first()?;
            Some((b.close / b.open - 1.0 - COST) * 100.0)
        }),
        split_ms,
        good,
    );
    for hold in [5, 20, 60] {
        summary(
            &format!("{title}: comprar no fechamento do 1º dia e vender {hold} pregões depois"),
            false,
            &rows(&|p| excess(&p.bars, 0, hold, spy)),
            split_ms,
            good,
        );
    }
    summary(
        &format!(
            "{title}: só os que subiram 15% ou mais no 1º dia, comprar no fechamento e vender 20 pregões depois"
        ),
        false,
        &rows(&|p| {
            let b = p.bars.first()?;
            (p.comparable && b.close / p.price >= 1.15)
                .then(|| excess(&p.bars, 0, 20, spy))
                .flatten()
        }),
        split_ms,
        good,
    );
    println!(
        " Entradas com stop (resultado em R: 1R é o que se perde no stop; alvo de 2R; stop primeiro quando o candle toca os dois):"
    );
    summary(
        &format!(
            "{title}: comprar quando passar da máxima do 1º dia (nos 10 pregões seguintes), stop na mínima do 1º dia, até 20 pregões"
        ),
        true,
        &rows(&|p| {
            let d0 = p.bars.first()?;
            let k =
                (1..=10.min(p.bars.len().saturating_sub(1))).find(|i| p.bars[*i].high > d0.high)?;
            let entry = d0.high.max(p.bars[k].open);
            follow(
                &p.bars,
                k,
                entry,
                d0.low,
                entry + 2.0 * (entry - d0.low),
                20,
            )
        }),
        split_ms,
        good,
    );
    summary(
        &format!(
            "{title}: comprar quando passar da máxima da 1ª semana (até o 25º pregão), stop na mínima da 1ª semana, até 40 pregões"
        ),
        true,
        &rows(&|p| {
            let week = p.bars.get(..5)?;
            let high = week.iter().map(|b| b.high).fold(f64::MIN, f64::max);
            let low = week.iter().map(|b| b.low).fold(f64::MAX, f64::min);
            let k =
                (5..=25.min(p.bars.len().saturating_sub(1))).find(|i| p.bars[*i].high > high)?;
            let entry = high.max(p.bars[k].open);
            follow(&p.bars, k, entry, low, entry + 2.0 * (entry - low), 40)
        }),
        split_ms,
        good,
    );
    summary(
        &format!(
            "{title}: comprar quando voltar ao preço do IPO (nos 60 pregões seguintes), stop 10% abaixo, alvo 20% acima, até 40 pregões"
        ),
        true,
        &rows(&|p| {
            let k = (1..=60.min(p.bars.len().saturating_sub(1)))
                .find(|i| p.comparable && p.bars[*i].low <= p.price)?;
            // Only when it traded above the IPO price before coming back.
            p.bars[..k]
                .iter()
                .any(|b| b.close > p.price)
                .then_some(())?;
            let entry = p.price.min(p.bars[k].open);
            follow(&p.bars, k, entry, entry * 0.9, entry * 1.2, 40)
        }),
        split_ms,
        good,
    );
}

/// Runs the study on the priced IPOs of $300 million or more since 2019.
///
/// # Errors
///
/// Returns an error when the calendar gives nothing or the S&P 500 history
/// cannot be fetched.
#[allow(clippy::too_many_lines)] // the list, the prices and the two groups, in order
pub async fn cmd_ipo_study(_data_dir: &Path) -> CliResult {
    let now = crate::markets::now_ms()?;
    let today = chrono::DateTime::from_timestamp_millis(now)
        .ok_or("invalid clock")?
        .date_naive();
    let split_ms = held_out_start()?
        .and_hms_opt(23, 59, 59)
        .ok_or("invalid date")?
        .and_utc()
        .timestamp_millis();
    println!(
        "IPOs dos EUA desde 2019 com oferta de $300 milhões ou mais (sem empresas de aquisição). Lista: calendário da Nasdaq; preços: Yahoo Finance."
    );
    println!(
        "Atenção: empresas que saíram da bolsa ou trocaram de código não têm preço na fonte e ficam de fora; como muitas dessas foram mal, os resultados de segurar por mais tempo ficam melhores do que foram de verdade."
    );
    let mut priced: Vec<Priced> = Vec::new();
    let mut month = NaiveDate::from_ymd_opt(2019, 1, 1).ok_or("invalid date")?;
    while month <= today {
        match fetch_calendar(month.year(), month.month()).await {
            Ok(c) => priced.extend(
                c.priced
                    .into_iter()
                    .filter(|p| p.amount >= 3e8 && !is_spac(&p.company)),
            ),
            Err(e) => eprintln!("warning: {}-{:02}: {e}", month.year(), month.month()),
        }
        month = (month + chrono::Days::new(32))
            .with_day(1)
            .ok_or("invalid date")?;
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    let mut seen = BTreeSet::new();
    priced.retain(|p| seen.insert((p.symbol.clone(), p.date)));
    if priced.is_empty() {
        return Err("the IPO calendar gave no priced IPOs".into());
    }
    println!(
        "{} IPOs na lista; buscando os preços (alguns minutos)...",
        priced.len()
    );
    let spy: BTreeMap<i64, f64> = fetch_chart("SPY", Interval::Day, now)
        .await?
        .iter()
        .map(|b| (b.open_time_ms.div_euclid(DAY_MS), b.close))
        .collect();
    let mut past = Vec::new();
    let mut missing = 0;
    for (n, p) in priced.iter().enumerate() {
        if n > 0 && n % 50 == 0 {
            println!("  {n} de {}...", priced.len());
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let Ok(daily) = fetch_chart(&p.symbol, Interval::Day, now).await else {
            missing += 1;
            continue;
        };
        // The history has to start at the listing: an older first candle
        // is another company that used the same code.
        match first_day(p, &daily) {
            Some((0, first)) => past.push(Past {
                comparable: (0.5..=3.5).contains(&(first.open / p.price)),
                price: p.price,
                amount: p.amount,
                bars: daily,
            }),
            _ => missing += 1,
        }
    }
    println!(
        "{} IPOs com preços desde o primeiro dia; {missing} sem dados na fonte.",
        past.len()
    );
    println!(
        "{} deles com preço ajustado por desdobramento depois do IPO: ficam fora das contas que usam o preço do IPO.",
        past.iter().filter(|p| !p.comparable).count()
    );
    let mut good = Vec::new();
    let all: Vec<&Past> = past.iter().collect();
    study(
        "Todos (oferta de $300 milhões ou mais)",
        &all,
        &spy,
        split_ms,
        &mut good,
    );
    let big: Vec<&Past> = past.iter().filter(|p| p.amount >= 1e9).collect();
    study(
        "Grandes (oferta de $1 bilhão ou mais)",
        &big,
        &spy,
        split_ms,
        &mut good,
    );
    println!();
    println!("RESUMO: o que ganhou nos dois períodos, com prova");
    if good.is_empty() {
        println!(" Nada.");
    }
    for g in good {
        println!(" {g}");
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// Text block of the upcoming IPOs for a dry run.
#[must_use]
pub fn upcoming_text(calendar: &Calendar, today: NaiveDate) -> String {
    let mut text = String::new();
    for u in calendar
        .upcoming
        .iter()
        .filter(|u| u.amount >= LIST_AMOUNT && u.date >= today)
    {
        let _ = writeln!(
            text,
            "IPO previsto: {} ({}) em {}, {}, oferta de {}",
            u.company,
            u.symbol,
            date_pt(u.date),
            range_pt(u),
            billions(u.amount)
        );
    }
    text
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn upcoming(date: NaiveDate) -> Upcoming {
        Upcoming {
            symbol: "ANT".to_owned(),
            company: "ANTHROPIC, PBC".to_owned(),
            exchange: "NASDAQ Global Select".to_owned(),
            price_low: 70.0,
            price_high: 78.0,
            shares: 825e6,
            date,
            amount: 74e9,
        }
    }

    fn bar(d: NaiveDate, open: f64, high: f64, low: f64, close: f64) -> Bar {
        Bar {
            open_time_ms: date_ms(d) + 14 * 3_600_000,
            open,
            high,
            low,
            close,
            volume: 1.0,
        }
    }

    #[test]
    fn a_large_ipo_alerts_once_then_on_changes_and_before_the_day() {
        let cal = Calendar {
            upcoming: vec![upcoming(day(2026, 11, 19))],
            priced: Vec::new(),
        };
        let first = alerts(&cal, &[], day(2026, 11, 12), &BTreeSet::new());
        let texts: Vec<&StageAlert> = first
            .iter()
            .map(|(_, a)| a)
            .filter(|a| !a.message.is_empty())
            .collect();
        assert_eq!(texts.len(), 1, "{first:?}");
        assert!(texts[0].message.starts_with(
            "IPO GRANDE NO RADAR. ANTHROPIC, PBC (ANT): estreia prevista para 19/11/2026"
        ));
        assert!(
            texts[0].message.contains("$70,00 a $78,00"),
            "{}",
            texts[0].message
        );
        assert!(
            texts[0].message.contains("até $74,0 bilhões"),
            "{}",
            texts[0].message
        );
        let sent: BTreeSet<String> = first.iter().map(|(_, a)| a.key.clone()).collect();
        // Nothing new the next day; a reminder three days before; a new date alerts.
        assert!(alerts(&cal, &[], day(2026, 11, 13), &sent).is_empty());
        let d3 = alerts(&cal, &[], day(2026, 11, 16), &sent);
        assert!(
            d3[0].1.message.starts_with("IPO em 3 dias."),
            "{}",
            d3[0].1.message
        );
        let moved = Calendar {
            upcoming: vec![upcoming(day(2026, 11, 20))],
            priced: Vec::new(),
        };
        let m = alerts(&moved, &[], day(2026, 11, 13), &sent);
        assert!(
            m[0].1.message.starts_with("IPO: a data mudou."),
            "{}",
            m[0].1.message
        );
    }

    #[test]
    fn the_first_day_is_reported_with_its_levels() {
        let p = Priced {
            symbol: "ANT".to_owned(),
            company: "ANTHROPIC, PBC".to_owned(),
            exchange: "NASDAQ".to_owned(),
            price: 78.0,
            shares: 825e6,
            date: day(2026, 11, 18),
            amount: 64e9,
        };
        let cal = Calendar {
            upcoming: Vec::new(),
            priced: vec![p],
        };
        let loaded = vec![Loaded {
            item: Item {
                group: GROUP.to_owned(),
                symbol: "ANT".to_owned(),
                name: "ANTHROPIC, PBC".to_owned(),
            },
            daily: vec![bar(day(2026, 11, 19), 95.0, 101.0, 90.0, 93.6)],
            weekly: Vec::new(),
        }];
        let a = alerts(&cal, &loaded, day(2026, 11, 19), &BTreeSet::new());
        let debut = a.iter().find(|(_, s)| s.key == "ANT|estreia").unwrap();
        assert!(
            debut
                .1
                .message
                .contains("abriu a 95,00 (+21,8% sobre o IPO)"),
            "{}",
            debut.1.message
        );
        assert!(
            debut
                .1
                .message
                .contains("fechou a 93,60 (+20,0% sobre o IPO e -1,5% sobre a abertura)"),
            "{}",
            debut.1.message
        );
        let v = ipos_json(&cal, &loaded, day(2026, 11, 19));
        assert_eq!(v["recent"][0]["first"], json!([95.0, 101.0, 90.0, 93.6]));
        assert!((v["recent"][0]["from_ipo"].as_f64().unwrap() - 20.0).abs() < 1e-9);
    }

    #[test]
    fn spacs_and_small_or_old_ipos_are_not_tracked() {
        let make = |company: &str, amount: f64, date: NaiveDate| Priced {
            symbol: company.chars().take(3).collect(),
            company: company.to_owned(),
            exchange: "NYSE".to_owned(),
            price: 10.0,
            shares: 1.0,
            date,
            amount,
        };
        let cal = Calendar {
            upcoming: Vec::new(),
            priced: vec![
                make("Big Acquisition Corp", 2e9, day(2026, 11, 1)),
                make("Small Co", 1e8, day(2026, 11, 1)),
                make("Old Co", 2e9, day(2026, 6, 1)),
                make("New Big Co", 2e9, day(2026, 11, 1)),
            ],
        };
        let t = tracked(&cal, day(2026, 11, 12));
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].1.company, "New Big Co");
    }

    #[test]
    fn the_stop_comes_first_and_the_target_waits_a_candle() {
        let d = day(2026, 1, 5);
        let bars = vec![
            bar(d, 100.0, 130.0, 99.0, 110.0),
            bar(d, 110.0, 125.0, 108.0, 120.0),
        ];
        // Entry candle reaches the target: ignored; the next one takes it.
        let r = follow(&bars, 0, 100.0, 95.0, 110.0, 5).unwrap();
        assert!((r - (2.0 - COST * 100.0 / 5.0)).abs() < 1e-9, "{r}");
        // A candle that touches both counts as the stop.
        let both = vec![
            bar(d, 100.0, 101.0, 99.0, 100.0),
            bar(d, 100.0, 111.0, 94.0, 105.0),
        ];
        let r = follow(&both, 0, 100.0, 95.0, 110.0, 5).unwrap();
        assert!((r - (-1.0 - COST * 100.0 / 5.0)).abs() < 1e-9, "{r}");
    }
}
