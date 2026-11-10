//! Sorting news headlines into the kinds of events that move crypto prices.
//!
//! Matching is by keywords on the lowercased title and summary, so it is
//! fast and predictable; a headline can match several kinds and the most
//! serious one wins. Price predictions and opinion pieces are ignored.

/// Kind of event a headline is about, most serious first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Hack,
    ExchangeTrouble,
    NetworkOutage,
    Stablecoin,
    Regulation,
    Macro,
    Etf,
    Earnings,
    BigHolders,
    Liquidations,
    Listing,
    Geopolitics,
    PriceMove,
}

impl Kind {
    /// Portuguese name used in the alert.
    #[must_use]
    pub fn label_pt(self) -> &'static str {
        match self {
            Self::Hack => "Hack ou roubo",
            Self::ExchangeTrouble => "Problema em corretora",
            Self::NetworkOutage => "Rede parada ou com falha",
            Self::Stablecoin => "Stablecoin (USDT, USDC)",
            Self::Regulation => "Governo, regulação ou processo",
            Self::Macro => "Juros, inflação ou economia dos EUA",
            Self::Etf => "ETF",
            Self::Earnings => "Balanço de empresa grande",
            Self::BigHolders => "Grandes compradores ou vendedores",
            Self::Liquidations => "Liquidações em massa",
            Self::Listing => "Listagem ou deslistagem",
            Self::Geopolitics => "Guerra, sanções ou eleições",
            Self::PriceMove => "Movimento forte de preço",
        }
    }

    /// What this kind of news usually does to prices and what to hope for,
    /// in plain Portuguese. General market experience, not a tested edge.
    #[must_use]
    pub fn meaning_pt(self) -> &'static str {
        match self {
            Self::Hack => {
                "Em geral derruba a moeda atingida nos primeiros dias, e o mercado todo se o valor for grande. Torcer para: o dinheiro ser congelado ou devolvido."
            }
            Self::ExchangeTrouble => {
                "Em geral derruba o mercado todo, por medo de quebra (como a FTX em 2022). Torcer para: os saques voltarem rápido."
            }
            Self::NetworkOutage => {
                "Em geral derruba a moeda da rede enquanto ela está parada. Torcer para: a rede voltar logo e sem perda de fundos."
            }
            Self::Stablecoin => {
                "Stablecoin saindo de 1 dólar em geral derruba o mercado todo e trava saques. Torcer para: voltar a 1 dólar rápido."
            }
            Self::Regulation => {
                "Processo, proibição ou regra dura em geral derrubam o preço; aprovação ou regra clara em geral ajudam a subir. Torcer para: decisão a favor ou regra clara."
            }
            Self::Macro => {
                "Juros caindo, inflação abaixo do esperado ou dólar mais fraco em geral ajudam cripto a subir; o contrário em geral derruba. Torcer para: juros e inflação mais baixos."
            }
            Self::Etf => {
                "ETF aprovado ou dinheiro entrando nos ETFs em geral ajuda a subir; dinheiro saindo em geral derruba. Torcer para: entrada de dinheiro."
            }
            Self::Earnings => {
                "Balanço acima do esperado em empresa grande em geral ajuda o índice a subir; abaixo do esperado ou previsão fraca em geral derruba. Torcer para: resultado e previsão acima do esperado."
            }
            Self::BigHolders => {
                "Governo, baleia ou liberação de tokens mandando moedas para corretora em geral pressiona para baixo; empresa comprando em geral ajuda a subir. Torcer para: compra, não venda."
            }
            Self::Liquidations => {
                "Liquidação em massa de comprados acelera a queda; de vendidos acelera a alta. Depois que passa, o preço costuma se acalmar."
            }
            Self::Listing => {
                "Listagem em corretora grande em geral ajuda a subir; deslistagem em geral derruba. Torcer para: listagem."
            }
            Self::Geopolitics => {
                "Guerra, ataque ou sanção em geral derrubam no começo, por medo; acordo ou cessar-fogo em geral ajudam a subir. Torcer para: acordo."
            }
            Self::PriceMove => {
                "Essa notícia só descreve um movimento que já aconteceu; não indica o próximo."
            }
        }
    }

    /// Like [`Self::meaning_pt`] for the US stock market.
    #[must_use]
    pub fn meaning_stocks_pt(self) -> &'static str {
        match self {
            Self::Macro => {
                "Juros caindo, inflação abaixo do esperado ou emprego sem exagero em geral ajudam a bolsa a subir; juros subindo ou inflação acima do esperado em geral derrubam. Torcer para: juros e inflação mais baixos."
            }
            Self::Regulation => {
                "Processo ou regra dura contra empresa grande em geral derruba a ação dela e às vezes o setor; decisão a favor em geral ajuda. Torcer para: decisão a favor."
            }
            Self::Geopolitics => {
                "Guerra, tarifa ou sanção em geral derrubam a bolsa no começo, por medo; acordo em geral ajuda a subir. Torcer para: acordo."
            }
            other => other.meaning_pt(),
        }
    }

    /// True for kinds that matter even when no tracked coin is named.
    #[must_use]
    pub fn market_wide(self) -> bool {
        matches!(
            self,
            Self::ExchangeTrouble
                | Self::Stablecoin
                | Self::Regulation
                | Self::Macro
                | Self::Etf
                | Self::Geopolitics
                | Self::Liquidations
        )
    }
}

