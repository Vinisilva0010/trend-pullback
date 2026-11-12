import "server-only";
import { z } from "zod";
import { env } from "./env";
import { when } from "./format";
import { notifyAll } from "./push";
import { EventSchema, WatchedSchema, type AppEvent } from "./schema";
import { alertsFor, inSession, parseIntraday, type Account, type Intraday, type Watched } from "./stockWatch";
import { KEYS, store } from "./store";
import { sendTelegram, telegramConfigured } from "./telegram";

// The cloud watch, called every few minutes by an outside scheduler: it
// warns when the computer's robot stops publishing, and checks the US
// stock signals and open trades during the session, computer on or off.

const HOUR_MS = 3_600_000;
/** The robot runs every hour: two hours without news means it stopped. */
export const STALE_MS = 2 * HOUR_MS;
const USER_AGENT = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";
const HOSTS = ["https://query1.finance.yahoo.com", "https://query2.finance.yahoo.com"];
const PARALLEL = 6;
const KEEP_EVENTS = 200;
const KEEP_SENT_MS = 45 * 24 * HOUR_MS;

/** 23:00 to 07:00 in Brasília: no "robot stopped" message while he sleeps. */
export function quietHours(nowMs: number): boolean {
  const hour = new Date(nowMs - 3 * HOUR_MS).getUTCHours();
  return hour >= 23 || hour < 7;
}

export type StaleStep = { send: "parado" | "voltou" | null; notifiedFor: string | null };

/**
 * One warning per stop (outside quiet hours) and one note when the robot
 * is back. `notifiedFor` is the last publish time already warned about.
 */
export function staleStep(nowMs: number, liveMs: number | null, notifiedFor: string | null): StaleStep {
  if (liveMs == null) return { send: null, notifiedFor };
  const stale = nowMs - liveMs > STALE_MS;
  const quiet = quietHours(nowMs);
  if (stale) {
    if (notifiedFor === String(liveMs) || quiet) return { send: null, notifiedFor };
    return { send: "parado", notifiedFor: String(liveMs) };
  }
  if (notifiedFor != null) return { send: quiet ? null : "voltou", notifiedFor: null };
  return { send: null, notifiedFor: null };
}

async function intraday(symbol: string): Promise<Intraday | null> {
  for (const host of HOSTS) {
    try {
      const r = await fetch(`${host}/v8/finance/chart/${encodeURIComponent(symbol)}?range=5d&interval=15m`, {
        headers: { "user-agent": USER_AGENT, accept: "application/json" },
        cache: "no-store",
        signal: AbortSignal.timeout(8000),
      });
      if (!r.ok) continue;
      const d = parseIntraday(await r.json());
      if (d) return d;
    } catch {
      // Try the other host.
    }
  }
  return null;
}

async function account(): Promise<Account> {
  const real = Number(env("STOCK_ACCOUNT_USD"));
  if (Number.isFinite(real) && real > 0) return { value: real, real: true };
  const paper = Number((await store().get(KEYS.watchAccount)) ?? "");
  return { value: Number.isFinite(paper) && paper > 0 ? paper : 2200, real: false };
}

async function readList(): Promise<Watched[]> {
  try {
    return z.array(WatchedSchema).parse(JSON.parse((await store().get(KEYS.watchList)) ?? "[]"));
  } catch {
    return [];
  }
}

export async function cloudEvents(): Promise<AppEvent[]> {
  try {
    return z.array(EventSchema).parse(JSON.parse((await store().get(KEYS.cloudEvents)) ?? "[]"));
  } catch {
    return [];
  }
}

async function checkStocks(nowMs: number): Promise<{ symbols: number; alerts: number }> {
  const list = await readList();
  if (list.length === 0) return { symbols: 0, alerts: 0 };
  const s = store();
  const sentAt = await s.hgetall(KEYS.watchSent);
  const sent = new Set(Object.keys(sentAt));
  const acct = await account();
  const symbols = [...new Set(list.map((w) => w.symbol))];
  const events: AppEvent[] = [];
  for (let i = 0; i < symbols.length; i += PARALLEL) {
    const batch = symbols.slice(i, i + PARALLEL);
    const data = await Promise.all(batch.map(intraday));
    for (const [j, d] of data.entries()) {
      if (!d) continue;
      for (const w of list.filter((x) => x.symbol === batch[j])) {
        for (const a of alertsFor(w, d, sent, acct)) {
          // Marked as sent only when Telegram took it, so a failure tries again.
          if (!(await sendTelegram(a.message))) continue;
          await s.hset(KEYS.watchSent, a.key, String(nowMs));
          sent.add(a.key);
          events.push({ id: a.key, ms: nowMs, asset: w.name, kind: "stormer", title: a.message.split("\n")[0]!.slice(0, 500), text: a.message, notify: true, level: 3, lean: null, market: "acoes" });
        }
      }
    }
  }
  if (events.length) {
    const all = [...events, ...(await cloudEvents())].slice(0, KEEP_EVENTS);
    await s.set(KEYS.cloudEvents, JSON.stringify(all));
    await notifyAll(events);
  }
  // Old marks leave a few at a time.
  const old = Object.entries(sentAt).filter(([, ms]) => nowMs - Number(ms) > KEEP_SENT_MS).slice(0, 20);
  for (const [key] of old) await s.hdel(KEYS.watchSent, key);
  return { symbols: symbols.length, alerts: events.length };
}

export type WatchResult = { ok: true; telegram: boolean; robot: StaleStep["send"]; symbols: number; alerts: number };

export async function runWatch(nowMs: number): Promise<WatchResult> {
  const s = store();
  const telegram = telegramConfigured();
  const liveMs = Number((await s.get(KEYS.liveMs)) ?? "") || null;
  const step = staleStep(nowMs, liveMs, (await s.get(KEYS.staleFor)) || null);
  let robot: StaleStep["send"] = null;
  if (step.send === "parado" && liveMs != null) {
    const hours = Math.floor((nowMs - liveMs) / HOUR_MS);
    const ok = await sendTelegram(
      `O robô do computador está parado: a última rodada foi ${when(liveMs)} (há ${hours} h).\nEnquanto ele não voltar, não saem sinais novos, avisos de cripto nem o relatório. Os avisos de gatilho, stop e alvo das ações continuam saindo pela nuvem.\nPara voltar: ligue o computador e entre no Windows.`,
    );
    if (ok) {
      await s.set(KEYS.staleFor, step.notifiedFor ?? "");
      robot = "parado";
    }
  } else if (step.notifiedFor == null && (await s.get(KEYS.staleFor))) {
    if (step.send === "voltou" && liveMs != null) await sendTelegram(`O robô do computador voltou: rodou ${when(liveMs)}.`);
    await s.set(KEYS.staleFor, "");
    robot = step.send;
  }
  const stocks = telegram && inSession(nowMs) ? await checkStocks(nowMs) : { symbols: 0, alerts: 0 };
  await s.set(KEYS.watchLast, String(nowMs));
  return { ok: true, telegram, robot, ...stocks };
}

/** True when the cloud watch ran in the last 40 minutes and can send to Telegram. */
export async function watchActive(nowMs: number): Promise<boolean> {
  const last = Number((await store().get(KEYS.watchLast)) ?? "");
  return telegramConfigured() && Number.isFinite(last) && last > 0 && nowMs - last < 40 * 60_000;
}
