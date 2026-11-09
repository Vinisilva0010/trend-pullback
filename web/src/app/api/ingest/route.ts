import { NextResponse } from "next/server";
import { IngestSchema } from "@/lib/schema";
import { KEYS, store } from "@/lib/store";
import { notifyAll } from "@/lib/push";
import { bearer, sameSecret } from "@/lib/secrets";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

const MAX_BYTES = 2_000_000;

// Called by the tool on the computer after each run, with the full state.
export async function POST(request: Request) {
  const token = bearer(request.headers.get("authorization"));
  const expected = process.env.INGEST_TOKEN ?? "";
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
  await store().set(kind === "live" ? KEYS.live : KEYS.daily, JSON.stringify(data));
  const sent = await notifyAll(new_events);
  return NextResponse.json({ ok: true, notifications: sent });
}
