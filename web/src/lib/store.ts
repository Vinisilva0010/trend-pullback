import "server-only";
import { Redis } from "@upstash/redis";
import { promises as fs } from "node:fs";
import path from "node:path";
import { env } from "./env";

// Key-value store: Upstash Redis in production; a JSON file in .data/
// during local development when Redis is not configured.

export interface Store {
  get(key: string): Promise<string | null>;
  set(key: string, value: string): Promise<void>;
  /** Increments a counter that expires `ttlSeconds` after its first increment. */
  hit(key: string, ttlSeconds: number): Promise<number>;
  hset(key: string, field: string, value: string): Promise<void>;
  hgetall(key: string): Promise<Record<string, string>>;
  hdel(key: string, field: string): Promise<void>;
}

function redisStore(url: string, token: string): Store {
  const redis = new Redis({ url, token, automaticDeserialization: false });
  return {
    get: (key) => redis.get<string>(key),
    async set(key, value) {
      await redis.set(key, value);
    },
    async hit(key, ttlSeconds) {
      const n = await redis.incr(key);
      if (n === 1) await redis.expire(key, ttlSeconds);
      return n;
    },
    async hset(key, field, value) {
      await redis.hset(key, { [field]: value });
    },
    async hgetall(key) {
      return (await redis.hgetall<Record<string, string>>(key)) ?? {};
    },
    async hdel(key, field) {
      await redis.hdel(key, field);
    },
  };
}

type FileData = { kv: Record<string, string>; hashes: Record<string, Record<string, string>>; hits: Record<string, { n: number; until: number }> };

function fileStore(): Store {
  const file = path.join(process.cwd(), ".data", "store.json");
  const load = async (): Promise<FileData> => {
    try {
      return JSON.parse(await fs.readFile(file, "utf8")) as FileData;
    } catch {
      return { kv: {}, hashes: {}, hits: {} };
    }
  };
  const save = async (data: FileData) => {
    await fs.mkdir(path.dirname(file), { recursive: true });
    await fs.writeFile(file, JSON.stringify(data));
  };
  return {
    async get(key) {
      const d = await load();
      const hit = d.hits[key];
      if (hit) return hit.until > Date.now() ? String(hit.n) : null;
      return d.kv[key] ?? null;
    },
    async set(key, value) {
      const d = await load();
      d.kv[key] = value;
      await save(d);
    },
    async hit(key, ttlSeconds) {
      const d = await load();
      const now = Date.now();
      const current = d.hits[key];
      const next = current && current.until > now ? { n: current.n + 1, until: current.until } : { n: 1, until: now + ttlSeconds * 1000 };
      d.hits[key] = next;
      await save(d);
      return next.n;
    },
    async hset(key, field, value) {
      const d = await load();
      d.hashes[key] = { ...(d.hashes[key] ?? {}), [field]: value };
      await save(d);
    },
    async hgetall(key) {
      return (await load()).hashes[key] ?? {};
    },
    async hdel(key, field) {
      const d = await load();
      if (d.hashes[key]) delete d.hashes[key][field];
      await save(d);
    },
  };
}

let cached: Store | null = null;

export function store(): Store {
  if (cached) return cached;
  const url = env("UPSTASH_REDIS_REST_URL") || env("KV_REST_API_URL");
  const token = env("UPSTASH_REDIS_REST_TOKEN") || env("KV_REST_API_TOKEN");
  if (url && token) {
    cached = redisStore(url, token);
  } else if (process.env.NODE_ENV === "development" || process.env.LOCAL_FILE_STORE === "1") {
    cached = fileStore();
  } else {
    throw new Error("Redis is not configured: connect Upstash Redis to the Vercel project");
  }
  return cached;
}

export const KEYS = {
  live: "tp:live",
  daily: "tp:daily",
  markets: "tp:markets",
  subscriptions: "tp:push:subs",
  // Small copies for the cloud watch, so it never reads the big snapshots.
  liveMs: "tp:live:ms",
  watchList: "tp:watch:list",
  watchAccount: "tp:watch:account",
  watchSent: "tp:watch:sent",
  watchLast: "tp:watch:last",
  staleFor: "tp:watch:stale",
  cloudEvents: "tp:cloud:events",
  loginFails: (ip: string) => `tp:login:fail:${ip}`,
} as const;
