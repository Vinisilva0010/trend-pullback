"use client";

import {
  CandlestickSeries,
  ColorType,
  createChart,
  createSeriesMarkers,
  CrosshairMode,
  HistogramSeries,
  LineStyle,
  type IChartApi,
  type IPriceLine,
  type ISeriesApi,
  type MouseEventParams,
  type SeriesMarker,
  type Time,
  type UTCTimestamp,
} from "lightweight-charts";
import { useEffect, useMemo, useRef, useState } from "react";
import type { CycleLine, Frame } from "@/lib/schema";
import { change, pct, price } from "@/lib/format";

// Candles from Binance's public API (live through its websocket) with the
// tool's levels drawn on top: active setup, sweeps waiting, liquidity and
// estimated liquidation zones.

type Zone = { low: number; high: number; usd: number } | null;
type Level = { price: number; color: "entry" | "down" | "up"; title: string };
type Props = {
  symbol: string;
  tf: "1h" | "4h" | "1d" | "1w" | "1M";
  /** Candles given by the page ([open ms, open, high, low, close]); no live feed then. */
  candles?: [number, number, number, number, number][];
  frame: Frame | null;
  /** Stormer signal or open trade levels for this chart. */
  extra?: Level[];
  longsZone: Zone;
  shortsZone: Zone;
  /** Cycle lines: usual depth of falls, usual height of bounces, the top of the fall. */
  cycle?: CycleLine[];
  /** Today's candle so far, drawn over the last given candle (stocks). */
  live?: [number, number, number, number, number] | null;
};

type Point = { logical: number; price: number };
type Measure = { a: Point | null; b: Point | null; fixed: boolean };

const BRT_OFFSET = -3 * 3600;
const STEP: Record<Props["tf"], number> = { "1h": 3600, "4h": 14400, "1d": 86400, "1w": 604800, "1M": 2_592_000 };

const LIGHT = {
  up: "#0b8f5a",
  down: "#d9304a",
  entry: "#c99700",
  liquidity: "#000000",
  confirm: "#1f5fd1",
  fall: "#7a2fb5",
  bounce: "#b5531f",
  turn: "#0b8f5a",
  top: "#000000",
};

/** Black background: brighter colors so lines and labels stand out. */
const DARK: typeof LIGHT = {
  up: "#22d38b",
  down: "#ff4d6a",
  entry: "#ffd23f",
  liquidity: "#ffffff",
  confirm: "#5aa2ff",
  fall: "#c58bff",
  bounce: "#ff9a4d",
  turn: "#22d38b",
  top: "#ffffff",
};

const THEME = {
  light: { bg: "#ffffff", text: "#000000", grid: "#efefef", border: "#000000" },
  dark: { bg: "#000000", text: "#ffffff", grid: "#1f2329", border: "#8a8f98" },
};

const DARK_KEY = "tp:chart:dark";

/** Chart time in Brasília (the library draws UTC). */
function chartTime(ms: number): UTCTimestamp {
  return (Math.floor(ms / 1000) + BRT_OFFSET) as UTCTimestamp;
}

type Kline = [number, string, string, string, string, string, ...unknown[]];

async function loadCandles(symbol: string, tf: string): Promise<Kline[]> {
  const direct = `https://data-api.binance.vision/api/v3/klines?symbol=${symbol}&interval=${tf}&limit=500`;
  try {
    const r = await fetch(direct, { cache: "no-store" });
    if (r.ok) return (await r.json()) as Kline[];
  } catch {
    // Falls back to the server route below.
  }
  const r = await fetch(`/api/klines?symbol=${symbol}&interval=${tf}`, { cache: "no-store" });
  if (!r.ok) throw new Error("sem dados de preço");
  return (await r.json()) as Kline[];
}

const NO_CYCLE: CycleLine[] = [];