const RULES: [(Kind, &[&str]); 12] = [
    (
        Kind::Hack,
        &[
            "hack",
            "exploit",
            "drained",
            "stolen",
            "theft",
            "breach",
            "attacker",
            "compromised",
            "heist",
        ],
    ),
    (
        Kind::ExchangeTrouble,
        &[
            "halts withdrawals",
            "halted withdrawals",
            "pauses withdrawals",
            "paused withdrawals",
            "suspends withdrawals",
            "suspended withdrawals",
            "withdrawals halted",
            "withdrawals paused",
            "withdrawals suspended",
            "insolven",
            "bankrupt",
            "exchange outage",
            "freezes withdrawals",
        ],
    ),
    (
        Kind::NetworkOutage,
        &[
            "outage",
            "network down",
            "network halt",
            "chain halt",
            "stopped producing blocks",
            "block production halted",
            "degraded performance",
            "finality issue",
            "lost finality",
        ],
    ),
    (
        Kind::Stablecoin,
        &["depeg", "de-peg", "tether", "usdt", "usdc", "stablecoin"],
    ),
    (
        Kind::Regulation,
        &[
            " sec ",
            " sec,",
            "sec's",
            "cftc",
            " doj",
            "lawsuit",
            "sues",
            "sued",
            "charges",
            "charged",
            "indict",
            " ban ",
            " bans ",
            "crackdown",
            "regulat",
            "enforcement",
            "senate",
            "congress",
            "white house",
            "executive order",
            "cvm",
            "banco central",
            "central bank",
        ],
    ),
    (
        Kind::Macro,
        &[
            " fed ",
            " fed,",
            "fed's",
            "federal reserve",
            "fomc",
            "powell",
            "rate cut",
            "rate hike",
            "interest rate",
            "cpi",
            "inflation",
            "jobs report",
            "payrolls",
            "unemployment",
            "recession",
            "treasury yield",
            "tariff",
            "dollar index",
        ],
    ),
    (Kind::Etf, &["etf"]),
    (
        Kind::BigHolders,
        &[
            "mt. gox",
            "mt gox",
            "mtgox",
            "government moves",
            "government transfers",
            "government sells",
            "strategy buys",
            "microstrategy",
            "saylor",
            "whale",
            "unlock",
            "blackrock",
            "treasury company",
            "reserve",
        ],
    ),
    (Kind::Liquidations, &["liquidat"]),
    (
        Kind::Listing,
        &["delist", "will list", " lists ", "listing"],
    ),
    (
        Kind::Geopolitics,
        &[
            " war ",
            " war,",
            "missile",
            "invasion",
            "sanction",
            "military",
            "ceasefire",
            "election",
            "strike on",
        ],
    ),
    (
        Kind::PriceMove,
        &[
            "plunge",
            "crash",
            "surge",
            "soar",
            "tumble",
            "all-time high",
            "record high",
            "sell-off",
            "selloff",
            "slump",
            "rally",
            "dump",
        ],
    ),
];

