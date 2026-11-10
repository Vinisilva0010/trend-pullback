"use client";

import { useQuote, dayChange } from "@/components/LiveQuotes";
import { newsReading, priceReading, READING_TEXT, verdict, type Reading } from "@/lib/mood";
import type { MarketMood } from "@/lib/schema";
import { pct } from "@/lib/format";

const TONE: Record<string, string> = {
  up: "bg-up text-white",
  down: "bg-down text-white",
  mixed: "bg-accent text-ink",
  flat: "bg-panel text-ink",
};

/** A bar from queda (left) to alta (right) with a mark at the score. */
function Gauge({ label, score, reading, detail }: { label: string; score: number; reading: Reading; detail: string }) {
  const left = `${Math.round((score + 1) * 50)}%`;
  return (
    <div className="mt-3">
      <p className="font-extrabold">
        {label}: <span className="font-black">{READING_TEXT[reading]}</span>
      </p>
      <div className="relative mt-1 h-4 rounded border-2 border-ink" style={{ background: "linear-gradient(90deg, #d9304a 0%, #f3f3f3 50%, #0b8f5a 100%)" }}>
        {reading !== "pouca" ? <span className="absolute -top-1.5 h-6 w-1.5 -translate-x-1/2 rounded bg-ink" style={{ left }} /> : null}
      </div>
      <div className="mt-0.5 flex justify-between text-xs font-extrabold">
        <span>Queda</span>
        <span>Alta</span>
      </div>
      <p className="text-sm">{detail}</p>
    </div>
  );
}

export type PriceSide = { up: number; down: number; total: number; change: number | null; scale: number; changeLabel: string; quoteSymbol?: string };

export function Thermometer({ title, mood, price }: { title: string; mood: MarketMood | null; price: PriceSide }) {
  const q = useQuote(price.quoteSymbol ?? "");
  const change = price.quoteSymbol ? dayChange(q, price.change) : price.change;
  const n = newsReading(mood);
  const p = priceReading({ ...price, change });
  const v = verdict(n.reading, p.reading);
  const newsDetail = mood
    ? `${mood.up} puxando para alta, ${mood.down} para queda e ${mood.unclear} sem direção nas últimas ${mood.hours} h. Leitura pelas palavras das manchetes: serve de termômetro, não de sinal.`
    : "Ainda sem notícias lidas.";
  const priceDetail = `${price.up} de ${price.total} em tendência de alta e ${price.down} em queda (médias de 8 e 80 dias); ${price.changeLabel} ${pct(change)}.`;
  return (
    <section className="rounded border-2 border-ink p-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-xl font-black">{title}</h3>
        <span className={`rounded px-3 py-1 text-xl font-black ${TONE[v.tone]}`}>{v.label}</span>
      </div>
      <Gauge label="Notícias" score={n.score} reading={n.reading} detail={newsDetail} />
      <Gauge label="Preço" score={p.score} reading={p.reading} detail={priceDetail} />
    </section>
  );
}
