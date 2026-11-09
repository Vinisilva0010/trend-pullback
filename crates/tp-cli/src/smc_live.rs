//! Live Smart Money / Wyckoff setups: a Telegram message at each stage as a
//! setup forms (sweep, armed, filled, closed, cancelled) and three paper
//! books, one per timeframe, that take every setup.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use chrono::DateTime;
use tp_core::intraday::{Bar, HOUR_MS, aggregate};
use tp_core::smc::{
    Setup, Side, SmcParams, Status, Sweep, nearest_liquidity, scan, scan_with_pending, stats,
};
use tp_data::intraday::parse_bars_csv;

use super::{asset_name, brt_pt, count_f64, fmt_pt, money_pt, pct_pt, r_mean_t, r_pt, smc_frames};

/// Starting balance of each paper book.
pub const BOOK_CAPITAL: f64 = 2200.0;
/// Share of the book lost when a trade hits its stop.
pub const BOOK_RISK: f64 = 0.02;
/// Sent stage keys, one per line, never forgotten.
pub const SENT_FILE: &str = "smc_sent.txt";
/// Closed trades of the paper books.
pub const TRADES_FILE: &str = "smc_trades_v2.csv";
/// Start time of each paper book.
pub const BOOKS_FILE: &str = "smc_books_v2.csv";
/// Timeframes that have a paper book (the daily one had too few trades to test).
pub const BOOK_FRAMES: [(&str, &str); 3] = [
    ("st1d", "Stormer diário (trap)"),
    ("1h", "curto (1h)"),
    ("4h", "swing (4h)"),
];
const TRADES_HEADER: &str = "key,tf,asset,side,fill_ms,exit_ms,entry,stop,exit,r,status";

/// One timeframe scanned for one asset.
pub struct FrameScan {
    pub hours: i64,
    pub params: SmcParams,
    pub bars: Vec<Bar>,
    pub setups: Vec<Setup>,
    pub pending: Vec<Sweep>,
}

impl FrameScan {
    pub fn tf(&self) -> &'static str {
        tf_code(self.hours)
    }

    pub fn label(&self) -> &'static str {
        match self.hours {
            1 => "curto (1h)",
            4 => "swing (4h)",
            _ => "mês (diário)",
        }
    }

    fn candle_word(&self) -> &'static str {
        match self.hours {
            1 => "um candle de 1h",
            4 => "um candle de 4h",
            _ => "um candle diário",
        }
    }

    /// Close time of candle `i`, also for candles after the data ends.
    pub fn close_ms(&self, i: usize) -> i64 {
        let step = self.hours * HOUR_MS;
        let Some(last) = self.bars.len().checked_sub(1) else {
            return 0;
        };
        let (base, extra) = if i <= last {
            (self.bars[i].open_time_ms, 0)
        } else {
            (
                self.bars[last].open_time_ms,
                i64::try_from(i - last).unwrap_or(0),
            )
        };
        base + (extra + 1) * step
    }

    pub fn open_ms(&self, i: usize) -> i64 {
        self.bars.get(i).map_or(0, |b| b.open_time_ms)
    }

    /// True when candle `i` closed recently enough to be news.
    fn recent(&self, i: usize, now_ms: i64) -> bool {
        self.close_ms(i) >= now_ms - (self.hours * 2 + 4) * HOUR_MS
    }

    fn last_close(&self) -> Option<f64> {
        self.bars.last().map(|b| b.close)
    }
}

/// True for the only variant that was positive in the history study: a
/// sweep at the floor or ceiling of a sideways range (Wyckoff spring or
/// UTAD) on the 1h or 4h chart. Only these go to the paper books, Telegram
/// and phone notifications; every setup still goes to the dashboard.
#[must_use]
pub fn is_candidate(hours: i64, wyckoff_range: bool) -> bool {
    hours <= 4 && wyckoff_range
}

fn verdict_line(candidate: bool) -> &'static str {
    if candidate {
        "CANDIDATO: varrida na borda de uma lateral (spring/UTAD), o único tipo que ficou positivo no histórico (ainda sem prova)."
    } else {
        "SÓ PARA ACOMPANHAR: este tipo de setup perdeu dinheiro no histórico; a conta fictícia não entra."
    }
}

fn tf_code(hours: i64) -> &'static str {
    match hours {
        1 => "1h",
        4 => "4h",
        _ => "1d",
    }
}

fn side_code(side: Side) -> &'static str {
    match side {
        Side::Long => "long",
        Side::Short => "short",
    }
}

fn side_upper(side: Side) -> &'static str {
    match side {
        Side::Long => "COMPRA",
        Side::Short => "VENDA (short)",
    }
}

pub fn when(ms: i64) -> String {
    DateTime::from_timestamp_millis(ms).map_or_else(String::new, brt_pt)
}

/// Change from `from` to `to`, in percent.
pub fn change_pct(from: f64, to: f64) -> f64 {
    if from == 0.0 {
        0.0
    } else {
        (to / from - 1.0) * 100.0
    }
}

