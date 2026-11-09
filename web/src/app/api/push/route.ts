import { NextResponse } from "next/server";
import { z } from "zod";
import { isLoggedIn } from "@/lib/auth";
import { removeSubscription, saveSubscription, SubscriptionSchema, vapidPublicKey } from "@/lib/push";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function GET() {
  if (!(await isLoggedIn())) return NextResponse.json({ error: "unauthorized" }, { status: 401 });
  return NextResponse.json({ publicKey: vapidPublicKey() });
}

export async function POST(request: Request) {
  if (!(await isLoggedIn())) return NextResponse.json({ error: "unauthorized" }, { status: 401 });
  const parsed = SubscriptionSchema.safeParse(await request.json().catch(() => null));
  if (!parsed.success) return NextResponse.json({ error: "invalid subscription" }, { status: 400 });
  await saveSubscription(parsed.data);
  return NextResponse.json({ ok: true });
}

export async function DELETE(request: Request) {
  if (!(await isLoggedIn())) return NextResponse.json({ error: "unauthorized" }, { status: 401 });
  const parsed = z.object({ endpoint: z.string().url().max(1000) }).safeParse(await request.json().catch(() => null));
  if (!parsed.success) return NextResponse.json({ error: "invalid" }, { status: 400 });
  await removeSubscription(parsed.data.endpoint);
  return NextResponse.json({ ok: true });
}
