import "server-only";
import { closeTrade, MAX_TRADES, newTrade, sorted, TradeSchema, type CloseInput, type JournalTrade, type OpenInput } from "./journal";
import { KEYS, store } from "./store";

export async function listTrades(): Promise<JournalTrade[]> {
  const raw = await store().hgetall(KEYS.journal);
  const out: JournalTrade[] = [];
  for (const value of Object.values(raw)) {
    try {
      const parsed = TradeSchema.safeParse(typeof value === "string" ? JSON.parse(value) : value);
      if (parsed.success) out.push(parsed.data);
    } catch {
      // A broken row is skipped, never shown half-read.
    }
  }
  return sorted(out);
}

export async function addTrade(input: OpenInput, now = Date.now()): Promise<JournalTrade | string> {
  const count = Object.keys(await store().hgetall(KEYS.journal)).length;
  if (count >= MAX_TRADES) return `limite de ${MAX_TRADES} trades atingido`;
  const trade = newTrade(input, crypto.randomUUID(), now);
  await store().hset(KEYS.journal, trade.id, JSON.stringify(trade));
  return trade;
}

export async function closeStored(input: CloseInput): Promise<JournalTrade | string> {
  const all = await listTrades();
  const t = all.find((x) => x.id === input.id);
  if (!t) return "trade não encontrado";
  const closed = closeTrade(t, input);
  if (typeof closed === "string") return closed;
  await store().hset(KEYS.journal, closed.id, JSON.stringify(closed));
  return closed;
}

export async function removeTrade(id: string): Promise<void> {
  await store().hdel(KEYS.journal, id);
}