pub fn signed_money(v: f64) -> String {
    if v > 0.0 {
        format!("+{}", money_pt(v))
    } else {
        money_pt(v)
    }
}

pub fn ratio_pt(v: f64) -> String {
    format!("{v:.1}").replace('.', ",")
}

/// Scans the three timeframes: 1h and 4h from live hourly bars, daily from
/// the daily history.
#[must_use]
pub fn scan_frames(hourly: &[Bar], daily: &[Bar]) -> Vec<FrameScan> {
    smc_frames()
        .into_iter()
        .map(|f| {
            let bars = match f.hours {
                1 => hourly.to_vec(),
                24 => daily.to_vec(),
                h => aggregate(hourly, h),
            };
            let (setups, all_pending) = scan_with_pending(&bars, f.params);
            // Several sweeps of nearby lows in one move say the same thing:
            // keep only the latest one per side.
            let pending: Vec<Sweep> = [Side::Long, Side::Short]
                .into_iter()
                .filter_map(|side| {
                    all_pending
                        .iter()
                        .filter(|p| p.side == side)
                        .max_by_key(|p| p.sweep_index)
                        .cloned()
                })
                .collect();
            FrameScan {
                hours: f.hours,
                params: f.params,
                bars,
                setups,
                pending,
            }
        })
        .collect()
}

/// A closed trade of a paper book.
#[derive(Debug, Clone, PartialEq)]
pub struct TradeRow {
    pub key: String,
    pub tf: String,
    pub asset: String,
    pub side: String,
    pub fill_ms: i64,
    pub exit_ms: i64,
    pub entry: f64,
    pub stop: f64,
    pub exit: f64,
    pub r: f64,
    pub status: String,
}

impl TradeRow {
    /// Book growth factor: risk 2% of the book, but never a position larger
    /// than the book itself (no leverage beyond the balance).
    #[must_use]
    pub fn growth(&self) -> f64 {
        book_growth(self.entry, self.stop, self.r)
    }

    #[must_use]
    pub fn csv(&self) -> String {
        format!(
            "{},{},{},{},{},{},{},{},{},{:.4},{}",
            self.key,
            self.tf,
            self.asset,
            self.side,
            self.fill_ms,
            self.exit_ms,
            self.entry,
            self.stop,
            self.exit,
            self.r,
            self.status
        )
    }
}

/// Share of the book at risk for a trade with this entry and stop.
fn book_risk_share(entry: f64, stop: f64) -> f64 {
    if entry == 0.0 {
        return 0.0;
    }
    ((entry - stop) / entry).abs().min(BOOK_RISK)
}

fn book_growth(entry: f64, stop: f64, r: f64) -> f64 {
    1.0 + book_risk_share(entry, stop) * r
}

/// Parses the trades file, skipping the header and bad lines.
#[must_use]
pub fn parse_trades(text: &str) -> Vec<TradeRow> {
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split(',').collect();
            if f.len() != 11 {
                return None;
            }
            Some(TradeRow {
                key: f[0].to_owned(),
                tf: f[1].to_owned(),
                asset: f[2].to_owned(),
                side: f[3].to_owned(),
                fill_ms: f[4].parse().ok()?,
                exit_ms: f[5].parse().ok()?,
                entry: f[6].parse().ok()?,
                stop: f[7].parse().ok()?,
                exit: f[8].parse().ok()?,
                r: f[9].parse().ok()?,
                status: f[10].to_owned(),
            })
        })
        .collect()
}

/// The three paper books: when each started and its closed trades.
#[derive(Debug, Clone, Default)]
pub struct Books {
    pub starts: BTreeMap<String, i64>,
    pub trades: Vec<TradeRow>,
    /// True when the start file did not exist and must be written.
    pub new_starts: bool,
}

impl Books {
    /// Reads the books from `live_dir`; books without a start begin at `now_ms`.
    #[must_use]
    pub fn load(live_dir: &Path, now_ms: i64) -> Self {
        let mut starts: BTreeMap<String, i64> = std::fs::read_to_string(live_dir.join(BOOKS_FILE))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let (tf, ms) = l.split_once(',')?;
                Some((tf.trim().to_owned(), ms.trim().parse().ok()?))
            })
            .collect();
        let mut new_starts = false;
        for (tf, _) in BOOK_FRAMES {
            starts.entry(tf.to_owned()).or_insert_with(|| {
                new_starts = true;
                now_ms
            });
        }
        let trades =
            parse_trades(&std::fs::read_to_string(live_dir.join(TRADES_FILE)).unwrap_or_default());
        Self {
            starts,
            trades,
            new_starts,
        }
    }

    #[must_use]
    pub fn starts_csv(&self) -> String {
        self.starts.iter().fold(String::new(), |mut out, (tf, ms)| {
            let _ = writeln!(out, "{tf},{ms}");
            out
        })
    }

    fn start(&self, tf: &str) -> i64 {
        self.starts.get(tf).copied().unwrap_or(i64::MAX)
    }

    fn equity(&self, tf: &str) -> f64 {
        BOOK_CAPITAL
            * self
                .trades
                .iter()
                .filter(|t| t.tf == tf)
                .map(TradeRow::growth)
                .product::<f64>()
    }

    /// Balance of one paper book.
    #[must_use]
    pub fn equity_of(&self, tf: &str) -> f64 {
        self.equity(tf)
    }

    /// True when the book already has this trade.
    #[must_use]
    pub fn has_trade_key(&self, key: &str) -> bool {
        self.has_trade(key)
    }

    fn has_trade(&self, key: &str) -> bool {
        self.trades.iter().any(|t| t.key == key)
    }
}

