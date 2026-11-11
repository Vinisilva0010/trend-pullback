import { NextResponse } from "next/server";
import { isLoggedIn } from "@/lib/auth";

export const runtime = "nodejs";

const SYMBOLS = new Set(["BTCUSDT", "ETHUSDT", "SOLUSDT"]);
const INTERVALS = new Set(["1h", "4h", "1d", "1w", "1M"]);

// Fallback when the browser cannot reach Binance's public market data directly.
export async function GET(request: Request) {
  if (!(await isLoggedIn())) return NextResponse.json({ error: "unauthorized" }, { status: 401 });
  const url = new URL(request.url);
  const symbol = url.searchParams.get("symbol") ?? "";
  const interval = url.searchParams.get("interval") ?? "";
  if (!SYMBOLS.has(symbol) || !INTERVALS.has(interval)) {
    return NextResponse.json({ error: "invalid" }, { status: 400 });
  }
  const upstream = await fetch(
    `https://data-api.binance.vision/api/v3/klines?symbol=${symbol}&interval=${interval}&limit=500`,
    { next: { revalidate: 30 }, signal: AbortSignal.timeout(10_000) },
  ).catch(() => null);
  if (!upstream?.ok) return NextResponse.json({ error: "upstream" }, { status: 502 });
  return new NextResponse(await upstream.text(), {
    headers: { "content-type": "application/json", "cache-control": "private, max-age=30" },
  });
}
