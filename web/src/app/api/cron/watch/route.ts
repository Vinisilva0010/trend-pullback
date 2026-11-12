import { NextResponse } from "next/server";
import { env } from "@/lib/env";
import { bearer, sameSecret } from "@/lib/secrets";
import { runWatch } from "@/lib/watch";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";
export const maxDuration = 30;

// Called every few minutes by an outside scheduler (cron-job.org) with
// `Authorization: Bearer CRON_SECRET`.
export async function GET(request: Request) {
  const expected = env("CRON_SECRET");
  if (expected.length < 32 || !sameSecret(bearer(request.headers.get("authorization")), expected)) {
    return NextResponse.json({ error: "unauthorized" }, { status: 401 });
  }
  try {
    return NextResponse.json(await runWatch(Date.now()));
  } catch {
    return NextResponse.json({ error: "watch failed" }, { status: 500 });
  }
}
