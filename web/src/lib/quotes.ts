// Last prices of stocks, indexes and ETFs from Yahoo Finance's public chart
// endpoint (the same one the tool reads). Not an official API: a symbol that
// fails is left out and the page keeps the last close the tool sent.

export type Quote = {
  symbol: string;
  price: number;
  /** Close of the session before today's. */
  prev_close: number | null;
  /** Today's candle so far ([open ms, open, high, low, close]), when the session has started. */
  day: [number, number, number, number, number] | null;
  /** This week and this month so far, same layout; the time is the first session's. */
  week: [number, number, number, number, number] | null;
  month: [number, number, number, number, number] | null;
  /** Time of the last price. */
  time_ms: number;
  /** Minutes the exchange delays its data on this source (0 is real time). */
  delay_min: number;
  /** The regular session is open now. */
  open: boolean;
};

/** Letters, digits and `^ . = -` only, like the tool (e.g. `ITUB4.SA`, `^BVSP`). */
export const SYMBOL = /^[A-Za-z0-9^.=-]{1,20}$/;

type Period = { start?: number; end?: number };

function num(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}

type Row = [number, number, number, number, number];

/** One candle from several daily ones (first open, highest high, lowest low, last close). */
function merge(rows: Row[]): Row | null {
  const first = rows[0];
  const last = rows.at(-1);
  if (!first || !last) return null;
  return [first[0], first[1], Math.max(...rows.map((r) => r[2])), Math.min(...rows.map((r) => r[3])), last[4]];
}

/** Reads a `range=1mo&interval=1d` chart response; null when it is not a chart. */
export function parseQuote(symbol: string, body: unknown, nowMs: number): Quote | null {
  const result = (body as { chart?: { result?: unknown[] } })?.chart?.result?.[0] as
    | {
        meta?: Record<string, unknown>;
        timestamp?: unknown[];
        indicators?: { quote?: Record<string, unknown[]>[] };
      }
    | undefined;
  const meta = result?.meta;
  const price = num(meta?.regularMarketPrice);
  if (!result || !meta || price == null) return null;
  const times = Array.isArray(result.timestamp) ? result.timestamp : [];
  const q = result.indicators?.quote?.[0] ?? {};
  const rows: Row[] = [];
  times.forEach((t, i) => {
    const o = num(q.open?.[i]);
    const h = num(q.high?.[i]);
    const l = num(q.low?.[i]);
    const c = num(q.close?.[i]);
    const ts = num(t);
    if (ts != null && o != null && h != null && l != null && c != null && l > 0 && l <= h) rows.push([ts * 1000, o, h, l, c]);
  });
  const timeMs = (num(meta.regularMarketTime) ?? nowMs / 1000) * 1000;
  const period = (meta.currentTradingPeriod as { regular?: Period } | undefined)?.regular;
  const open = period?.start != null && period?.end != null && nowMs >= period.start * 1000 && nowMs < period.end * 1000;
  const sameDay = (a: number, b: number) => Math.abs(a - b) < 20 * 3600 * 1000 && new Date(a).getUTCDate() === new Date(b).getUTCDate();
  const last = rows.at(-1);
  // The last row is today's candle when it is from the day of the last price.
  const today = last && sameDay(last[0], timeMs) ? last : null;
  const before = today ? rows.at(-2) : last;
  const day: Quote["day"] = today ? [today[0], today[1], Math.max(today[2], price), Math.min(today[3], price), price] : null;
  // Rows of this week (from Monday) and this month, with today's live values.
  const live = rows.map((r) => (today && r[0] === today[0] && day ? day : r));
  const ref = new Date(last?.[0] ?? timeMs);
  const monday = Date.UTC(ref.getUTCFullYear(), ref.getUTCMonth(), ref.getUTCDate() - ((ref.getUTCDay() + 6) % 7));
  const week = merge(live.filter((r) => r[0] >= monday));
  const month = merge(live.filter((r) => new Date(r[0]).getUTCMonth() === ref.getUTCMonth() && new Date(r[0]).getUTCFullYear() === ref.getUTCFullYear()));
  return {
    symbol,
    price,
    prev_close: before ? before[4] : num(meta.chartPreviousClose),
    day,
    week,
    month,
    time_ms: timeMs,
    delay_min: num(meta.exchangeDataDelayedBy) ?? 0,
    open,
  };
}
