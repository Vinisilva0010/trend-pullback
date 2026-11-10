import type { Daily, MarketAsset, MarketMood } from "./schema";
import { newsReading, priceReading, verdict } from "./mood";
import { pct } from "./format";

// The day's reading at the top of the home page, for crypto and US stocks:
// one word and the reasons behind it, in plain sentences.

export type Brief = { label: string; tone: "up" | "down" | "mixed" | "flat"; lines: string[] };

const TREND_PT: Record<string, string> = { alta: "alta", queda: "queda", baixa: "queda", lateral: "lateral", indefinida: "indefinida" };

function newsLine(m: MarketMood | null | undefined): string | null {
  if (!m || m.up + m.down + m.unclear === 0) return null;
  const top = m.items[0];
  const lead = `Notícias das últimas ${m.hours} h: ${m.up} puxando para alta e ${m.down} para queda.`;
  return top ? `${lead} A mais importante: "${top.title}" (${top.source}).` : lead;
}

const READING_TONE: Record<string, Brief["tone"]> = { CUIDADO: "down", OPORTUNIDADE: "up", NEUTRO: "flat" };

export function cryptoBrief(daily: Daily | null, reading: { label: string; why: string } | null, mood: MarketMood | null | undefined): Brief {
  const lines: string[] = [];
  if (reading?.why) lines.push(`${reading.why.charAt(0).toUpperCase()}${reading.why.slice(1)}${reading.why.endsWith(".") ? "" : "."}`);
  const assets = daily?.assets ?? [];
  const turned = assets.filter((a) => a.previous_trend && a.previous_trend !== a.trend);
  for (const a of turned) lines.push(`${a.name} mudou de tendência: era ${TREND_PT[a.previous_trend!] ?? a.previous_trend} e agora é ${TREND_PT[a.trend] ?? a.trend}.`);
  if (assets.length) {
    lines.push(`Tendência agora: ${assets.map((a) => `${a.name} ${TREND_PT[a.trend] ?? a.trend}`).join(", ")}. No último dia: ${assets.map((a) => `${a.name} ${pct(a.change)}`).join(", ")}.`);
  }
  const crowded = assets.find((a) => a.positioning && (a.positioning.long_share >= 0.6 || a.positioning.long_share <= 0.4));
  if (crowded?.positioning) {
    const share = Math.round(crowded.positioning.long_share * 100);
    lines.push(
      share >= 60
        ? `${crowded.name}: ${share}% do dinheiro em futuros está apostando na alta, com alavancagem. Se o preço cair, essas posições são fechadas à força e aceleram a queda.`
        : `${crowded.name}: só ${share}% do dinheiro em futuros está apostando na alta; a maioria aposta na queda. Se o preço subir, essas posições são fechadas à força e aceleram a alta.`,
    );
  }
  const n = newsLine(mood);
  if (n) lines.push(n);
  const label = reading?.label ?? "SEM RELATÓRIO";
  return { label, tone: READING_TONE[label] ?? "flat", lines };
}

export function stocksBrief(us: MarketAsset[], mood: MarketMood | null | undefined, openSignals: number): Brief {
  const lines: string[] = [];
  const spy = us.find((a) => a.symbol === "SPY");
  const qqq = us.find((a) => a.symbol === "QQQ");
  if (spy) lines.push(`S&P 500 (SPY): tendência de ${TREND_PT[spy.trend]} no diário e ${TREND_PT[spy.weekly_trend]} no semanal; ${pct(spy.change_day)} no último pregão e ${pct(spy.change_week)} na semana.`);
  if (qqq) lines.push(`Nasdaq (QQQ), as empresas de tecnologia: tendência de ${TREND_PT[qqq.trend]}; ${pct(qqq.change_day)} no último pregão.`);
  const up = us.filter((a) => a.trend === "alta").length;
  const down = us.filter((a) => a.trend === "baixa").length;
  if (us.length) lines.push(`Das ${us.length} ações e ETFs acompanhados, ${up} estão em tendência de alta e ${down} em queda.`);
  const n = newsLine(mood);
  if (n) lines.push(n);
  lines.push(openSignals ? `${openSignals} ${openSignals === 1 ? "sinal de compra aberto" : "sinais de compra abertos"} agora, na aba Ações.` : "Nenhum sinal de compra aberto agora.");
  const v = verdict(
    newsReading(mood).reading,
    priceReading({ up, down, total: us.length, change: spy?.change_day ?? null, scale: 1 }).reading,
  );
  return { label: v.label.toUpperCase(), tone: v.tone, lines };
}