/// Position line for a paper book entering at `entry` with `stop`.
pub fn position_line(label: &str, equity: f64, entry: f64, stop: f64) -> String {
    let move_share = if entry == 0.0 {
        0.0
    } else {
        ((entry - stop) / entry).abs()
    };
    if move_share <= 0.0 {
        return String::new();
    }
    let size = (BOOK_RISK * equity / move_share).min(equity);
    let risk = size * move_share;
    let capped = if move_share < BOOK_RISK {
        " (todo o saldo; o stop é curto e a posição não passa do saldo)"
    } else {
        ""
    };
    format!(
        "Conta fictícia {label} (saldo {}): posição de {}{capped}, perde {} ({}% da conta) se bater o stop.",
        money_pt(equity),
        money_pt(size),
        money_pt(risk),
        ratio_pt(risk / equity * 100.0)
    )
}

/// Historical record of the setup for one asset, frame and side.
struct HistoryNotes<'a> {
    data_dir: &'a Path,
    hourly: BTreeMap<String, Option<Vec<Bar>>>,
    notes: BTreeMap<(String, i64, &'static str), String>,
}

impl<'a> HistoryNotes<'a> {
    fn new(data_dir: &'a Path) -> Self {
        Self {
            data_dir,
            hourly: BTreeMap::new(),
            notes: BTreeMap::new(),
        }
    }

    fn note(&mut self, asset: &str, f: &FrameScan, side: Side, wyckoff: bool) -> String {
        let key = (
            asset.to_owned(),
            f.hours,
            if wyckoff {
                match side {
                    Side::Long => "long-w",
                    Side::Short => "short-w",
                }
            } else {
                side_code(side)
            },
        );
        if let Some(n) = self.notes.get(&key) {
            return n.clone();
        }
        let setups: Vec<Setup> = if f.hours == 24 {
            f.setups.clone()
        } else {
            let data_dir = self.data_dir;
            let hourly = self.hourly.entry(asset.to_owned()).or_insert_with(|| {
                let path = data_dir
                    .join("processed")
                    .join("binance")
                    .join("futures_1h")
                    .join(format!("{asset}.csv"));
                std::fs::read_to_string(path)
                    .ok()
                    .and_then(|t| parse_bars_csv(&t).ok())
            });
            match hourly {
                Some(h) if f.hours == 1 => scan(h, f.params),
                Some(h) => scan(&aggregate(h, f.hours), f.params),
                None => Vec::new(),
            }
        };
        let note = history_text(asset_name(asset), &setups, side, wyckoff);
        self.notes.insert(key, note.clone());
        note
    }
}

fn history_text(name: &str, setups: &[Setup], side: Side, wyckoff: bool) -> String {
    let closed: Vec<&Setup> = setups
        .iter()
        .filter(|s| s.side == side && s.r.is_some() && (!wyckoff || s.wyckoff_range))
        .collect();
    let what = match (side, wyckoff) {
        (Side::Long, false) => "compras",
        (Side::Short, false) => "vendas",
        (Side::Long, true) => "compras com spring",
        (Side::Short, true) => "vendas com UTAD",
    };
    if closed.is_empty() {
        return format!("Histórico: nenhum trade deste setup ({what}) em {name} neste gráfico.");
    }
    let s = stats(closed.iter().copied());
    let rs: Vec<f64> = closed.iter().filter_map(|s| s.r).collect();
    let (_, t) = r_mean_t(&rs);
    let hit = count_f64(s.targets) / count_f64(s.trades) * 100.0;
    let verdict = if s.trades >= 30 && s.mean_r > 0.0 && t >= 2.0 {
        "ganhou com folga, vantagem comprovada"
    } else if s.mean_r > 0.0 {
        "positivo, mas sem prova (pode ser sorte)"
    } else {
        "no histórico este setup PERDEU dinheiro aqui; cuidado"
    };
    format!(
        "Histórico em {name} neste gráfico ({what}): {} trades, {}% bateram o alvo, média {} por trade: {verdict}.",
        s.trades,
        ratio_pt(hit),
        r_pt(s.mean_r)
    )
}

fn tags(wyckoff: bool, climax: bool, side: Side) -> Vec<String> {
    let mut out = Vec::new();
    if wyckoff {
        out.push(match side {
            Side::Long => "Wyckoff: a varrida foi no piso de uma lateral (spring), o caso clássico de acumulação.".to_owned(),
            Side::Short => "Wyckoff: a varrida foi no teto de uma lateral (UTAD), o caso clássico de distribuição.".to_owned(),
        });
    }
    if climax {
        out.push(
            "Volume da varrida foi mais que o dobro da média (clímax: muita gente stopada de uma vez)."
                .to_owned(),
        );
    }
    out
}

