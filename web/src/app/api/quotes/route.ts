import { NextResponse } from "next/server";
import { isLoggedIn } from "@/lib/auth";
import { parseQuote, SYMBOL, type Quote } from "@/lib/quotes";

export const runtime = "nodejs";

const MAX_SYMBOLS = 60;
const PARALLEL = 6;
// Yahoo answers 429 to clients without a browser-like user agent.
const USER_AGENT = "Mozilla/5.0 (X11; Linux x86_64) trend-pullback/0.1";

async function one(symbol: string): Promise<Quote | null> {
  const url = `https://query1.finance.yahoo.com/v8/finance/chart/${encodeURIComponent(symbol)}?range=5d&interval=1d`;
  try {
    // Each symbol is cached for a minute, so many open pages cost one request.
    const r = await fetch(url, { headers: { "user-agent": USER_AGENT }, next: { revalidate: 60 }, signal: AbortSignal.timeout(8000) });
    if (!r.ok) return null;
    return parseQuote(symbol, await r.json(), Date.now());
  } catch {
    return null;
  }
}

// Last prices for the Mercados tab, for logged-in users only.
export async function GET(request: Request) {
  if (!(await isLoggedIn())) return NextResponse.json({ error: "unauthorized" }, { status: 401 });
  const raw = new URL(request.url).searchParams.get("symbols") ?? "";
  const symbols = [...new Set(raw.split(",").map((s) => s.trim()).filter(Boolean))];
  if (symbols.length === 0 || symbols.length > MAX_SYMBOLS || !symbols.every((s) => SYMBOL.test(s))) {
    return NextResponse.json({ error: "invalid" }, { status: 400 });
  }
  const out: Quote[] = [];
  for (let i = 0; i < symbols.length; i += PARALLEL) {
    const got = await Promise.all(symbols.slice(i, i + PARALLEL).map(one));
    out.push(...got.filter((q): q is Quote => q != null));
  }
  return NextResponse.json({ quotes: out }, { headers: { "cache-control": "private, max-age=30" } });
}
