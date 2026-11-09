import { NextResponse } from "next/server";
import { z } from "zod";
import { createSession, SESSION_COOKIE, SESSION_DAYS } from "@/lib/session";
import { sameSecret } from "@/lib/secrets";
import { KEYS, store } from "@/lib/store";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

const MAX_FAILS = 5;
const LOCK_SECONDS = 15 * 60;

function clientIp(request: Request): string {
  const forwarded = request.headers.get("x-forwarded-for")?.split(",")[0]?.trim();
  return (forwarded || request.headers.get("x-real-ip") || "local").slice(0, 64);
}

export async function POST(request: Request) {
  const origin = request.headers.get("origin");
  if (origin && new URL(origin).host !== request.headers.get("host")) {
    return NextResponse.json({ error: "bad origin" }, { status: 403 });
  }
  const password = process.env.DASHBOARD_PASSWORD ?? "";
  const secret = process.env.SESSION_SECRET ?? "";
  if (password.length < 12 || secret.length < 32) {
    return NextResponse.json({ error: "server not configured" }, { status: 500 });
  }
  const ip = clientIp(request);
  const fails = await store().get(KEYS.loginFails(ip));
  if (Number(fails ?? 0) >= MAX_FAILS) {
    return NextResponse.json({ error: "Muitas tentativas. Espere 15 minutos." }, { status: 429 });
  }
  const parsed = z.object({ password: z.string().max(200) }).safeParse(await request.json().catch(() => null));
  if (!parsed.success || !sameSecret(parsed.data.password, password)) {
    await store().hit(KEYS.loginFails(ip), LOCK_SECONDS);
    return NextResponse.json({ error: "Senha errada." }, { status: 401 });
  }
  const response = NextResponse.json({ ok: true });
  response.cookies.set(SESSION_COOKIE, await createSession(secret), {
    httpOnly: true,
    secure: process.env.NODE_ENV === "production",
    sameSite: "lax",
    path: "/",
    maxAge: SESSION_DAYS * 24 * 3600,
  });
  return response;
}
