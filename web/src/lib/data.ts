import "server-only";
import { z } from "zod";
import { DailySchema, EventSchema, LiveSchema, MarketsSchema, type Daily, type Live, type Markets } from "./schema";
import { KEYS, store } from "./store";

async function read<T>(key: string, parse: (v: unknown) => T): Promise<T | null> {
  const text = await store().get(key);
  if (!text) return null;
  try {
    return parse(JSON.parse(text));
  } catch {
    return null;
  }
}

export async function getLive(): Promise<Live | null> {
  const live = await read(KEYS.live, (v) => LiveSchema.parse(v));
  if (!live) return null;
  // Alerts sent by the cloud watch join the feed.
  const cloud = await read(KEYS.cloudEvents, (v) => z.array(EventSchema).parse(v));
  if (!cloud?.length) return live;
  const seen = new Set(live.events.map((e) => e.id));
  const events = [...cloud.filter((e) => !seen.has(e.id)), ...live.events].sort((a, b) => b.ms - a.ms).slice(0, 1000);
  return { ...live, events };
}

export function getMarkets(): Promise<Markets | null> {
  return read(KEYS.markets, (v) => MarketsSchema.parse(v));
}

export function getDaily(): Promise<Daily | null> {
  return read(KEYS.daily, (v) => DailySchema.parse(v));
}