fn sweep_story(side: Side, level: f64, extreme: f64) -> String {
    match side {
        Side::Long => format!(
            "o preço passou abaixo do fundo de {}, onde ficam os stops de quem está comprado, foi até {} e fechou de volta acima",
            fmt_pt(level),
            fmt_pt(extreme)
        ),
        Side::Short => format!(
            "o preço passou acima do topo de {}, onde ficam os stops de quem está vendido, foi até {} e fechou de volta abaixo",
            fmt_pt(level),
            fmt_pt(extreme)
        ),
    }
}

fn sweep_message(asset: &str, f: &FrameScan, p: &Sweep) -> String {
    let (beyond, back) = match p.side {
        Side::Long => ("acima", "abaixo"),
        Side::Short => ("abaixo", "acima"),
    };
    let mut lines = vec![
        format!(
            "{} {}: {}. Primeiro passo de um setup de {} (varrida de liquidez).",
            asset_name(asset),
            f.label(),
            sweep_story(p.side, p.swept_level, p.sweep_extreme),
            side_upper(p.side)
        ),
        format!(
            "Falta a confirmação: {} fechar {beyond} de {} (quebra de estrutura, CHoCH) até {}. Se antes disso o preço for {back} de {}, o setup morre.",
            f.candle_word(),
            fmt_pt(p.choch_level),
            when(f.close_ms(p.deadline_index)),
            fmt_pt(p.sweep_extreme)
        ),
    ];
    lines.extend(tags(p.wyckoff_range, p.climax_volume, p.side));
    lines.join("\n")
}

fn plan_text(s: &Setup) -> String {
    let (order, ob, liquidity) = match s.side {
        Side::Long => (
            "ordem de compra",
            "último candle de queda antes da virada",
            "o topo anterior, onde ficam os stops de quem está vendido",
        ),
        Side::Short => (
            "ordem de venda",
            "último candle de alta antes da virada",
            "o fundo anterior, onde ficam os stops de quem está comprado",
        ),
    };
    format!(
        "Plano: {order} em {}, no reteste do order block ({ob}). Stop {} ({}). Alvo {} ({}), {liquidity}. Ganho possível de {} vezes o risco.",
        fmt_pt(s.entry),
        fmt_pt(s.stop),
        pct_pt(change_pct(s.entry, s.stop)),
        fmt_pt(s.target),
        pct_pt(change_pct(s.entry, s.target)),
        ratio_pt(s.reward_to_risk())
    )
}

fn armed_message(asset: &str, f: &FrameScan, s: &Setup, history: &str, position: &str) -> String {
    let (side_where, close_where) = match s.side {
        Side::Long => ("abaixo", "acima"),
        Side::Short => ("acima", "abaixo"),
    };
    let mut lines = vec![
        format!(
            "{} {}: SETUP DE {} ARMADO.",
            asset_name(asset),
            f.label(),
            side_upper(s.side)
        ),
        format!(
            "Como chegou aqui: o preço varreu os stops {side_where} de {} (foi até {}) e depois fechou {close_where} de {}, virando a estrutura (CHoCH).",
            fmt_pt(s.swept_level),
            fmt_pt(s.sweep_extreme),
            fmt_pt(s.choch_level)
        ),
        plan_text(s),
        format!(
            "A ordem vale até {}. Se o preço chegar ao alvo antes de voltar à entrada, o setup é cancelado.",
            when(f.close_ms(s.choch_index + f.params.order_valid))
        ),
    ];
    lines.extend(tags(s.wyckoff_range, s.climax_volume, s.side));
    lines.push(history.to_owned());
    if !position.is_empty() {
        lines.push(position.to_owned());
    }
    lines.join("\n")
}

fn filled_message(
    asset: &str,
    f: &FrameScan,
    s: &Setup,
    fill: usize,
    history: &str,
    position: &str,
) -> String {
    let what = match s.side {
        Side::Long => "na compra",
        Side::Short => "na venda (short)",
    };
    let mut lines = vec![
        format!(
            "{} {}: ENTROU {what} a {} (candle que fechou em {}).",
            asset_name(asset),
            f.label(),
            fmt_pt(s.entry),
            when(f.close_ms(fill))
        ),
        format!(
            "Stop {} ({}), alvo {} ({}). Se nenhum dos dois bater, o trade fecha pelo prazo em {}.",
            fmt_pt(s.stop),
            pct_pt(change_pct(s.entry, s.stop)),
            fmt_pt(s.target),
            pct_pt(change_pct(s.entry, s.target)),
            when(f.close_ms(fill + f.params.max_hold))
        ),
        history.to_owned(),
    ];
    if !position.is_empty() {
        lines.push(position.to_owned());
    }
    lines.join("\n")
}

