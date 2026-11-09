"use client";

import { useEffect, useState } from "react";
import { pct, price } from "@/lib/format";

type Tick = { price: number; change: number };

/**
 * Live price and 24h change straight from Binance's public stream; falls
 * back to the last value the tool published when the stream is unavailable.
 */
export function LivePrice({ symbol, fallback, fallbackChange }: { symbol: string; fallback: number | null; fallbackChange: number | null }) {
  const [tick, setTick] = useState<Tick | null>(null);
  useEffect(() => {
    let ws: WebSocket | null = null;
    let retry: number | undefined;
    let closed = false;
    const connect = () => {
      ws = new WebSocket(`wss://data-stream.binance.vision/ws/${symbol.toLowerCase()}@miniTicker`);
      ws.onmessage = (event) => {
        const data = JSON.parse(String(event.data)) as { c?: string; o?: string };
        const c = Number(data.c);
        const o = Number(data.o);
        if (Number.isFinite(c) && Number.isFinite(o) && o > 0) setTick({ price: c, change: (c / o - 1) * 100 });
      };
      ws.onclose = () => {
        if (!closed) retry = window.setTimeout(connect, 5000);
      };
    };
    connect();
    return () => {
      closed = true;
      window.clearTimeout(retry);
      ws?.close();
    };
  }, [symbol]);
  const value = tick?.price ?? fallback;
  const ch = tick?.change ?? fallbackChange;
  const up = (ch ?? 0) >= 0;
  return (
    <div className="flex items-baseline gap-3">
      <span className="num text-3xl font-black">{price(value)}</span>
      <span className={`num rounded px-2 py-0.5 text-base font-extrabold ${up ? "bg-up-soft text-up" : "bg-down-soft text-down"}`}>
        {pct(ch)} <span className="font-bold">24h</span>
      </span>
      {tick ? (
        <span className="text-xs font-bold text-up" title="Preço ao vivo">
          ● ao vivo
        </span>
      ) : null}
    </div>
  );
}