/// Words that mark predictions, guides and opinion pieces, which are skipped.
const NOISE: [&str; 9] = [
    "price prediction",
    "price analysis",
    "prediction:",
    "how to ",
    "what is ",
    "explained",
    "opinion",
    "sponsored",
    "podcast",
];

/// Tracked coins named in a headline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Coins {
    pub btc: bool,
    pub eth: bool,
    pub sol: bool,
}

impl Coins {
    #[must_use]
    pub fn any(self) -> bool {
        self.btc || self.eth || self.sol
    }

    /// Portuguese list of the coins, or "mercado em geral".
    #[must_use]
    pub fn label_pt(self) -> String {
        let names: Vec<&str> = [(self.btc, "BTC"), (self.eth, "ETH"), (self.sol, "SOL")]
            .iter()
            .filter(|(on, _)| *on)
            .map(|(_, n)| *n)
            .collect();
        if names.is_empty() {
            "mercado em geral".to_owned()
        } else {
            names.join(", ")
        }
    }
}

/// A headline worth an alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Classified {
    pub kind: Kind,
    pub coins: Coins,
}

fn has_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + word.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// Coins named in the text (already lowercased).
#[must_use]
pub fn coins_in(text: &str) -> Coins {
    Coins {
        btc: has_word(text, "bitcoin") || has_word(text, "btc"),
        eth: has_word(text, "ethereum") || has_word(text, "ether") || has_word(text, "eth"),
        sol: has_word(text, "solana") || has_word(text, "sol"),
    }
}

/// Classifies a headline. Returns None for noise, for headlines that match
/// no kind, and for coin-specific kinds that name none of BTC, ETH or SOL.
#[must_use]
pub fn classify_headline(title: &str, summary: &str) -> Option<Classified> {
    let title_lc = title.to_lowercase();
    if NOISE.iter().any(|n| title_lc.contains(n)) {
        return None;
    }
    let text = format!("{title_lc} {}", summary.to_lowercase());
    // Leading and trailing spaces let rules like "sec " match at the edges.
    let padded = format!(" {text} ");
    let kind = RULES
        .iter()
        .find(|(_, words)| words.iter().any(|w| padded.contains(w)))
        .map(|(k, _)| *k)?;
    let coins = coins_in(&text);
    (coins.any() || kind.market_wide()).then_some(Classified { kind, coins })
}

/// Which way a headline pushes prices, read from its words. A rough
/// reading: keyword matching, not a tested edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lean {
    Up,
    Down,
    Unclear,
}

impl Lean {
    /// Code used in the dashboard data.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Up => "alta",
            Self::Down => "queda",
            Self::Unclear => "incerto",
        }
    }

    /// Portuguese words used in the alert.
    #[must_use]
    pub fn label_pt(self) -> &'static str {
        match self {
            Self::Up => "puxa para alta",
            Self::Down => "puxa para queda",
            Self::Unclear => "direção incerta",
        }
    }
}

/// Words and phrases that usually come with rising prices.
const UP_WORDS: &[&str] = &[
    "approve",
    "approves",
    "approved",
    "approval",
    "inflow",
    "inflows",
    "rate cut",
    "rate cuts",
    "cuts rates",
    "cut rates",
    "cooler",
    "cools",
    "cooling",
    "eases",
    "easing",
    "softer",
    "rally",
    "rallies",
    "rallied",
    "surge",
    "surges",
    "surged",
    "soar",
    "soars",
    "soared",
    "jump",
    "jumps",
    "jumped",
    "gain",
    "gains",
    "gained",
    "climb",
    "climbs",
    "climbed",
    "rise",
    "rises",
    "rose",
    "rebound",
    "rebounds",
    "rebounded",
    "record high",
    "all-time high",
    "beats",
    "tops estimates",
    "buys",
    "bought",
    "buying",
    "purchase",
    "purchases",
    "acquires",
    "accumulates",
    "ceasefire",
    "truce",
    "peace",
    "deal",
    "agreement",
    "will list",
    "lists",
    "dismiss",
    "dismissed",
    "dismisses",
    "bullish",
    "upgrade",
    "upgraded",
    "upgrades",
    "stimulus",
    "optimism",
    "relief",
    "strong",
    "stronger",
    "recovers",
    "recovery",
];

