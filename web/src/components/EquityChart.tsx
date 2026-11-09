"use client";

import { ColorType, createChart, LineSeries, type UTCTimestamp } from "lightweight-charts";
import { useEffect, useRef } from "react";
import { money } from "@/lib/format";

/** Balance of a paper book after each closed trade. */
export function EquityChart({ points }: { points: { ms: number; value: number }[] }) {
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = box.current;
    if (!el || points.length < 2) return;
    const chart = createChart(el, {
      autoSize: true,
      layout: { background: { type: ColorType.Solid, color: "#ffffff" }, textColor: "#000000", fontSize: 13 },
      grid: { vertLines: { visible: false }, horzLines: { color: "#efefef" } },
      rightPriceScale: { borderColor: "#000000" },
      timeScale: { borderColor: "#000000", timeVisible: true },
      localization: { locale: "pt-BR", priceFormatter: (p: number) => money(p) },
      handleScroll: false,
      handleScale: false,
    });
    const line = chart.addSeries(LineSeries, { color: "#000000", lineWidth: 3 });
    // One point per second at most: the library needs strictly increasing times.
    const data: { time: UTCTimestamp; value: number }[] = [];
    for (const p of points) {
      let t = Math.floor(p.ms / 1000) - 3 * 3600;
      const last = data.at(-1);
      if (last && t <= last.time) t = last.time + 1;
      data.push({ time: t as UTCTimestamp, value: p.value });
    }
    line.setData(data);
    chart.timeScale().fitContent();
    return () => chart.remove();
  }, [points]);
  if (points.length < 2) {
    return <p className="rounded bg-panel p-3">O gráfico aparece depois do primeiro trade fechado.</p>;
  }
  return <div ref={box} className="h-56 w-full rounded border-2 border-ink" />;
}
