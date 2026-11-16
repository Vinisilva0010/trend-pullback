// "Meus trades": the user's real trades, typed in by hand. Only prices,
// dates and notes: no account, balance or broker data is stored.

import { z } from "zod";
import { SETUPS, type SetupId } from "./score";

const SETUP_IDS = SETUPS.map((s) => s.id) as [SetupId, ...SetupId[]];

/** Most trades kept; the form refuses new ones past it. */
export const MAX_TRADES = 2000;

const price = z.number().finite().positive().max(1e9);
const ms = z.number().int().min(946_684_800_000).max(4_102_444_800_000);
const note = z.string().trim().max(500);

export const OpenSchema = z
  .object({
    symbol: z
      .string()
      .trim()
      .toUpperCase()
      .regex(/^[A-Z0-9.^=-]{1,20}$/, "símbolo com letras, números e . ^ = - (até 20)"),
    side: z.enum(["long", "short"]),
    setup: z.enum(SETUP_IDS),
    entry: price,
    stop: price,
    target: price.nullable(),
    opened_ms: ms,
    reason: note,
  })
  .strict()
  .superRefine((t, ctx) => {
    const long = t.side === "long";
    if (long ? t.stop >= t.entry : t.stop <= t.entry) {
      ctx.addIssue({ code: "custom", path: ["stop"], message: long ? "na compra o stop fica abaixo da entrada" : "na venda o stop fica acima da entrada" });
    }
    if (t.target != null && (long ? t.target <= t.entry : t.target >= t.entry)) {
      ctx.addIssue({ code: "custom", path: ["target"], message: long ? "na compra o alvo fica acima da entrada" : "na venda o alvo fica abaixo da entrada" });
    }
  });

export const CloseSchema = z
  .object({
    id: z.string().uuid(),
    exit: price,
    closed_ms: ms,
    followed: z.boolean(),
    exit_note: note,
  })
  .strict();

export const DeleteSchema = z.object({ id: z.string().uuid() }).strict();

export const TradeSchema = z.object({
  id: z.string().uuid(),
  symbol: z.string().max(20),
  side: z.enum(["long", "short"]),
  setup: z.enum(SETUP_IDS),
  entry: price,
  stop: price,
  target: price.nullable(),
  opened_ms: ms,
  reason: z.string().max(500),
  created_ms: ms,
  exit: price.nullable(),
  closed_ms: ms.nullable(),
  followed: z.boolean().nullable(),
  exit_note: z.string().max(500),
});

export type OpenInput = z.infer<typeof OpenSchema>;
export type CloseInput = z.infer<typeof CloseSchema>;
export type JournalTrade = z.infer<typeof TradeSchema>;

export function newTrade(input: OpenInput, id: string, now: number): JournalTrade {
  return { ...input, id, created_ms: now, exit: null, closed_ms: null, followed: null, exit_note: "" };
}

/** Closes a trade; refuses an exit dated before the entry. */
export function closeTrade(t: JournalTrade, input: CloseInput): JournalTrade | string {
  if (t.closed_ms != null) return "este trade já foi fechado";
  if (input.closed_ms < t.opened_ms) return "a saída não pode ser antes da entrada";
  return { ...t, exit: input.exit, closed_ms: input.closed_ms, followed: input.followed, exit_note: input.exit_note };
}

/** Result in R (1R = what the stop loses); null while open. */
export function tradeR(t: Pick<JournalTrade, "side" | "entry" | "stop" | "exit">): number | null {
  if (t.exit == null) return null;
  const risk = Math.abs(t.entry - t.stop);
  if (risk === 0) return null;
  return (t.side === "long" ? t.exit - t.entry : t.entry - t.exit) / risk;
}

/** Percent the stop is from the entry. */
export function stopPct(t: Pick<JournalTrade, "entry" | "stop">): number {
  return (Math.abs(t.entry - t.stop) / t.entry) * 100;
}

/** Midnight in Brasília (03:00 UTC) of a yyyy-mm-dd date from the form. */
export function dayMs(text: string): number | null {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(text)) return null;
  const ms = Date.parse(`${text}T03:00:00Z`);
  return Number.isFinite(ms) ? ms : null;
}

/** Trades in display order: open first (newest entry first), then closed (newest exit first). */
export function sorted(trades: JournalTrade[]): JournalTrade[] {
  return [...trades].sort((a, b) => {
    if ((a.closed_ms == null) !== (b.closed_ms == null)) return a.closed_ms == null ? -1 : 1;
    return (b.closed_ms ?? b.opened_ms) - (a.closed_ms ?? a.opened_ms);
  });
}

/** A number typed in the form, Brazilian ("1.234,56") or plain ("1234.56"). */
export function parseNum(text: string): number | null {
  let t = text.trim().replace(/\s/g, "");
  if (!t) return null;
  if (t.includes(",")) t = t.replace(/\./g, "").replace(",", ".");
  if (!/^\d+(\.\d+)?$/.test(t)) return null;
  const v = Number(t);
  return Number.isFinite(v) ? v : null;
}