/// Words and phrases that usually come with falling prices.
const DOWN_WORDS: &[&str] = &[
    "reject",
    "rejects",
    "rejected",
    "denies",
    "denied",
    "outflow",
    "outflows",
    "rate hike",
    "rate hikes",
    "hikes",
    "hotter",
    "sticky",
    "plunge",
    "plunges",
    "plunged",
    "tumble",
    "tumbles",
    "tumbled",
    "slide",
    "slides",
    "slid",
    "fall",
    "falls",
    "fell",
    "drop",
    "drops",
    "dropped",
    "sink",
    "sinks",
    "sank",
    "crash",
    "crashes",
    "sell-off",
    "selloff",
    "slump",
    "slumps",
    "dump",
    "dumps",
    "misses",
    "missed",
    "sells",
    "sold",
    "selling",
    "unlock",
    "unlocks",
    "delist",
    "delists",
    "delisted",
    "ban",
    "bans",
    "banned",
    "sues",
    "sued",
    "lawsuit",
    "charges",
    "charged",
    "probe",
    "investigation",
    "crackdown",
    "war",
    "attack",
    "attacks",
    "missile",
    "invasion",
    "sanction",
    "sanctions",
    "tariff",
    "tariffs",
    "shutdown",
    "recession",
    "layoffs",
    "bearish",
    "downgrade",
    "downgraded",
    "downgrades",
    "fears",
    "warns",
    "default",
    "weak",
    "weaker",
    "slowdown",
    "losses",
    "hack",
    "exploit",
    "outage",
];

fn count_words(text: &str, words: &[&str]) -> usize {
    words.iter().filter(|w| has_word(text, w)).count()
}

/// Reads which way a headline pushes prices. Hacks, exchange trouble,
/// outages and depegs always push down; liquidated shorts push up and
/// liquidated longs push down; the rest goes by the words of the title,
/// then of the summary when the title is even.
#[must_use]
pub fn lean_of(kind: Kind, title: &str, summary: &str) -> Lean {
    let title = title.to_lowercase();
    match kind {
        Kind::Hack | Kind::ExchangeTrouble | Kind::NetworkOutage => return Lean::Down,
        Kind::Stablecoin if title.contains("depeg") || title.contains("de-peg") => {
            return Lean::Down;
        }
        Kind::Liquidations => {
            let shorts = has_word(&title, "short") || has_word(&title, "shorts");
            let longs = has_word(&title, "long") || has_word(&title, "longs");
            match (shorts, longs) {
                (true, false) => return Lean::Up,
                (false, true) => return Lean::Down,
                _ => {}
            }
        }
        _ => {}
    }
    let read = |text: &str| {
        let up = count_words(text, UP_WORDS);
        let down = count_words(text, DOWN_WORDS);
        match up.cmp(&down) {
            std::cmp::Ordering::Greater => Lean::Up,
            std::cmp::Ordering::Less => Lean::Down,
            std::cmp::Ordering::Equal => Lean::Unclear,
        }
    };
    match read(&title) {
        Lean::Unclear => read(&summary.to_lowercase()),
        lean => lean,
    }
}

/// How much a news group matters, 1 to 3: the serious kinds count 3 when
/// two or more sources carry them, 2 with one source; big holders,
/// liquidations and listings count 2; a price move only describes the past
/// and counts 1.
#[must_use]
pub fn level_of(kind: Kind, sources: usize) -> u8 {
    match kind {
        Kind::PriceMove => 1,
        Kind::BigHolders | Kind::Liquidations | Kind::Listing => 2,
        _ if sources >= 2 => 3,
        _ => 2,
    }
}