export function CandleChart({ symbol, tf, frame, longsZone, shortsZone, extra = [], cycle = NO_CYCLE, candles: given, live = null }: Props) {
  const box = useRef<HTMLDivElement>(null);
  const chartRef = useRef<IChartApi | null>(null);
  const seriesRef = useRef<ISeriesApi<"Candlestick"> | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Ruler: two taps (or clicks) on the chart measure the move between them.
  const [ruler, setRuler] = useState(false);
  const [pts, setPts] = useState<Measure>({ a: null, b: null, fixed: false });
  const [, setTick] = useState(0);
  const liveRef = useRef(live);
  liveRef.current = live;
  const drawLive = () => {
    const c = liveRef.current;
    const series = seriesRef.current;
    if (!c || !series) return;
    const last = series.data().at(-1);
    // Only today's candle or a newer one: older times cannot be updated.
    if (last && Number(last.time) > Number(chartTime(c[0]))) return;
    try {
      series.update({ time: chartTime(c[0]), open: c[1], high: c[2], low: c[3], close: c[4] });
    } catch {
      // A candle the chart cannot take is skipped; the next quote tries again.
    }
  };
  const [layers, setLayers] = useState({ setup: true, liquidity: true, liquidations: true, history: true, cycle: true });
  // Black background (remembered on this device) and full screen.
  const [dark, setDark] = useState(false);
  const [full, setFull] = useState(false);
  const wrap = useRef<HTMLDivElement>(null);
  const COLORS = dark ? DARK : LIGHT;
  useEffect(() => {
    try {
      setDark(window.localStorage.getItem(DARK_KEY) === "1");
    } catch {
      // No storage: the white background stays.
    }
  }, []);
  const switchDark = () =>
    setDark((d) => {
      try {
        window.localStorage.setItem(DARK_KEY, d ? "0" : "1");
      } catch {
        // Not remembered; the switch still works now.
      }
      return !d;
    });
  const switchFull = () => {
    const el = wrap.current;
    if (!full) {
      setFull(true);
      // Real full screen where the browser allows it (computers, Android);
      // on the iPhone the chart just covers the whole page.
      el?.requestFullscreen?.().catch(() => undefined);
    } else {
      setFull(false);
      if (document.fullscreenElement) document.exitFullscreen().catch(() => undefined);
    }
  };
  useEffect(() => {
    const onChange = () => {
      if (!document.fullscreenElement) setFull(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setFull(false);
    };
    document.addEventListener("fullscreenchange", onChange);
    window.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("fullscreenchange", onChange);
      window.removeEventListener("keydown", onKey);
    };
  }, []);

  // Chart and candles: rebuilt when the asset or timeframe changes.
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    setError(null);
    const chart = createChart(el, {
      autoSize: true,
      layout: { background: { type: ColorType.Solid, color: THEME.light.bg }, textColor: THEME.light.text, fontSize: 13, fontFamily: "system-ui, sans-serif" },
      grid: { vertLines: { color: THEME.light.grid }, horzLines: { color: THEME.light.grid } },
      crosshair: { mode: CrosshairMode.Normal },
      rightPriceScale: { borderColor: THEME.light.border },
      timeScale: { borderColor: THEME.light.border, timeVisible: tf === "1h" || tf === "4h", secondsVisible: false, rightOffset: 6 },
      localization: { locale: "pt-BR", priceFormatter: (p: number) => price(p) },
    });
    const candles = chart.addSeries(CandlestickSeries, {
      upColor: LIGHT.up,
      downColor: LIGHT.down,
      borderVisible: false,
      wickUpColor: LIGHT.up,
      wickDownColor: LIGHT.down,
      priceLineColor: "#000000",
    });
    const volume = chart.addSeries(HistogramSeries, { priceScaleId: "", priceFormat: { type: "volume" }, lastValueVisible: false, priceLineVisible: false });
    volume.priceScale().applyOptions({ scaleMargins: { top: 0.82, bottom: 0 } });
    chartRef.current = chart;
    seriesRef.current = candles;

    let ws: WebSocket | null = null;
    let cancelled = false;
    const source: Promise<Kline[]> = given
      ? Promise.resolve(given.map((c) => [c[0], String(c[1]), String(c[2]), String(c[3]), String(c[4]), "0"] as Kline))
      : loadCandles(symbol, tf);
    source
      .then((rows) => {
        if (cancelled) return;
        candles.setData(rows.map((k) => ({ time: chartTime(k[0]), open: +k[1], high: +k[2], low: +k[3], close: +k[4] })));
        volume.setData(
          rows.map((k) => ({ time: chartTime(k[0]), value: +k[5], color: +k[4] >= +k[1] ? "#0b8f5a55" : "#d9304a55" })),
        );
        chart.timeScale().scrollToRealTime();
        if (given) {
          drawLive();
          return;
        }
        ws = new WebSocket(`wss://data-stream.binance.vision/ws/${symbol.toLowerCase()}@kline_${tf}`);
        ws.onmessage = (event) => {
          const k = (JSON.parse(String(event.data)) as { k?: { t: number; o: string; h: string; l: string; c: string; v: string } }).k;
          if (!k) return;
          const time = chartTime(k.t);
          candles.update({ time, open: +k.o, high: +k.h, low: +k.l, close: +k.c });
          volume.update({ time, value: +k.v, color: +k.c >= +k.o ? "#0b8f5a55" : "#d9304a55" });
        };
      })
      .catch(() => setError("Não consegui carregar os candles agora. Tente de novo em instantes."));
    return () => {
      cancelled = true;
      ws?.close();
      chart.remove();
      chartRef.current = null;
      seriesRef.current = null;
    };
  }, [symbol, tf, given]);

  // Colors of the chart itself follow the background.
  useEffect(() => {
    const chart = chartRef.current;
    const series = seriesRef.current;
    if (!chart || !series) return;
    const t = dark ? THEME.dark : THEME.light;
    const c = dark ? DARK : LIGHT;
    chart.applyOptions({
      layout: { background: { type: ColorType.Solid, color: t.bg }, textColor: t.text },
      grid: { vertLines: { color: t.grid }, horzLines: { color: t.grid } },
      rightPriceScale: { borderColor: t.border },
      timeScale: { borderColor: t.border },
    });
    series.applyOptions({ upColor: c.up, downColor: c.down, wickUpColor: c.up, wickDownColor: c.down, priceLineColor: t.text });
  }, [dark, symbol, tf, given]);

  // Ruler mode: the chart stops scrolling so taps and drags measure instead.
  useEffect(() => {
    const chart = chartRef.current;
    const series = seriesRef.current;
    if (!chart || !series || !ruler) return;
    chart.applyOptions({ handleScroll: false, handleScale: false });
    const toPoint = (param: MouseEventParams<Time>): Point | null => {
      if (!param.point || param.logical == null) return null;
      const p = series.coordinateToPrice(param.point.y);
      return p == null ? null : { logical: Number(param.logical), price: p };
    };
    const onClick = (param: MouseEventParams<Time>) => {
      const p = toPoint(param);
      if (!p) return;
      setPts((cur) => (!cur.a || cur.fixed ? { a: p, b: p, fixed: false } : { ...cur, b: p, fixed: true }));
    };
    const onMove = (param: MouseEventParams<Time>) => {
      const p = toPoint(param);
      if (p) setPts((cur) => (cur.a && !cur.fixed ? { ...cur, b: p } : cur));
    };
    const redraw = () => setTick((t) => t + 1);
    chart.subscribeClick(onClick);
    chart.subscribeCrosshairMove(onMove);
    chart.timeScale().subscribeVisibleLogicalRangeChange(redraw);
    return () => {
      chart.unsubscribeClick(onClick);
      chart.unsubscribeCrosshairMove(onMove);
      chart.timeScale().unsubscribeVisibleLogicalRangeChange(redraw);
      chart.applyOptions({ handleScroll: true, handleScale: true });
    };
  }, [ruler, symbol, tf, given]);

  // A new quote moves today's candle.
  useEffect(() => drawLive(), [live]);

  const active = useMemo(() => frame?.setups.filter((s) => s.status === "armado" || s.status === "aberto") ?? [], [frame]);

  // Levels and markers: redrawn when the data or the chosen layers change.
  useEffect(() => {
    const series = seriesRef.current;
    if (!series) return;
    const lines: IPriceLine[] = [];
    const add = (p: number, color: string, title: string, style: LineStyle, width: 1 | 2 | 3 = 2) =>
      lines.push(series.createPriceLine({ price: p, color, title, lineStyle: style, lineWidth: width, axisLabelVisible: true }));
    if (layers.setup) {
      for (const s of active) {
        const tag = s.candidate ? "" : " (só acompanhar)";
        add(s.entry, COLORS.entry, `Entrada${tag}`, LineStyle.Solid, 3);
        add(s.stop, COLORS.down, "Stop", LineStyle.Solid);
        add(s.target, COLORS.up, "Alvo", LineStyle.Solid);
      }
      for (const p of frame?.pending ?? []) add(p.choch_level, COLORS.confirm, "Confirma o setup", LineStyle.Dashed);
      for (const l of extra) add(l.price, COLORS[l.color], l.title, LineStyle.Solid, l.color === "entry" ? 3 : 2);
    }
    if (layers.liquidity && frame) {
      if (frame.liquidity.above != null) add(frame.liquidity.above, COLORS.liquidity, "Liquidez acima", LineStyle.Dotted, 2);
      if (frame.liquidity.below != null) add(frame.liquidity.below, COLORS.liquidity, "Liquidez abaixo", LineStyle.Dotted, 2);
    }
    if (layers.liquidations) {
      if (longsZone) {
        add(longsZone.high, COLORS.down, "Liquidação comprados", LineStyle.LargeDashed, 2);
        add(longsZone.low, COLORS.down, "", LineStyle.LargeDashed, 2);
      }
      if (shortsZone) {
        add(shortsZone.low, COLORS.up, "Liquidação vendidos", LineStyle.LargeDashed, 2);
        add(shortsZone.high, COLORS.up, "", LineStyle.LargeDashed, 2);
      }
    }
    if (layers.cycle) {
      for (const l of cycle) add(l.price, COLORS[l.kind], l.title, l.kind === "top" ? LineStyle.Solid : LineStyle.Dashed, 2);
    }
    const step = STEP[tf];
    // Candles open on UTC boundaries; snap each mark to its candle's open.
    const snap = (ms: number) => chartTime(Math.floor(ms / (step * 1000)) * step * 1000);
    const markers: SeriesMarker<Time>[] = [];
    if (frame && layers.history) {
      for (const s of frame.setups) {
        const long = s.side === "long";
        markers.push({ time: snap(s.sweep_ms), position: long ? "belowBar" : "aboveBar", color: COLORS.confirm, shape: long ? "arrowUp" : "arrowDown", text: "Varrida" });
        if (s.fill_ms) markers.push({ time: snap(s.fill_ms - step * 1000), position: long ? "belowBar" : "aboveBar", color: COLORS.entry, shape: "circle", text: "Entrou" });
        if (s.exit_ms && s.status !== "cancelado" && s.status !== "aberto") {
          const won = s.status === "alvo" || (s.r ?? 0) > 0;
          markers.push({ time: snap(s.exit_ms - step * 1000), position: long ? "aboveBar" : "belowBar", color: won ? COLORS.up : COLORS.down, shape: "square", text: won ? "Alvo" : s.status === "stop" ? "Stop" : "Prazo" });
        }
      }
      for (const p of frame.pending) {
        const long = p.side === "long";
        markers.push({ time: snap(p.sweep_ms), position: long ? "belowBar" : "aboveBar", color: COLORS.confirm, shape: long ? "arrowUp" : "arrowDown", text: "Varrida" });
      }
    }
    markers.sort((a, b) => Number(a.time) - Number(b.time));
    const markerApi = createSeriesMarkers(series, markers);
    return () => {
      for (const l of lines) series.removePriceLine(l);
      markerApi.detach();
    };
  }, [active, frame, longsZone, shortsZone, layers, tf, symbol, extra, cycle, COLORS]);

  const toggle = (k: keyof typeof layers) => setLayers((v) => ({ ...v, [k]: !v[k] }));
  const LAYER_LABEL: Record<keyof typeof layers, string> = {
    setup: "Setup (entrada, stop, alvo)",
    liquidity: "Liquidez",
    liquidations: "Zonas de liquidação",
    history: "Marcas no gráfico",
    cycle: "Ciclo (quedas e repiques)",
  };

  let overlay: React.ReactNode = null;
  const chart = chartRef.current;
  const series = seriesRef.current;
  if (ruler && pts.a && pts.b && chart && series) {
    const x1 = chart.timeScale().logicalToCoordinate(pts.a.logical as never);
    const x2 = chart.timeScale().logicalToCoordinate(pts.b.logical as never);
    const y1 = series.priceToCoordinate(pts.a.price);
    const y2 = series.priceToCoordinate(pts.b.price);
    if (x1 != null && x2 != null && y1 != null && y2 != null) {
      const up = pts.b.price >= pts.a.price;
      const move = change(pts.a.price, pts.b.price);
      const bars = Math.round(Math.abs(pts.b.logical - pts.a.logical));
      overlay = (
        <>
          <div
            className="pointer-events-none absolute z-10 border-2"
            style={{
              left: Math.min(x1, x2) + 2,
              top: Math.min(y1, y2) + 2,
              width: Math.max(2, Math.abs(x2 - x1)),
              height: Math.max(2, Math.abs(y2 - y1)),
              background: up ? "#0b8f5a33" : "#d9304a33",
              borderColor: up ? COLORS.up : COLORS.down,
            }}
          />
          <div
            className="pointer-events-none absolute z-10 whitespace-nowrap rounded px-2 py-1 text-sm font-black text-white"
            style={{ left: Math.max(4, Math.min(x2 + 8, (box.current?.clientWidth ?? 300) - 190)), top: Math.max(4, y2 - 30), background: up ? COLORS.up : COLORS.down }}
          >
            {pct(move)} · {price(pts.b.price - pts.a.price)} · {bars} {bars === 1 ? "candle" : "candles"}
          </div>
        </>
      );
    }
  }

  const btn = "rounded border-2 border-ink px-2.5 py-1 text-[13px] font-extrabold";
  return (
    <div
      ref={wrap}
      className={full ? "fixed inset-0 z-50 flex flex-col p-2" : ""}
      style={full ? { background: dark ? THEME.dark.bg : THEME.light.bg } : undefined}
    >
      <div className={`relative ${full ? "min-h-0 flex-1" : ""}`}>
        <div ref={box} className={`w-full rounded border-2 ${dark ? "border-[#8a8f98]" : "border-ink"} ${full ? "h-full" : "h-[58vh] min-h-[340px]"}`} />
        {overlay}
      </div>
      <div className="mt-2 flex flex-wrap gap-2">
        <button type="button" onClick={switchFull} aria-pressed={full} className={`${btn} ${full ? "bg-accent text-ink" : "bg-paper text-ink"}`}>
          {full ? "Sair da tela cheia" : "Tela cheia"}
        </button>
        <button type="button" onClick={switchDark} aria-pressed={dark} className={`${btn} bg-paper text-ink`}>
          {dark ? "Fundo branco" : "Fundo preto"}
        </button>
        <button
          type="button"
          onClick={() => {
            setRuler((v) => !v);
            setPts({ a: null, b: null, fixed: false });
          }}
          aria-pressed={ruler}
          className={`rounded border-2 border-ink px-2.5 py-1 text-[13px] font-extrabold ${ruler ? "bg-accent text-ink" : "bg-paper text-ink"}`}
        >
          {ruler ? "Régua ligada (toque em 2 pontos)" : "Régua (medir %)"}
        </button>
        {(Object.keys(layers) as (keyof typeof layers)[]).map((k) => (
          <button
            key={k}
            type="button"
            onClick={() => toggle(k)}
            aria-pressed={layers[k]}
            className={`${btn} ${
              full && dark
                ? layers[k]
                  ? "border-white bg-white text-black"
                  : "border-white bg-black text-white"
                : layers[k]
                  ? "bg-ink text-paper"
                  : "bg-paper text-ink"
            }`}
          >
            {LAYER_LABEL[k]}
          </button>
        ))}
      </div>
      {error ? <p className="mt-2 font-extrabold text-down">{error}</p> : null}
    </div>
  );
}
