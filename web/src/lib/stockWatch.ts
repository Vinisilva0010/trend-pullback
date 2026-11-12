import { change, money, pct, price, when } from "./format";

// The intraday watch of the US stock trades, run from the cloud every few
// minutes (the same rules as `stock_watch.rs` on the computer): the price
// passing a signal's trigger, and an open trade hitting its stop or target.

export type Watched = {
  key: string;
  symbol: string;
  name: string;
  tf: string;
  /** `sinal` (waiting for the trigger) or `aberto` (entered). */
  status: string;
  trigger: number;
  stop: number;
  target: number;
  /** Watch from this time on: the signal's close, or the entry's close. */
  from_ms: number;
  /** A signal expires here (null for an open trade). */
  until_ms: number | null;
  in_book: boolean;
};

/** [open ms, open, high, low, close] */
export type Bar = [number, number, number, number, number];
export type Intraday = { bars: Bar[]; price: number | null; delayMin: number };
export type Account = { value: number; real: boolean };
export type WatchAlert = { key: string; message: string };

const RISK = 0.02;
const POSITION_CAP = 1 / 3;
/** Below this reward to risk, buying late is outside what the test covered. */
const LATE_RR = 1.5;
const DAY_MS = 86_400_000;
const HOUR_MS = 3_600_000;

/** Monday to Friday, 13:00 to 22:00 UTC: the New York session in summer and winter time. */
export function inSession(nowMs: number): boolean {
  const weekday = new Date(nowMs).getUTCDay();
  const hour = Math.floor((((nowMs % DAY_MS) + DAY_MS) % DAY_MS) / HOUR_MS);
  return weekday !== 0 && weekday !== 6 && hour >= 13 && hour < 22;
}

/** Whole shares for a buy at `price` with `stop`: 2% of the account at risk, a third of it at most. */
export function shares(account: number, entry: number, stop: number): number {
  const risk = entry - stop;
  if (!(risk > 0) || !(entry > 0) || !(account > 0)) return 0;
  const budget = Math.min(((RISK * account) / risk) * entry, account * POSITION_CAP);
  const n = Math.floor(budget / entry);
  return Number.isFinite(n) && n > 0 ? n : 0;
}

function num(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}

/** Reads a `range=5d&interval=15m` chart response; null when it is not a chart. */
export function parseIntraday(body: unknown): Intraday | null {
  const result = (body as { chart?: { result?: unknown[] } })?.chart?.result?.[0] as
    | { meta?: Record<string, unknown>; timestamp?: unknown[]; indicators?: { quote?: Record<string, unknown[]>[] } }
    | undefined;
  if (!result) return null;
  const q = result.indicators?.quote?.[0] ?? {};
  const bars: Bar[] = [];
  (Array.isArray(result.timestamp) ? result.timestamp : []).forEach((t, i) => {
    const [ts, o, h, l, c] = [num(t), num(q.open?.[i]), num(q.high?.[i]), num(q.low?.[i]), num(q.close?.[i])];
    if (ts != null && o != null && h != null && l != null && c != null && l > 0 && l <= h) bars.push([ts * 1000, o, h, l, c]);
  });
  return { bars, price: num(result.meta?.regularMarketPrice) ?? bars.at(-1)?.[4] ?? null, delayMin: num(result.meta?.exchangeDataDelayedBy) ?? 0 };
}

const chart = (tf: string) => (tf === "1w" ? "semanal" : "diário");
const ratio = (v: number) => v.toLocaleString("pt-BR", { minimumFractionDigits: 1, maximumFractionDigits: 1 });

function accountLabel(a: Account): string {
  return a.real
    ? `a tua conta de ${money(a.value)} (STOCK_ACCOUNT_USD)`
    : `o saldo da conta fictícia, ${money(a.value)} (para usar o teu saldo real, coloque STOCK_ACCOUNT_USD na Vercel)`;
}

