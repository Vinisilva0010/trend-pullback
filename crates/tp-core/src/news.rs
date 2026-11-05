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
            Self::BigHolders => "Grandes compradores ou vendedores",
            Self::Liquidations => "Liquidações em massa",
            Self::Listing => "Listagem ou deslistagem",
            Self::Geopolitics => "Guerra, sanções ou eleições",
            Self::PriceMove => "Movimento forte de preço",
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
}
