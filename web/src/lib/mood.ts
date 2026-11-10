import type { MarketMood } from "./schema";

// The market thermometer: the news of the last 24 hours (read by keywords,
// so it is rough) next to the price (trend and change), and one word that
// says whether both agree.

export type Reading = "alta" | "queda" | "neutro" | "pouca";

/** Below this weight (levels added up) there is too little news to read. */
const MIN_WEIGHT = 4;
const EDGE = 0.2;

function word(score: number): Reading {
  if (score >= EDGE) return "alta";
  if (score <= -EDGE) return "queda";
  return "neutro";
}

const clamp = (v: number) => Math.max(-1, Math.min(1, v));

export function newsReading(m: MarketMood | null | undefined): { reading: Reading; score: number } {
  if (!m || m.weight < MIN_WEIGHT) return { reading: "pouca", score: 0 };
  return { reading: word(m.score), score: clamp(m.score) };
}

/**
 * Price side: half the trend breadth (assets in uptrend minus downtrend,
 * over all) and half the change, where `scale` percent counts as a full
 * move (2% for crypto in 24 hours, 1% for the S&P 500 in a day).
 */
export function priceReading(p: { up: number; down: number; total: number; change: number | null; scale: number }): { reading: Reading; score: number } {
  if (p.total === 0 && p.change == null) return { reading: "pouca", score: 0 };
  const trend = p.total > 0 ? (p.up - p.down) / p.total : 0;
  const move = p.change == null ? trend : clamp(p.change / p.scale);
  const score = clamp(0.5 * trend + 0.5 * move);
  return { reading: word(score), score };
}

export type Verdict = { label: string; tone: "up" | "down" | "mixed" | "flat" };

/** One word from both sides. */
export function verdict(news: Reading, price: Reading): Verdict {
  const pair = new Set([news, price]);
  if (news === price && news === "alta") return { label: "Alta", tone: "up" };
  if (news === price && news === "queda") return { label: "Queda", tone: "down" };
  if (pair.has("alta") && pair.has("queda")) return { label: "Dividido", tone: "mixed" };
  if (pair.has("alta")) return { label: "Alta fraca", tone: "up" };
  if (pair.has("queda")) return { label: "Queda fraca", tone: "down" };
  return { label: "Neutro", tone: "flat" };
}

export const READING_TEXT: Record<Reading, string> = {
  alta: "puxando para alta",
  queda: "puxando para queda",
  neutro: "sem lado claro",
  pouca: "pouca informação",
};