fn exit_word(status: Status) -> &'static str {
    match status {
        Status::Target => "no alvo",
        Status::Stop => "no stop",
        _ => "pelo prazo",
    }
}

fn direction(side: Side) -> f64 {
    match side {
        Side::Long => 1.0,
        Side::Short => -1.0,
    }
}

fn closed_message(asset: &str, f: &FrameScan, s: &Setup, book: Option<(f64, f64)>) -> String {
    let exit = s.exit_price.unwrap_or(s.entry);
    let r = s.r.unwrap_or(0.0);
    let mut text = format!(
        "{} {}: SAIU {} a {} (candle que fechou em {}). Entrada foi {}. Resultado do trade: {} ({}, já com custos).",
        asset_name(asset),
        f.label(),
        exit_word(s.status),
        fmt_pt(exit),
        when(f.close_ms(s.exit_index.unwrap_or(0))),
        fmt_pt(s.entry),
        pct_pt(direction(s.side) * change_pct(s.entry, exit)),
        r_pt(r)
    );
    if let Some((before, after)) = book {
        let _ = write!(
            text,
            "\nConta fictícia {}: {} no trade, saldo agora {}.",
            f.label(),
            signed_money(after - before),
            money_pt(after)
        );
    }
    text
}

fn cancelled_message(asset: &str, f: &FrameScan, s: &Setup) -> String {
    let ran = s
        .exit_index
        .and_then(|i| f.bars.get(i))
        .is_some_and(|b| match s.side {
            Side::Long => b.high >= s.target,
            Side::Short => b.low <= s.target,
        });
    let why = if ran {
        format!(
            "o preço chegou ao alvo de {} sem voltar para {}",
            fmt_pt(s.target),
            fmt_pt(s.entry)
        )
    } else {
        format!("a ordem em {} venceu sem o preço voltar", fmt_pt(s.entry))
    };
    format!(
        "{} {}: setup de {} CANCELADO sem entrar: {why}. Nada muda na conta fictícia.",
        asset_name(asset),
        f.label(),
        side_upper(s.side)
    )
}

fn before_book(label: &str) -> String {
    format!(
        "Esse trade começou antes de a conta fictícia {label} existir, então não conta no saldo dela."
    )
}

/// One message to send, with the stage key to save.
#[derive(Debug, Clone)]
pub struct StageAlert {
    pub key: String,
    pub message: String,
    /// Goes to Telegram and phone notifications (otherwise dashboard only).
    pub notify: bool,
}

