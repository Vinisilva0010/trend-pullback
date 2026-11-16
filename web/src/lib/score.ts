// Scoreboard of trades in R (1R = what the stop loses): the paper books
// of the tool and the user's own trades, each against what the 10-year
// test of the same setup gave.

import type { Book } from "./schema";

/** What the test of a setup gave, per trade. */
export type Reference = { r: number; winShare: number; trades: number; source: string };

export type SetupId = "acoes-1d" | "acoes-1w" | "cripto-st1d" | "cripto-smc" | "ipo" | "capitulacao" | "outro";

export type SetupInfo = { id: SetupId; label: string; reference: Reference | null; note: string };

// From `tp refine-study` (lapidar-resultado, Nov 2026): the stock trap
// trades with no filter, both periods together.
export const SETUPS: SetupInfo[] = [
  {
    id: "acoes-1d",
    label: "Ações EUA: trap de compra no diário, a favor da tendência",
    reference: { r: 0.15, winShare: 0.46, trades: 659, source: "teste de 10 anos nas ações e ETFs da lista" },
    note: "Passou no teste nos dois períodos.",
  },
  {
    id: "acoes-1w",
    label: "Ações EUA: trap de compra no semanal",
    reference: { r: 0.21, winShare: 0.45, trades: 1175, source: "teste de 10 anos nas ações e ETFs da lista" },
    note: "Passou no teste nos dois períodos.",
  },
  {
    id: "cripto-st1d",
    label: "Cripto: Stormer diário (trap) no BTC, ETH, SOL",
    reference: null,
    note: "Em cripto nenhum setup do Stormer passou no teste; a conta fictícia serve para ver se ao vivo é diferente.",
  },
  {
    id: "cripto-smc",
    label: "Cripto: spring/UTAD no 1h ou 4h",
    reference: null,
    note: "Ficou positivo no histórico, mas sem prova.",
  },
  {
    id: "ipo",
    label: "IPO: compra no fechamento do 1º dia, vende 5 pregões depois",
    reference: null,
    note: "O teste mediu em %, não em R: +3,6% acima do S&P 500 em média, 59% dos casos, pior caso -32,5%.",
  },
  {
    id: "capitulacao",
    label: "Cripto: pavio de capitulação com medo em 20 ou menos",
    reference: null,
    note: "Passou só em 90 dias, medido em %, não em R, e com poucos dias diferentes.",
  },
  { id: "outro", label: "Fora dos setups testados", reference: null, note: "Trade por conta própria, sem teste." },
];

export function setupInfo(id: SetupId): SetupInfo {
  return SETUPS.find((s) => s.id === id) ?? SETUPS[SETUPS.length - 1]!;
}

export type Score = {
  trades: number;
  wins: number;
  winShare: number;
  meanR: number;
  totalR: number;
  /** Longest run of losing trades in a row. */
  worstStreak: number;
  bestR: number;
  worstR: number;
  /** Range of the mean that luck alone allows (about 95%), from 2 trades on. */
  luck: { low: number; high: number } | null;
};

/** Score of trades in R, oldest first. */
export function score(rs: number[]): Score {
  const n = rs.length;
  const totalR = rs.reduce((a, b) => a + b, 0);
  const meanR = n ? totalR / n : 0;
  let streak = 0;
  let worstStreak = 0;
  for (const r of rs) {
    streak = r < 0 ? streak + 1 : 0;
    worstStreak = Math.max(worstStreak, streak);
  }
  const wins = rs.filter((r) => r > 0).length;
  let luck: Score["luck"] = null;
  if (n >= 2) {
    const sd = Math.sqrt(rs.reduce((a, r) => a + (r - meanR) ** 2, 0) / (n - 1));
    const half = (2 * sd) / Math.sqrt(n);
    luck = { low: meanR - half, high: meanR + half };
  }
  return {
    trades: n,
    wins,
    winShare: n ? wins / n : 0,
    meanR,
    totalR,
    worstStreak,
    bestR: n ? Math.max(...rs) : 0,
    worstR: n ? Math.min(...rs) : 0,
    luck,
  };
}

/** Fewest trades before a live result says anything against the test. */
export const MIN_TRADES = 20;

export type Verdict = { tone: "neutral" | "good" | "bad"; text: string };

/** How a live score compares with the test of the same setup. */
export function verdict(s: Score, ref: Reference | null): Verdict {
  if (s.trades === 0) return { tone: "neutral", text: "Nenhum trade fechado ainda." };
  if (s.trades < MIN_TRADES || !s.luck) {
    return {
      tone: "neutral",
      text: `${s.trades} ${s.trades === 1 ? "trade" : "trades"}: ainda é pouco. Com menos de ${MIN_TRADES} trades, sorte e azar mandam no resultado; não dá para dizer se está igual ao teste.`,
    };
  }
  if (!ref) {
    if (s.luck.low > 0) return { tone: "good", text: "Ganhando, e com trades suficientes para não ser só sorte." };
    if (s.luck.high < 0) return { tone: "bad", text: "Perdendo, e com trades suficientes para não ser só azar." };
    return { tone: "neutral", text: "Ainda não dá para separar o resultado de sorte ou azar." };
  }
  if (ref.r < s.luck.low) return { tone: "good", text: "Melhor que o teste, além do que a sorte explica." };
  if (ref.r > s.luck.high) {
    return { tone: "bad", text: "Pior que o teste, além do que o azar explica. Vale olhar se o mercado mudou ou se a execução está diferente da regra." };
  }
  return { tone: "neutral", text: "Dentro do esperado pelo teste: a diferença cabe na sorte ou no azar." };
}

/** Which setup a paper-book trade belongs to. */
export function bookSetup(bookTf: string, key: string): SetupId | null {
  if (bookTf === "acoes") {
    const tf = key.split("|")[1];
    return tf === "1w" ? "acoes-1w" : tf === "1d" ? "acoes-1d" : null;
  }
  if (bookTf === "st1d") return "cripto-st1d";
  if (bookTf === "1h" || bookTf === "4h") return "cripto-smc";
  return null;
}

/** R of the paper-book trades of each setup, oldest first. */
export function bookRs(books: Book[]): Map<SetupId, number[]> {
  const out = new Map<SetupId, number[]>();
  for (const b of books) {
    for (const t of [...b.trades].sort((x, y) => x.exit_ms - y.exit_ms)) {
      const id = bookSetup(b.tf, t.key);
      if (!id) continue;
      out.set(id, [...(out.get(id) ?? []), t.r]);
    }
  }
  return out;
}
