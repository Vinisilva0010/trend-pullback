import "server-only";
import webpush from "web-push";
import { z } from "zod";
import type { AppEvent } from "./schema";
import { KEYS, store } from "./store";
import { env } from "./env";

export const SubscriptionSchema = z.object({
  endpoint: z.string().url().max(1000).startsWith("https://"),
  keys: z.object({ p256dh: z.string().min(10).max(200), auth: z.string().min(10).max(100) }),
});
export type PushSubscriptionJson = z.infer<typeof SubscriptionSchema>;

/** Most notifications sent per publish; the rest become one summary. */
const MAX_SINGLE = 4;

export function vapidPublicKey(): string | null {
  return env("VAPID_PUBLIC_KEY") || null;
}

function configure(): boolean {
  const pub = env("VAPID_PUBLIC_KEY");
  const priv = env("VAPID_PRIVATE_KEY");
  const subject = env("VAPID_SUBJECT");
  if (!pub || !priv || !subject) return false;
  webpush.setVapidDetails(subject, pub, priv);
  return true;
}

export async function saveSubscription(sub: PushSubscriptionJson): Promise<void> {
  await store().hset(KEYS.subscriptions, sub.endpoint, JSON.stringify(sub));
}

export async function removeSubscription(endpoint: string): Promise<void> {
  await store().hdel(KEYS.subscriptions, endpoint);
}

type Payload = { title: string; body: string; url: string; tag: string };

export function payloadsFor(events: AppEvent[]): Payload[] {
  const important = events.filter((e) => e.notify);
  const single = important.slice(0, MAX_SINGLE).map((e) => {
    const lines = e.text.split("\n").filter(Boolean);
    return {
      title: e.title.slice(0, 120),
      body: (lines[1] ?? lines[0] ?? "").slice(0, 240),
      url: `/avisos#${encodeURIComponent(e.id)}`,
      tag: e.id.slice(0, 64),
    };
  });
  const rest = important.length - single.length;
  if (rest > 0) {
    single.push({ title: `Mais ${rest} avisos novos`, body: "Abra o painel para ver todos.", url: "/avisos", tag: "resumo" });
  }
  return single;
}

/** Sends the events marked `notify` to every saved device. */
export async function notifyAll(events: AppEvent[]): Promise<number> {
  const payloads = payloadsFor(events);
  if (payloads.length === 0 || !configure()) return 0;
  const subs = await store().hgetall(KEYS.subscriptions);
  let sent = 0;
  for (const [endpoint, raw] of Object.entries(subs)) {
    const parsed = SubscriptionSchema.safeParse(JSON.parse(raw));
    if (!parsed.success) {
      await removeSubscription(endpoint);
      continue;
    }
    for (const p of payloads) {
      try {
        await webpush.sendNotification(parsed.data, JSON.stringify(p), { TTL: 6 * 3600, urgency: "high" });
        sent++;
      } catch (error) {
        const status = (error as { statusCode?: number }).statusCode;
        if (status === 404 || status === 410) {
          await removeSubscription(endpoint);
          break;
        }
        console.error("push failed", status);
      }
    }
  }
  return sent;
}
