import { NextResponse } from "next/server";
import { IngestSchema } from "@/lib/schema";
import { KEYS, store } from "@/lib/store";
import { notifyAll } from "@/lib/push";
import { bearer, sameSecret } from "@/lib/secrets";
import { env } from "@/lib/env";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

const MAX_BYTES = 4_000_000;

// Called by the tool on the computer after each run, with the full state.
export async function POST(request: Request) {
  const token = bearer(request.headers.get("authorization"));
  const expected = env("INGEST_TOKEN");
  if (expected.length < 32 || !sameSecret(token, expected)) {
    return NextResponse.json({ error: "unauthorized" }, { status: 401 });
  }
  const length = Number(request.headers.get("content-length") ?? "0");
  if (length > MAX_BYTES) return NextResponse.json({ error: "too large" }, { status: 413 });
  const text = await request.text();
  if (text.length > MAX_BYTES) return NextResponse.json({ error: "too large" }, { status: 413 });
  let body: unknown;
  try {
    body = JSON.parse(text);
  } catch {
    return NextResponse.json({ error: "invalid json" }, { status: 400 });
  }
  const parsed = IngestSchema.safeParse(body);
  if (!parsed.success) {
    return NextResponse.json({ error: "invalid data", issues: parsed.error.issues.slice(0, 5) }, { status: 400 });
  }
  const { kind, data, new_events } = parsed.data;
  const key = kind === "live" ? KEYS.live : kind === "daily" ? KEYS.daily : KEYS.markets;
  const s = store();
  await s.set(key, JSON.stringify(data));
  if (parsed.data.kind === "live") {
    await s.set(KEYS.liveMs, String(parsed.data.data.generated_ms));
    const stocks = parsed.data.data.books.find((b) => b.tf === "acoes");
    if (stocks) await s.set(KEYS.watchAccount, String(stocks.equity));
  } else if (parsed.data.kind === "markets") {
    await s.set(KEYS.watchList, JSON.stringify(parsed.data.data.watch));
  }
  const sent = await notifyAll(new_events);
  return NextResponse.json({ ok: true, notifications: sent });
}