/// Everything new for one asset this run: stage messages and closed trades
/// for the paper books (which `books` is updated with).
#[allow(clippy::too_many_lines)] // one pass over the stages of every setup
pub fn stage_alerts(
    data_dir: &Path,
    asset: &str,
    frames: &[FrameScan],
    now_ms: i64,
    sent: &BTreeSet<String>,
    books: &mut Books,
) -> (Vec<StageAlert>, Vec<TradeRow>) {
    let mut history = HistoryNotes::new(data_dir);
    let mut alerts = Vec::new();
    let mut new_trades = Vec::new();
    for f in frames {
        let tf = f.tf();
        let has_book = BOOK_FRAMES.iter().any(|(b, _)| *b == tf);
        let prefix = |side: Side, sweep_index: usize| {
            format!(
                "{asset}|{tf}|{}|{}",
                side_code(side),
                f.open_ms(sweep_index)
            )
        };
        let unsent = |key: &str| !sent.contains(key);
        let mut push = |key: String, message: String, notify: bool| {
            alerts.push(StageAlert {
                key,
                message,
                notify,
            });
        };
        for p in &f.pending {
            let base = prefix(p.side, p.sweep_index);
            let key = format!("{base}|varrida");
            if unsent(&key) {
                let notify = is_candidate(f.hours, p.wyckoff_range);
                push(key, sweep_message(asset, f, p), notify);
                if notify {
                    // Marker so a later "did not become a setup" also notifies.
                    push(format!("{base}|vw"), String::new(), false);
                }
            }
        }
        let mut closed: Vec<&Setup> = Vec::new();
        for s in &f.setups {
            let base = prefix(s.side, s.sweep_index);
            let candidate = is_candidate(f.hours, s.wyckoff_range);
            let start = books.start(tf);
            let in_book =
                has_book && candidate && s.fill_index.is_some_and(|k| f.close_ms(k) > start);
            if s.status == Status::Waiting {
                let key = format!("{base}|armado");
                if unsent(&key) {
                    let note = history.note(asset, f, s.side, candidate);
                    let pos = if candidate && has_book {
                        position_line(f.label(), books.equity(tf), s.entry, s.stop)
                    } else {
                        String::new()
                    };
                    let text = with_verdict(&armed_message(asset, f, s, &note, &pos), candidate);
                    push(key, text, candidate);
                }
            }
            if let Some(k) = s.fill_index {
                let key = format!("{base}|entrou");
                if unsent(&key) && (s.status == Status::Open || f.recent(k, now_ms) || in_book) {
                    let note = history.note(asset, f, s.side, candidate);
                    let pos = if in_book {
                        position_line(f.label(), books.equity(tf), s.entry, s.stop)
                    } else if candidate && has_book {
                        before_book(f.label())
                    } else {
                        String::new()
                    };
                    let text =
                        with_verdict(&filled_message(asset, f, s, k, &note, &pos), candidate);
                    push(key, text, candidate);
                }
            }
            if s.status == Status::Cancelled {
                let key = format!("{base}|cancelado");
                let armed = sent.contains(&format!("{base}|armado"));
                if armed && unsent(&key) {
                    push(key, cancelled_message(asset, f, s), candidate);
                }
            }
            if s.r.is_some() && in_book && !books.has_trade(&base) {
                closed.push(s);
            }
            if s.r.is_some() && !in_book {
                let key = format!("{base}|saiu");
                if unsent(&key) && s.exit_index.is_some_and(|i| f.recent(i, now_ms)) {
                    let mut text = closed_message(asset, f, s, None);
                    if candidate && has_book {
                        text.push('\n');
                        text.push_str(&before_book(f.label()));
                    }
                    push(key, text, candidate);
                }
            }
        }
        // Book trades in exit order, so each balance follows the one before.
        closed.sort_by_key(|s| s.exit_index);
        for s in closed {
            let base = prefix(s.side, s.sweep_index);
            let before = books.equity(tf);
            let row = TradeRow {
                key: base.clone(),
                tf: tf.to_owned(),
                asset: asset.to_owned(),
                side: side_code(s.side).to_owned(),
                fill_ms: s.fill_index.map_or(0, |k| f.close_ms(k)),
                exit_ms: s.exit_index.map_or(0, |k| f.close_ms(k)),
                entry: s.entry,
                stop: s.stop,
                exit: s.exit_price.unwrap_or(s.entry),
                r: s.r.unwrap_or(0.0),
                status: format!("{:?}", s.status).to_lowercase(),
            };
            books.trades.push(row.clone());
            new_trades.push(row);
            let after = books.equity(tf);
            let key = format!("{base}|saiu");
            if unsent(&key) {
                push(
                    key,
                    closed_message(asset, f, s, Some((before, after))),
                    true,
                );
            }
        }
        // Sweeps announced earlier that never became a setup.
        let first_ms = f.open_ms(0);
        let started: BTreeSet<i64> = f
            .setups
            .iter()
            .filter(|s| s.status != Status::Cancelled || s.fill_index.is_some())
            .map(|s| f.open_ms(s.sweep_index))
            .collect();
        let waiting: BTreeSet<i64> = f.pending.iter().map(|p| f.open_ms(p.sweep_index)).collect();
        let head = format!("{asset}|{tf}|");
        for key in sent.range(head.clone()..) {
            let Some(rest) = key.strip_prefix(&head) else {
                break;
            };
            let Some(base) = key.strip_suffix("|varrida") else {
                continue;
            };
            let mut parts = rest.split('|');
            let side = parts.next().unwrap_or("");
            let Some(sweep_ms) = parts.next().and_then(|t| t.parse::<i64>().ok()) else {
                continue;
            };
            let done = ["armado", "entrou", "anulada"]
                .iter()
                .any(|stage| sent.contains(&format!("{base}|{stage}")));
            if done
                || sweep_ms <= first_ms
                || waiting.contains(&sweep_ms)
                || started.contains(&sweep_ms)
            {
                continue;
            }
            let word = if side == "long" { "compra" } else { "venda" };
            push(
                format!("{base}|anulada"),
                format!(
                    "{} {}: a varrida de {} não virou setup de {word} (o preço passou do extremo da varrida, fez uma varrida nova no lugar dela, a confirmação não veio no prazo, ou não havia alvo que pagasse 2 para 1). Nada a fazer.",
                    asset_name(asset),
                    f.label(),
                    when(sweep_ms)
                ),
                sent.contains(&format!("{base}|vw")),
            );
        }
    }
    (alerts, new_trades)
}

/// Puts the candidate or follow-only line right after the title line.
fn with_verdict(message: &str, candidate: bool) -> String {
    match message.split_once('\n') {
        Some((title, rest)) => format!("{title}\n{}\n{rest}", verdict_line(candidate)),
        None => format!("{message}\n{}", verdict_line(candidate)),
    }
}