function entryText(w: Watched, atMs: number, last: number, delayMin: number, account: Account): string {
  let text = `${w.name} (gráfico ${chart(w.tf)}): PASSOU DO GATILHO ${price(w.trigger)} em ${when(atMs)}. Hora de decidir a compra.\nPreço agora ${price(last)} (${pct(change(w.trigger, last))} do gatilho${delayMin > 0 ? `; a fonte tem ${delayMin} min de atraso` : ""}).`;
  if (last <= w.stop) return `${text}\nJá voltou para o stop ${price(w.stop)} ou abaixo: não entrar.`;
  const rr = (w.target - last) / (last - w.stop);
  text += `\nComprando agora: stop ${price(w.stop)} (${pct(change(last, w.stop), false)} abaixo), alvo ${price(w.target)} (${pct(change(last, w.target), false)} acima), ${ratio(rr)} vezes o risco. O teste comprou no gatilho, com 2 vezes.`;
  if (rr < LATE_RR) text += `\nO preço já correu: abaixo de ${ratio(LATE_RR)} vezes o risco, o teste não cobre essa entrada.`;
  const n = shares(account.value, last, w.stop);
  text +=
    n === 0
      ? `\nCom ${accountLabel(account)}, uma ação passa do limite por trade (um terço da conta).`
      : `\nQuantidade: ${n} ${n === 1 ? "ação" : "ações"} (cerca de ${money(n * last)}), perde ${money(n * (last - w.stop))} se bater o stop. Conta: ${accountLabel(account)}, risco de 2%.`;
  return `${text}\nNa Nomad não existe ordem stop: compre com ordem a mercado ou limitada. O stop e o alvo ficam por conta destes avisos.`;
}

function exitAlert(w: Watched, bars: Bar[], last: number): WatchAlert | null {
  const stopAt = bars.find((b) => b[3] <= w.stop);
  const targetAt = bars.find((b) => b[2] >= w.target);
  const book = w.in_book ? "\nA conta fictícia registra a saída pelo candle fechado, no próximo aviso de fechamento." : "";
  // The stop comes first when both happened on the same candle.
  if (stopAt && (!targetAt || stopAt[0] <= targetAt[0])) {
    return {
      key: `${w.key}|stopdia`,
      message: `${w.name} (gráfico ${chart(w.tf)}): BATEU NO STOP ${price(w.stop)} em ${when(stopAt[0])}. Preço agora ${price(last)}.\nSe você comprou, venda agora: na Nomad, ordem a mercado.${book}`,
    };
  }
  if (targetAt) {
    return {
      key: `${w.key}|alvodia`,
      message: `${w.name} (gráfico ${chart(w.tf)}): CHEGOU NO ALVO ${price(w.target)} em ${when(targetAt[0])}. Preço agora ${price(last)}.\nSe você comprou, venda agora para garantir o lucro: na Nomad, ordem a mercado ou limitada no alvo.${book}`,
    };
  }
  return null;
}

/** New alerts for one watched setup from its intraday candles. */
export function alertsFor(w: Watched, d: Intraday, sent: ReadonlySet<string>, account: Account): WatchAlert[] {
  const bars = d.bars.filter((b) => b[0] >= w.from_ms);
  const last = d.price ?? bars.at(-1)?.[4];
  if (last == null) return [];
  const out: WatchAlert[] = [];
  let after = bars;
  if (w.status !== "aberto") {
    const at = bars.findIndex((b) => (w.until_ms == null || b[0] < w.until_ms) && b[2] >= w.trigger);
    if (at < 0) return out;
    const key = `${w.key}|gatilho`;
    if (!sent.has(key)) out.push({ key, message: entryText(w, bars[at]![0], last, d.delayMin, account) });
    after = bars.slice(at);
  }
  const exit = exitAlert(w, after, last);
  if (exit && !sent.has(exit.key)) out.push(exit);
  return out;
}