/// Stock market news: what moves the US market as a whole.
const STOCK_RULES: [(Kind, &[&str]); 5] = [
    (
        Kind::Macro,
        &[
            " fed ",
            " fed,",
            "fed's",
            "federal reserve",
            "fomc",
            "powell",
            "rate cut",
            "rate hike",
            "interest rate",
            "cpi",
            "inflation",
            "jobs report",
            "payrolls",
            "jobless claims",
            "unemployment",
            "recession",
            "gdp",
            "retail sales",
            "consumer spending",
            "pce",
            "treasury yield",
            "bond yields",
            "10-year",
            "dollar index",
        ],
    ),
    (
        Kind::Geopolitics,
        &[
            " war ",
            " war,",
            "missile",
            "invasion",
            "sanction",
            "military",
            "ceasefire",
            "election",
            "tariff",
            "trade war",
            "trade deal",
            "shutdown",
        ],
    ),
    (
        Kind::Regulation,
        &["antitrust", " sec ", "doj", "lawsuit", "executive order"],
    ),
    (
        Kind::Earnings,
        &[
            "earnings",
            "quarterly results",
            "revenue",
            "guidance",
            "profit",
            "forecast",
        ],
    ),
    (
        Kind::PriceMove,
        &[
            "plunge",
            "crash",
            "surge",
            "soar",
            "tumble",
            "record high",
            "all-time high",
            "sell-off",
            "selloff",
            "slump",
            "rally",
            "rallies",
            "rebound",
            "falls",
            "rises",
            "gains",
            "slides",
            "drops",
        ],
    ),
];

/// Names that make a headline about the US market as a whole.
const MARKET_WORDS: &[&str] = &[
    "stocks",
    "stock market",
    "s&p 500",
    "s&p",
    "nasdaq",
    "dow",
    "wall street",
    "stock futures",
    "equities",
    "russell 2000",
];

/// The largest US companies: their earnings move the indexes.
const BIG_NAMES: &[&str] = &[
    "apple",
    "microsoft",
    "nvidia",
    "amazon",
    "alphabet",
    "google",
    "meta",
    "tesla",
    "broadcom",
    "netflix",
    "jpmorgan",
    "berkshire",
    "oracle",
    "amd",
    "walmart",
    "eli lilly",
];

const STOCK_NOISE: &[&str] = &[
    "stocks to buy",
    "should you buy",
    "should you sell",
    "here's why",
    "here's what",
    "is it time to",
    "best stocks",
    "my favorite",
    "millionaire",
    "retire",
];

