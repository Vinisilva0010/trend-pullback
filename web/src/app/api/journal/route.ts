import { NextResponse } from "next/server";
import { isLoggedIn } from "@/lib/auth";
import { CloseSchema, DeleteSchema, OpenSchema } from "@/lib/journal";
import { addTrade, closeStored, listTrades, removeTrade } from "@/lib/journalStore";
import { KEYS, store } from "@/lib/store";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

/** Most changes per hour; a typing mistake loop never fills the store. */
const MAX_WRITES = 200;
const MAX_BODY = 8_000;

function fail(error: string, status: number) {
  return NextResponse.json({ error }, { status });
}

/** Logged in, same site, JSON, small body, under the hourly limit. */
async function readWrite(request: Request): Promise<unknown | NextResponse> {
  if (!(await isLoggedIn())) return fail("unauthorized", 401);
  const origin = request.headers.get("origin");
  let sameSite = false;
  try {
    sameSite = origin != null && new URL(origin).host === request.headers.get("host");
  } catch {
    sameSite = false;
  }
  if (!sameSite) return fail("bad origin", 403);
  if (!request.headers.get("content-type")?.startsWith("application/json")) return fail("json only", 415);
  const text = await request.text();
  if (text.length > MAX_BODY) return fail("too large", 413);
  if ((await store().hit(KEYS.journalHits, 3600)) > MAX_WRITES) return fail("Muitas alterações nesta hora. Tente mais tarde.", 429);
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return fail("invalid json", 400);
  }
}

function issues(error: { issues: { path: PropertyKey[]; message: string }[] }): string {
  return error.issues.map((i) => (i.path.length ? `${i.path.join(".")}: ${i.message}` : i.message)).join("; ");
}

export async function GET() {
  if (!(await isLoggedIn())) return fail("unauthorized", 401);
  return NextResponse.json({ trades: await listTrades() });
}

export async function POST(request: Request) {
  const body = await readWrite(request);
  if (body instanceof NextResponse) return body;
  const parsed = OpenSchema.safeParse(body);
  if (!parsed.success) return fail(issues(parsed.error), 400);
  const out = await addTrade(parsed.data);
  return typeof out === "string" ? fail(out, 409) : NextResponse.json({ trade: out });
}

export async function PATCH(request: Request) {
  const body = await readWrite(request);
  if (body instanceof NextResponse) return body;
  const parsed = CloseSchema.safeParse(body);
  if (!parsed.success) return fail(issues(parsed.error), 400);
  const out = await closeStored(parsed.data);
  return typeof out === "string" ? fail(out, 409) : NextResponse.json({ trade: out });
}

export async function DELETE(request: Request) {
  const body = await readWrite(request);
  if (body instanceof NextResponse) return body;
  const parsed = DeleteSchema.safeParse(body);
  if (!parsed.success) return fail(issues(parsed.error), 400);
  await removeTrade(parsed.data.id);
  return NextResponse.json({ ok: true });
}
