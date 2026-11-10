"use client";

import { createContext, useContext, useEffect, useState } from "react";
import type { Quote } from "@/lib/quotes";
import { CandleChart } from "@/components/CandleChart";
import { change, pct, price, when } from "@/lib/format";

const REFRESH_MS = 60_000;
const BATCH = 60;

const QuotesContext = createContext<Record<string, Quote>>({});

/**
 * Fetches the last prices of `symbols` every minute while the page is
 * visible and hands them to the quote components inside.
 */
export function LiveQuotes({ symbols, children }: { symbols: string[]; children: React.ReactNode }) {
  const [quotes, setQuotes] = useState<Record<string, Quote>>({});
  const key = symbols.join(",");
  useEffect(() => {
    let stopped = false;
    const list = key ? key.split(",") : [];
    const load = async () => {
      if (document.visibilityState !== "visible" || list.length === 0) return;
      const next: Record<string, Quote> = {};
      for (let i = 0; i < list.length; i += BATCH) {
        try {
          const r = await fetch(`/api/quotes?symbols=${encodeURIComponent(list.slice(i, i + BATCH).join(","))}`, { cache: "no-store" });
          if (!r.ok) continue;
          const body = (await r.json()) as { quotes: Quote[] };
          for (const q of body.quotes) next[q.symbol] = q;
        } catch {
          // The page keeps the last close the tool sent.
        }
      }
      if (!stopped) setQuotes((old) => ({ ...old, ...next }));
    };
    void load();
    const timer = window.setInterval(load, REFRESH_MS);
    const onVisible = () => void load();
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      stopped = true;
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [key]);
  return <QuotesContext.Provider value={quotes}>{children}</QuotesContext.Provider>;
}

export function useQuote(symbol: string): Quote | undefined {
  return useContext(QuotesContext)[symbol];
}

/** Change of the day from the previous close, live when a quote arrived. */
export function dayChange(q: Quote | undefined, fallback: number | null): number | null {
  if (q?.prev_close) return change(q.prev_close, q.price);
  return fallback;
}

/** "ao vivo", "atraso de 15 min" or "fechado", with the time of the last price. */
export function QuoteStatus({ q }: { q: Quote | undefined }) {
  if (!q) return <span className="text-xs font-bold">último fechamento</span>;
  const label = !q.open ? `fechado, último preço ${when(q.time_ms)}` : q.delay_min > 0 ? `atraso de ${q.delay_min} min` : "ao vivo";
  return <span className={`text-xs font-bold ${q.open ? "text-up" : ""}`}>{q.open ? "● " : ""}{label}</span>;
}

/** Price and day change in a list row. */
export function RowQuote({ symbol, fallback, fallbackChange }: { symbol: string; fallback: number | null; fallbackChange: number | null }) {
  const q = useQuote(symbol);
  const ch = dayChange(q, fallbackChange);
  const color = ch == null ? "" : ch > 0 ? "text-up" : ch < 0 ? "text-down" : "";
  return (
    <>
      <span className="num text-lg font-black">{price(q?.price ?? fallback)}</span>
      <span className={`num font-extrabold ${color}`}>
        {pct(ch)} <span className="font-bold text-ink">dia</span>
      </span>
      <QuoteStatus q={q} />
    </>
  );
}

/** Big price line on an asset page. */
export function BigQuote({ symbol, fallback, fallbackChange }: { symbol: string; fallback: number | null; fallbackChange: number | null }) {
  const q = useQuote(symbol);
  const ch = dayChange(q, fallbackChange);
  return (
    <p className="num text-2xl font-black">
      {price(q?.price ?? fallback)} <span className={(ch ?? 0) >= 0 ? "text-up" : "text-down"}>{pct(ch)}</span>{" "}
      <span className="text-base font-bold">no dia</span> <QuoteStatus q={q} />
    </p>
  );
}

/** How far the price is from a Stormer signal's trigger, or its trade's stop and target. */
export function TriggerStatus({ symbol, status, trigger, stop, target }: { symbol: string; status: string; trigger: number; stop: number; target: number }) {
  const q = useQuote(symbol);
  if (!q || (status !== "sinal" && status !== "aberto")) return null;
  const high = q.day?.[2] ?? q.price;
  const low = q.day?.[3] ?? q.price;
  let text: string;
  let strong = false;
  if (status === "sinal") {
    if (high >= trigger) {
      strong = true;
      text = `Passou do gatilho hoje (máxima ${price(high)}). Preço agora ${price(q.price)}, ${pct(change(trigger, q.price))} do gatilho.`;
    } else {
      text = `Preço agora ${price(q.price)}: falta ${pct(change(q.price, trigger), false)} para o gatilho.`;
    }
  } else if (low <= stop) {
    strong = true;
    text = `Tocou no stop hoje (mínima ${price(low)}). Preço agora ${price(q.price)}.`;
  } else if (high >= target) {
    strong = true;
    text = `Tocou no alvo hoje (máxima ${price(high)}). Preço agora ${price(q.price)}.`;
  } else {
    text = `Preço agora ${price(q.price)}: stop ${pct(change(q.price, stop), false)} abaixo, alvo ${pct(change(q.price, target), false)} acima.`;
  }
  return (
    <p className={`mt-2 rounded p-2 ${strong ? "bg-accent font-black" : "bg-panel font-extrabold"}`}>
      {text} <QuoteStatus q={q} />
    </p>
  );
}

/** The asset chart with today's candle moving (daily chart only). */
export function LiveMarketChart(props: Omit<React.ComponentProps<typeof CandleChart>, "live">) {
  const q = useQuote(props.symbol);
  return <CandleChart {...props} live={props.tf === "1d" ? (q?.day ?? null) : null} />;
}