/// Section of the daily summary: paper books, open trades, armed orders,
/// sweeps waiting for confirmation and the nearest liquidity.
#[must_use]
#[allow(clippy::too_many_lines)] // one section: books, trades, orders, sweeps, liquidity
pub fn summary_section(assets: &[(String, Vec<FrameScan>)], books: &Books) -> String {
    let mut lines = vec![
        "Smart Money / Wyckoff".to_owned(),
        format!(
            "Contas fictícias (cada uma começou com {}; entram só nos candidatos spring/UTAD; cada trade arrisca 2% sem passar do saldo):",
            money_pt(BOOK_CAPITAL)
        ),
    ];
    for (tf, label) in BOOK_FRAMES {
        let mine: Vec<&TradeRow> = books.trades.iter().filter(|t| t.tf == tf).collect();
        let equity = books.equity(tf);
        let count = |status: &str| mine.iter().filter(|t| t.status == status).count();
        lines.push(format!(
            "{label}: {} ({}), {} trades fechados ({} no alvo, {} no stop, {} pelo prazo).",
            money_pt(equity),
            pct_pt(change_pct(BOOK_CAPITAL, equity)),
            mine.len(),
            count("target"),
            count("stop"),
            count("time")
        ));
    }
    let mut open = Vec::new();
    let mut armed = Vec::new();
    let mut sweeps = Vec::new();
    let mut liquidity = Vec::new();
    for (asset, frames) in assets {
        let name = asset_name(asset);
        let mut above = Vec::new();
        let mut below = Vec::new();
        for f in frames {
            let now = f.last_close().unwrap_or(0.0);
            for s in &f.setups {
                let side = match s.side {
                    Side::Long => "comprado",
                    Side::Short => "vendido",
                };
                let tag = if is_candidate(f.hours, s.wyckoff_range) {
                    "[candidato] "
                } else {
                    "(só acompanhar) "
                };
                match s.status {
                    Status::Open => open.push(format!(
                        "{tag}{name} {}: {side} desde {}, stop {}, alvo {}; agora {} ({} a favor).",
                        f.label(),
                        fmt_pt(s.entry),
                        fmt_pt(s.stop),
                        fmt_pt(s.target),
                        fmt_pt(now),
                        pct_pt(direction(s.side) * change_pct(s.entry, now))
                    )),
                    Status::Waiting => armed.push(format!(
                        "{tag}{name} {}: ordem de {} em {}, stop {}, alvo {} ({} para 1); vale até {}.",
                        f.label(),
                        if s.side == Side::Long {
                            "compra"
                        } else {
                            "venda"
                        },
                        fmt_pt(s.entry),
                        fmt_pt(s.stop),
                        fmt_pt(s.target),
                        ratio_pt(s.reward_to_risk()),
                        when(f.close_ms(s.choch_index + f.params.order_valid))
                    )),
                    _ => {}
                }
            }
            for p in &f.pending {
                let (pass, close) = match p.side {
                    Side::Long => ("abaixo", "acima"),
                    Side::Short => ("acima", "abaixo"),
                };
                sweeps.push(format!(
                    "{name} {}: varreu {pass} de {}; vira setup se fechar {close} de {} até {}.",
                    f.label(),
                    fmt_pt(p.swept_level),
                    fmt_pt(p.choch_level),
                    when(f.close_ms(p.deadline_index))
                ));
            }
            if f.hours >= 4 {
                let (up, down) = nearest_liquidity(&f.bars, f.params.wing);
                let which = if f.hours == 4 { "4h" } else { "diário" };
                if let Some(u) = up {
                    above.push(format!("{} ({which})", fmt_pt(u)));
                }
                if let Some(d) = down {
                    below.push(format!("{} ({which})", fmt_pt(d)));
                }
            }
        }
        if !above.is_empty() || !below.is_empty() {
            liquidity.push(format!(
                "{name}: acima {}; abaixo {}.",
                if above.is_empty() {
                    "nada perto".to_owned()
                } else {
                    above.join(" e ")
                },
                if below.is_empty() {
                    "nada perto".to_owned()
                } else {
                    below.join(" e ")
                }
            ));
        }
    }
    lines.push(String::new());
    lines.push("Trades abertos".to_owned());
    if open.is_empty() {
        lines.push("Nenhum.".to_owned());
    }
    lines.extend(open);
    lines.push(String::new());
    lines.push("Ordens armadas (esperando o preço voltar ao order block)".to_owned());
    if armed.is_empty() {
        lines.push("Nenhuma.".to_owned());
    }
    lines.extend(armed);
    if !sweeps.is_empty() {
        lines.push(String::new());
        lines.push("Varridas esperando confirmação".to_owned());
        lines.extend(sweeps);
    }
    if !liquidity.is_empty() {
        lines.push(String::new());
        lines.push(
            "Liquidez mais perto (topos e fundos ainda não tocados, onde ficam stops)".to_owned(),
        );
        lines.extend(liquidity);
    }
    lines.join("\n")
}

/// Saves the stage keys, the new closed trades and, the first time, the
/// start of each book.
///
/// # Errors
///
/// Returns an I/O error when a file cannot be written.
pub fn save(live_dir: &Path, books: &Books, keys: &str, trades: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let append = |name: &str, text: &str, header: Option<&str>| -> std::io::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let path = live_dir.join(name);
        let fresh = !path.exists();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        if let (true, Some(h)) = (fresh, header) {
            writeln!(file, "{h}")?;
        }
        file.write_all(text.as_bytes())
    };
    append(SENT_FILE, keys, None)?;
    append(TRADES_FILE, trades, Some(TRADES_HEADER))?;
    if books.new_starts {
        std::fs::write(live_dir.join(BOOKS_FILE), books.starts_csv())?;
    }
    Ok(())
}

