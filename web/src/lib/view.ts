import type { Pending, Setup } from "./schema";
import { change, pct, price, when } from "./format";

// Plain-language descriptions shared by the pages.

export const ASSETS = ["BTC", "ETH", "SOL"] as const;
export type AssetName = (typeof ASSETS)[number];
export const TIMEFRAMES = ["1h", "4h", "1d", "1w", "1M"] as const;
export type Timeframe = (typeof TIMEFRAMES)[number];

export const TF_LABEL: Record<Timeframe, string> = {
  "1h": "1 hora",
  "4h": "4 horas",
  "1d": "Diário",
  "1w": "Semanal",
  "1M": "Mensal",
};

export function sideWord(side: "long" | "short"): string {
  return side === "long" ? "Compra" : "Venda (short)";
}

export const STATUS_TEXT: Record<Setup["status"], string> = {
  armado: "Ordem armada, esperando o preço voltar à entrada",
  aberto: "Trade aberto",
  alvo: "Saiu no alvo",
  stop: "Saiu no stop",
  prazo: "Saiu pelo prazo",
  cancelado: "Cancelado sem entrar",
};

export function planLines(s: Setup): string[] {
  const lines = [
    `Entrada ${price(s.entry)} · Stop ${price(s.stop)} (${pct(change(s.entry, s.stop))}) · Alvo ${price(s.target)} (${pct(change(s.entry, s.target))})`,
    `Ganho possível: ${s.rr.toLocaleString("pt-BR", { maximumFractionDigits: 1 })} vezes o risco.`,
  ];
  if (s.status === "armado") lines.push(`A ordem vale até ${when(s.valid_until_ms)}.`);
  if (s.fill_ms) lines.push(`Entrou em ${when(s.fill_ms)}.`);
  if (s.exit_ms && s.exit_price != null && s.status !== "cancelado") {
    const dir = s.side === "long" ? 1 : -1;
    lines.push(`Saiu em ${when(s.exit_ms)} a ${price(s.exit_price)}: ${pct(dir * change(s.entry, s.exit_price))} no trade.`);
  }
  return lines;
}

export function sweepStory(p: Pick<Pending, "side" | "swept_level" | "sweep_extreme">): string {
  return p.side === "long"
    ? `Passou abaixo do fundo de ${price(p.swept_level)} (stops de quem estava comprado), foi até ${price(p.sweep_extreme)} e voltou.`
    : `Passou acima do topo de ${price(p.swept_level)} (stops de quem estava vendido), foi até ${price(p.sweep_extreme)} e voltou.`;
}

export function confirmLine(p: Pending): string {
  const dir = p.side === "long" ? "acima" : "abaixo";
  const dead = p.side === "long" ? "abaixo" : "acima";
  return `Vira setup se um candle fechar ${dir} de ${price(p.choch_level)} até ${when(p.deadline_ms)}. Morre se o preço for ${dead} de ${price(p.sweep_extreme)}.`;
}

export const SECTION_HEADINGS = [
  "Leitura do dia",
  "O que aconteceu",
  "Alavancagem",
  "Onde estão as liquidações",
  "Planos",
  "Funding",
  "Agenda",
  "Conta fictícia",
  "Para você",
  "Smart Money",
  "Contas fictícias",
  "Trades abertos",
  "Ordens armadas",
  "Varridas esperando",
  "Liquidez mais perto",
];

export function isHeading(line: string): boolean {
  return SECTION_HEADINGS.some((h) => line.startsWith(h));
}

export function dayReading(sections: string[]): { label: string; why: string } | null {
  const first = sections[0]?.split("\n") ?? [];
  const head = first.find((l) => l.startsWith("Leitura do dia"));
  if (!head) return null;
  const label = head.split(": ").pop() ?? "";
  const why = (first.find((l) => l.startsWith("Motivo:")) ?? "").replace(/^Motivo:\s*/, "");
  return { label, why };
}
