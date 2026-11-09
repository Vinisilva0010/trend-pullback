import "server-only";
import { DailySchema, LiveSchema, type Daily, type Live } from "./schema";
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

export function getLive(): Promise<Live | null> {
  return read(KEYS.live, (v) => LiveSchema.parse(v));
}

export function getDaily(): Promise<Daily | null> {
  return read(KEYS.daily, (v) => DailySchema.parse(v));
}
