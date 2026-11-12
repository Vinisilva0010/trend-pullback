import { NextResponse } from "next/server";
import { env } from "@/lib/env";
import { bearer, sameSecret } from "@/lib/secrets";
import { watchActive } from "@/lib/watch";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

// The tool on the computer asks this (with the ingest token) before doing
// the intraday stock watch itself.
export async function GET(request: Request) {
  const expected = env("INGEST_TOKEN");
  if (expected.length < 32 || !sameSecret(bearer(request.headers.get("authorization")), expected)) {
    return NextResponse.json({ error: "unauthorized" }, { status: 401 });
  }
  return NextResponse.json({ active: await watchActive(Date.now()) });
}
