import { NextResponse } from "next/server";
import { isLoggedIn } from "@/lib/auth";
import { parseQuote, SYMBOL, type Quote } from "@/lib/quotes";

export const runtime = "nodejs";

const MAX_SYMBOLS = 60;
const PARALLEL = 6;
// Yahoo answers 429 to clients without a browser-like user agent.
const USER_AGENT = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";
// Two hosts of the same data: when one refuses a cloud server, the other often answers.
const HOSTS = ["https://query1.finance.yahoo.com", "https://query2.finance.yahoo.com"];

async function one(symbol: string): Promise<Quote | string> {
  let reason = "sem resposta";
  for (const host of HOSTS) {
    const url = `${host}/v8/finance/chart/${encodeURIComponent(symbol)}?range=1mo&interval=1d`;
    try {
      // Each symbol is cached for a minute, so many open pages cost one request.
      const r = await fetch(url, { headers: { "user-agent": USER_AGENT, accept: "application/json" }, next: { revalidate: 60 }, signal: AbortSignal.timeout(8000) });
      if (!r.ok) {
        reason = `a fonte respondeu ${r.status}`;
        continue;
      }
      const q = parseQuote(symbol, await r.json(), Date.now());
      if (q) return q;
      reason = "resposta sem preço";
    } catch {
      reason = "a fonte não respondeu a tempo";
    }
  }
  return reason;
}

// Last prices for the Mercados tab, for logged-in users only.
export async function GET(request: Request) {
  if (!(await isLoggedIn())) return NextResponse.json({ error: "unauthorized" }, { status: 401 });
  const raw = new URL(request.url).searchParams.get("symbols") ?? "";
  const symbols = [...new Set(raw.split(",").map((s) => s.trim()).filter(Boolean))];
  if (symbols.length === 0 || symbols.length > MAX_SYMBOLS || !symbols.every((s) => SYMBOL.test(s))) {
    return NextResponse.json({ error: "invalid" }, { status: 400 });
  }
  const quotes: Quote[] = [];
  const failed: { symbol: string; reason: string }[] = [];
  for (let i = 0; i < symbols.length; i += PARALLEL) {
    const batch = symbols.slice(i, i + PARALLEL);
    const got = await Promise.all(batch.map(one));
    got.forEach((g, j) => (typeof g === "string" ? failed.push({ symbol: batch[j]!, reason: g }) : quotes.push(g)));
  }
  return NextResponse.json({ quotes, failed }, { headers: { "cache-control": "private, max-age=30" } });
}
