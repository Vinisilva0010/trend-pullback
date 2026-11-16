//! Risk lines on the stock signals and warnings for open trades: the
//! company's earnings date (Nasdaq calendar), the US economic agenda
//! (`config/events.csv`: Fed decisions, CPI, jobs report) and two trades
//! in the same sector at once. These are warnings, not filters: the tests
//! of the setups did not separate trades with earnings or Fed days.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Datelike, NaiveDate, Utc, Weekday};
use tp_data::earnings::{Earning, When};

use crate::stock_watch::Watched;
use crate::stormer_live::StageAlert;

/// Days ahead the earnings calendar is read.
pub const EARNINGS_DAYS: i64 = 14;
/// Days ahead the economic agenda goes into a signal.
const AGENDA_DAYS: i64 = 7;

/// Sector of the US stocks and ETFs of the lists, to see two trades that
/// tend to move together.
#[must_use]
pub fn sector(symbol: &str) -> Option<&'static str> {
    Some(match symbol {
        "SPY" | "QQQ" | "DIA" | "IWM" => "índices amplos",
        "NVDA" | "AMD" | "AVGO" | "INTC" | "QCOM" | "TXN" | "SMH" => "semicondutores",
        "AAPL" | "MSFT" | "ADBE" | "CRM" | "CSCO" | "IBM" | "ORCL" | "PLTR" | "SHOP" | "GOOGL"
        | "META" | "NFLX" | "XLK" | "QTUM" => "tecnologia e internet",
        "JPM" | "BAC" | "GS" | "MS" | "AXP" | "V" | "MA" | "BLK" | "BRK-B" | "IBKR" | "SOFI"
        | "XLF" => "financeiro",
        "LLY" | "UNH" | "JNJ" | "ABBV" | "MRK" | "PFE" | "TMO" | "ABT" | "AMGN" | "ISRG"
        | "XLV" => "saúde",
        "AMZN" | "TSLA" | "HD" | "MCD" | "NKE" | "SBUX" | "DIS" | "XLY" => "consumo",
        "KO" | "PEP" | "PG" | "WMT" | "COST" | "XLP" => "consumo básico",
        "XOM" | "CVX" | "XLE" => "energia",
        "CAT" | "DE" | "GE" | "HON" | "BA" | "LMT" | "UNP" | "UPS" | "RKLB" | "XLI" => "indústria",
        "GLD" | "SLV" | "NEM" | "AU" | "GFI" => "ouro, prata e mineradoras",
        "LIN" | "XLB" => "materiais",
        "NEE" | "XLU" | "T" | "VZ" => "utilidades e telecom",
        "EWZ" => "Brasil",
        _ => return None,
    })
}

/// What the risk lines need.
#[derive(Debug, Clone, Default)]
pub struct Risks {
    /// Next earnings date of each symbol, within [`EARNINGS_DAYS`].
    pub earnings: BTreeMap<String, (NaiveDate, When)>,
    /// Economic agenda (time, description).
    pub agenda: Vec<(DateTime<Utc>, String)>,
}

impl Risks {
    /// Keeps the first date of each symbol of `symbols`.
    #[must_use]
    pub fn with_earnings(mut self, rows: &[Earning], symbols: &BTreeSet<String>) -> Self {
        for r in rows.iter().filter(|r| symbols.contains(&r.symbol)) {
            self.earnings
                .entry(r.symbol.clone())
                .and_modify(|e| {
                    if r.date < e.0 {
                        *e = (r.date, r.when);
                    }
                })
                .or_insert((r.date, r.when));
        }
        self
    }

    /// The risk line of a new signal on `symbol`, with the symbols of the
    /// trades open now (`open`).
    #[must_use]
    pub fn note(&self, symbol: &str, open: &[(String, String)], now_ms: i64) -> Option<String> {
        let mut parts = Vec::new();
        let today = brasilia_day(now_ms);
        if let Some((date, when)) = self.earnings.get(symbol)
            && *date >= today
        {
            parts.push(format!(
                "BALANÇO DA EMPRESA em {} ({}). No balanço o preço costuma abrir com salto e pode passar direto do stop, saindo pior que o preço marcado; muita gente não entra ou entra com posição menor antes dele.",
                date.format("%d/%m"),
                when.pt()
            ));
        }
        let now = DateTime::from_timestamp_millis(now_ms).unwrap_or_default();
        let soon: Vec<String> = self
            .agenda
            .iter()
            .filter(|(t, _)| *t > now && *t - now <= chrono::Duration::days(AGENDA_DAYS))
            .map(|(t, what)| {
                format!(
                    "{what} em {}",
                    (*t - chrono::Duration::hours(3)).format("%d/%m às %Hh%M")
                )
            })
            .collect();
        if !soon.is_empty() {
            parts.push(format!(
                "Agenda dos próximos 7 dias: {}. Nesses dias o preço costuma mexer forte.",
                soon.join("; ")
            ));
        }
        if let Some(s) = sector(symbol) {
            let same: Vec<&str> = open
                .iter()
                .filter(|(other, other_sector)| other != symbol && other_sector == s)
                .map(|(other, _)| other.as_str())
                .collect();
            if !same.is_empty() {
                parts.push(format!(
                    "Já há trade aberto no mesmo setor ({s}: {}). Ações do mesmo setor costumam cair juntas; dois trades assim somam o risco.",
                    same.join(", ")
                ));
            }
        }
        (!parts.is_empty()).then(|| parts.join("\n"))
    }
}

/// The date in Brasília (UTC-3).
#[must_use]
pub fn brasilia_day(now_ms: i64) -> NaiveDate {
    DateTime::from_timestamp_millis(now_ms - 3 * 3_600_000)
        .unwrap_or_default()
        .date_naive()
}