/// Groups messages into Telegram-sized texts (under `limit` characters).
#[must_use]
pub fn pack_messages(messages: &[String], limit: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in messages {
        let fits = out
            .last()
            .is_some_and(|last| last.chars().count() + 2 + m.chars().count() <= limit);
        match out.last_mut() {
            Some(last) if fits => {
                last.push_str("\n\n");
                last.push_str(m);
            }
            _ => out.push(m.chars().take(limit).collect()),
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn book_risks_two_percent_but_never_more_than_the_balance() {
        // Stop 5% away: position is 40% of the book, a full stop loses 2%.
        assert!((book_growth(100.0, 95.0, -1.0) - 0.98).abs() < 1e-12);
        // Stop 0.5% away: the position is capped at the book, a stop loses 0.5%.
        assert!((book_growth(100.0, 99.5, -1.0) - 0.995).abs() < 1e-12);
        assert!((book_growth(100.0, 95.0, 3.0) - 1.06).abs() < 1e-12);
    }

    #[test]
    fn trades_round_trip_through_csv() {
        let row = TradeRow {
            key: "SOLUSDT|4h|long|1700000000000".to_owned(),
            tf: "4h".to_owned(),
            asset: "SOLUSDT".to_owned(),
            side: "long".to_owned(),
            fill_ms: 1,
            exit_ms: 2,
            entry: 89.1,
            stop: 87.6,
            exit: 94.5,
            r: 3.5,
            status: "target".to_owned(),
        };
        let text = format!("{TRADES_HEADER}\n{}\n", row.csv());
        assert_eq!(parse_trades(&text), vec![row]);
    }

    #[test]
    fn messages_are_packed_under_the_limit() {
        let msgs = vec!["a".repeat(10), "b".repeat(10), "c".repeat(10)];
        let packed = pack_messages(&msgs, 22);
        assert_eq!(packed.len(), 2);
        assert_eq!(
            packed[0],
            format!("{}\n\n{}", "a".repeat(10), "b".repeat(10))
        );
        assert_eq!(pack_messages(&["x".repeat(50)], 20)[0].chars().count(), 20);
    }

    #[test]
    fn history_text_handles_no_trades() {
        assert!(history_text("SOL", &[], Side::Long, false).contains("nenhum trade"));
    }

    #[test]
    fn position_line_caps_at_the_balance() {
        let wide = position_line("swing (4h)", 2200.0, 100.0, 95.0);
        assert!(wide.contains("$880"), "{wide}");
        assert!(wide.contains("$44"), "{wide}");
        let tight = position_line("curto (1h)", 2200.0, 100.0, 99.5);
        assert!(tight.contains("$2.200"), "{tight}");
        assert!(tight.contains("$11"), "{tight}");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod run_tests {
    use super::*;

    fn walk(n: usize, hours: i64, seed: u64) -> Vec<Bar> {
        let mut x = seed;
        let mut price = 100.0_f64;
        (0..n)
            .map(|i| {
                x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                let u = f64::from(u32::try_from(x >> 40).unwrap()) / f64::from(1u32 << 24);
                let open = price;
                price *= 1.0 + (u - 0.5) * 0.03;
                let close = price;
                Bar {
                    open_time_ms: i64::try_from(i).unwrap() * hours * HOUR_MS,
                    open,
                    high: open.max(close) * (1.0 + u * 0.01),
                    low: open.min(close) * (1.0 - (1.0 - u) * 0.01),
                    close,
                    volume: 100.0 + u * 300.0,
                }
            })
            .collect()
    }

    #[test]
    fn every_stage_renders_and_books_follow() {
        let hourly = walk(760, 1, 7);
        let daily: Vec<Bar> = walk(800, 24, 11)
            .into_iter()
            .map(|mut b| {
                b.open_time_ms -= 800 * 24 * HOUR_MS - 760 * HOUR_MS;
                b
            })
            .collect();
        let frames = scan_frames(&hourly, &daily);
        let now = hourly.last().unwrap().open_time_ms + HOUR_MS + 60_000;
        let mut books = Books::default();
        for tf in ["1h", "4h", "1d"] {
            books.starts.insert(tf.to_owned(), 0);
        }
        let dir = std::env::temp_dir();
        let (alerts, trades) =
            stage_alerts(&dir, "SOLUSDT", &frames, now, &BTreeSet::new(), &mut books);
        for a in &alerts {
            println!("{}\n{}\n", a.key, a.message);
        }
        assert!(!trades.is_empty());
        let keys: BTreeSet<String> = alerts.iter().map(|a| a.key.clone()).collect();
        assert_eq!(keys.len(), alerts.len(), "duplicate keys");
        // A second run with everything saved says nothing new.
        let (again, more) = stage_alerts(&dir, "SOLUSDT", &frames, now, &keys, &mut books);
        assert!(again.is_empty(), "{:?}", again.first().map(|a| &a.key));
        assert!(more.is_empty());
        let section = summary_section(&[("SOLUSDT".to_owned(), frames)], &books);
        println!("{section}");
        assert!(section.contains("Contas fictícias"));
    }
}