/// Classifies a US stock market headline. Macro, geopolitics and
/// regulation count on their own; earnings need one of the largest
/// companies; price moves need the market itself (stocks, S&P 500,
/// Nasdaq, Dow).
#[must_use]
pub fn classify_stock_headline(title: &str, summary: &str) -> Option<Kind> {
    let title_lc = title.to_lowercase();
    if NOISE
        .iter()
        .chain(STOCK_NOISE)
        .any(|n| title_lc.contains(n))
    {
        return None;
    }
    let padded = format!(" {title_lc} {} ", summary.to_lowercase());
    let kind = STOCK_RULES
        .iter()
        .find(|(_, words)| words.iter().any(|w| padded.contains(w)))
        .map(|(k, _)| *k)?;
    let about_market = MARKET_WORDS.iter().any(|w| has_word(&title_lc, w));
    let big_name = BIG_NAMES.iter().any(|w| has_word(&title_lc, w));
    let keep = match kind {
        Kind::Earnings => big_name,
        Kind::PriceMove => about_market,
        _ => true,
    };
    keep.then_some(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hacks_win_over_other_kinds() {
        let c = classify_headline("Solana DeFi protocol drained of $40M in exploit", "").unwrap();
        assert_eq!(c.kind, Kind::Hack);
        assert!(c.coins.sol && !c.coins.btc);
    }

    #[test]
    fn macro_news_counts_without_a_coin() {
        let c = classify_headline("Powell signals rate cut as inflation cools", "").unwrap();
        assert_eq!(c.kind, Kind::Macro);
        assert_eq!(c.coins.label_pt(), "mercado em geral");
    }

    #[test]
    fn coin_specific_kinds_need_a_tracked_coin() {
        assert!(classify_headline("Dogecoin price surges 20%", "").is_none());
        let c = classify_headline("Bitcoin price surges past $85K", "").unwrap();
        assert_eq!(c.kind, Kind::PriceMove);
        assert_eq!(c.coins.label_pt(), "BTC");
    }

    #[test]
    fn predictions_and_guides_are_skipped() {
        assert!(classify_headline("Bitcoin price prediction: will BTC crash?", "").is_none());
        assert!(classify_headline("How to stake ETH safely", "").is_none());
    }

    #[test]
    fn words_match_whole_words_only() {
        assert!(!coins_in("solid console soldiers").sol);
        assert!(coins_in("sol jumps").sol);
        assert!(!coins_in("together").eth);
        assert!(coins_in("eth/usd").eth);
    }

    #[test]
    fn exchange_and_stablecoin_trouble_are_market_wide() {
        let c =
            classify_headline("Exchange halts withdrawals after security incident", "").unwrap();
        assert_eq!(c.kind, Kind::ExchangeTrouble);
        let c = classify_headline("USDC briefly loses its dollar peg in depeg scare", "").unwrap();
        assert_eq!(c.kind, Kind::Stablecoin);
    }

    #[test]
    fn short_keywords_need_word_edges() {
        assert!(classify_headline("Urban art fair adds Bitcoin booth", "").is_none());
        let c = classify_headline("SEC sues exchange over unregistered tokens", "").unwrap();
        assert_eq!(c.kind, Kind::Regulation);
        let c = classify_headline("Federal Reserve holds rates steady", "").unwrap();
        assert_eq!(c.kind, Kind::Macro);
    }

    #[test]
    fn unrelated_headlines_are_ignored() {
        assert!(classify_headline("New NFT collection launches on Polygon", "").is_none());
    }

    #[test]
    fn reads_the_lean_of_a_headline() {
        assert_eq!(lean_of(Kind::Hack, "Protocol loses $5M", ""), Lean::Down);
        assert_eq!(
            lean_of(
                Kind::Macro,
                "Powell signals rate cut as inflation cools",
                ""
            ),
            Lean::Up
        );
        assert_eq!(
            lean_of(Kind::Etf, "Bitcoin ETFs see record outflows", ""),
            Lean::Down
        );
        assert_eq!(
            lean_of(Kind::Liquidations, "$500M in shorts liquidated", ""),
            Lean::Up
        );
        assert_eq!(
            lean_of(Kind::Regulation, "SEC meets exchange leaders", ""),
            Lean::Unclear
        );
        // Whole words only: "bank" is not "ban".
        assert_eq!(
            lean_of(Kind::Macro, "Central bank meets", ""),
            Lean::Unclear
        );
    }

    #[test]
    fn levels_weigh_sources_and_kinds() {
        assert_eq!(level_of(Kind::Macro, 3), 3);
        assert_eq!(level_of(Kind::Macro, 1), 2);
        assert_eq!(level_of(Kind::Listing, 5), 2);
        assert_eq!(level_of(Kind::PriceMove, 5), 1);
    }

    #[test]
    fn stock_headlines_need_the_market_or_a_big_name() {
        assert_eq!(
            classify_stock_headline("Fed holds rates steady, signals two cuts", ""),
            Some(Kind::Macro)
        );
        assert_eq!(
            classify_stock_headline("Nvidia earnings beat as data center revenue soars", ""),
            Some(Kind::Earnings)
        );
        assert_eq!(
            classify_stock_headline("Small bakery chain earnings rise", ""),
            None
        );
        assert_eq!(
            classify_stock_headline("Stocks rally as S&P 500 hits record high", ""),
            Some(Kind::PriceMove)
        );
        assert_eq!(classify_stock_headline("Gold price rallies", ""), None);
        assert_eq!(
            classify_stock_headline("3 stocks to buy before the Fed meeting", ""),
            None
        );
    }
}