/// The weekdays from `from` on, `days` calendar days ahead.
#[must_use]
pub fn weekdays(from: NaiveDate, days: i64) -> Vec<NaiveDate> {
    (0..days)
        .map(|d| from + chrono::Duration::days(d))
        .filter(|d| !matches!(d.weekday(), Weekday::Sat | Weekday::Sun))
        .collect()
}

/// Open trades and their sectors, from the intraday watch list.
#[must_use]
pub fn open_trades(watch: &[Watched]) -> Vec<(String, String)> {
    watch
        .iter()
        .filter(|w| w.status == "aberto")
        .filter_map(|w| Some((w.symbol.clone(), sector(&w.symbol)?.to_owned())))
        .collect()
}

/// Warnings for open trades and waiting signals whose company reports in
/// the next 3 days, once per report.
#[must_use]
pub fn earnings_alerts(
    watch: &[Watched],
    risks: &Risks,
    now_ms: i64,
    sent: &BTreeSet<String>,
) -> Vec<(String, StageAlert)> {
    let today = brasilia_day(now_ms);
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for w in watch {
        let Some((date, when)) = risks.earnings.get(&w.symbol) else {
            continue;
        };
        if *date < today || (*date - today).num_days() > 3 {
            continue;
        }
        let key = format!("{}|balanco|{date}", w.symbol);
        if sent.contains(&key) || !seen.insert(key.clone()) {
            continue;
        }
        let what = if w.status == "aberto" {
            format!(
                "Você tem trade aberto nela (stop {}, alvo {}).",
                super::fmt_pt(w.stop),
                super::fmt_pt(w.target)
            )
        } else {
            format!(
                "Há um sinal esperando o gatilho em {}.",
                super::fmt_pt(w.trigger)
            )
        };
        let day = if *date == today {
            "HOJE".to_owned()
        } else {
            date.format("%d/%m").to_string()
        };
        out.push((
            w.name.clone(),
            StageAlert {
                key,
                message: format!(
                    "BALANÇO {} ({}) {day}, {}. {what}\nNo balanço o preço costuma abrir com salto e pode passar direto do stop, saindo pior que o marcado. Quem não quer esse risco reduz a posição ou sai antes; quem fica, aceita que o stop pode não segurar.",
                    w.name,
                    w.symbol,
                    when.pt()
                ),
                notify: true,
            },
        ));
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn watched(symbol: &str, status: &str) -> Watched {
        Watched {
            key: format!("{symbol}|1d|trap|compra|1"),
            symbol: symbol.to_owned(),
            name: symbol.to_owned(),
            tf: "1d".to_owned(),
            status: status.to_owned(),
            trigger: 100.0,
            stop: 95.0,
            target: 110.0,
            from_ms: 0,
            until_ms: None,
            in_book: true,
        }
    }

    fn ms(y: i32, m: u32, d: u32, h: u32) -> i64 {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis()
    }

    #[test]
    fn a_signal_gets_earnings_agenda_and_sector_lines() {
        let day = |d| NaiveDate::from_ymd_opt(2026, 11, d).unwrap();
        let symbols: BTreeSet<String> = ["NVDA".to_owned()].into();
        let rows = vec![
            Earning {
                symbol: "NVDA".to_owned(),
                date: day(19),
                when: When::AfterClose,
            },
            Earning {
                symbol: "NVDA".to_owned(),
                date: day(18),
                when: When::AfterClose,
            },
            Earning {
                symbol: "KO".to_owned(),
                date: day(18),
                when: When::BeforeOpen,
            },
        ];
        let risks = Risks {
            agenda: vec![(
                DateTime::from_timestamp_millis(ms(2026, 11, 20, 13)).unwrap(),
                "Inflação dos EUA (CPI)".to_owned(),
            )],
            ..Default::default()
        }
        .with_earnings(&rows, &symbols);
        assert_eq!(risks.earnings.len(), 1);
        let open = open_trades(&[watched("AMD", "aberto"), watched("KO", "sinal")]);
        let note = risks.note("NVDA", &open, ms(2026, 11, 16, 23)).unwrap();
        assert!(
            note.contains("BALANÇO DA EMPRESA em 18/11 (depois do fechamento)"),
            "{note}"
        );
        assert!(
            note.contains("Inflação dos EUA (CPI) em 20/11 às 10h00"),
            "{note}"
        );
        assert!(note.contains("mesmo setor (semicondutores: AMD)"), "{note}");
        assert!(
            risks
                .note("KO", &[], ms(2026, 11, 16, 23))
                .unwrap()
                .starts_with("Agenda")
        );
    }

    #[test]
    fn open_trades_get_one_warning_before_the_report() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        let risks = Risks {
            earnings: [("NVDA".to_owned(), (day, When::AfterClose))].into(),
            ..Default::default()
        };
        let watch = [watched("NVDA", "aberto")];
        let a = earnings_alerts(&watch, &risks, ms(2026, 11, 16, 23), &BTreeSet::new());
        assert_eq!(a.len(), 1);
        assert!(
            a[0].1
                .message
                .starts_with("BALANÇO NVDA (NVDA) 18/11, depois do fechamento."),
            "{}",
            a[0].1.message
        );
        let sent: BTreeSet<String> = [a[0].1.key.clone()].into();
        assert!(earnings_alerts(&watch, &risks, ms(2026, 11, 17, 23), &sent).is_empty());
        // A week before: not yet.
        assert!(earnings_alerts(&watch, &risks, ms(2026, 11, 10, 23), &BTreeSet::new()).is_empty());
        assert_eq!(
            weekdays(NaiveDate::from_ymd_opt(2026, 11, 14).unwrap(), 3).len(),
            1
        );
    }
}
