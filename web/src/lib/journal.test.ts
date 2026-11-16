import { describe, expect, it } from "vitest";
import { CloseSchema, closeTrade, dayMs, newTrade, OpenSchema, parseNum, sorted, stopPct, tradeR } from "./journal";

const ID = "6f1c2b9e-3a4d-4e5f-8a7b-1c2d3e4f5a6b";
const DAY = Date.UTC(2026, 10, 16, 3);
const base = { symbol: " nvda ", side: "long", setup: "acoes-1d", entry: 100, stop: 95, target: 110, opened_ms: DAY, reason: "trap no diário" };

describe("journal", () => {
  it("accepts a valid trade and refuses a stop or target on the wrong side", () => {
    const ok = OpenSchema.parse(base);
    expect(ok.symbol).toBe("NVDA");
    expect(OpenSchema.safeParse({ ...base, stop: 101 }).success).toBe(false);
    expect(OpenSchema.safeParse({ ...base, target: 99 }).success).toBe(false);
    expect(OpenSchema.safeParse({ ...base, side: "short", stop: 105, target: 90 }).success).toBe(true);
    expect(OpenSchema.safeParse({ ...base, symbol: "<script>" }).success).toBe(false);
    expect(OpenSchema.safeParse({ ...base, setup: "x" }).success).toBe(false);
    expect(OpenSchema.safeParse({ ...base, extra: 1 }).success).toBe(false);
    expect(OpenSchema.safeParse({ ...base, reason: "x".repeat(501) }).success).toBe(false);
    expect(OpenSchema.safeParse({ ...base, entry: -1 }).success).toBe(false);
  });

  it("measures the result in R for buys and sells", () => {
    expect(tradeR({ side: "long", entry: 100, stop: 95, exit: 110 })).toBe(2);
    expect(tradeR({ side: "long", entry: 100, stop: 95, exit: 92 })).toBeCloseTo(-1.6);
    expect(tradeR({ side: "short", entry: 100, stop: 105, exit: 90 })).toBe(2);
    expect(tradeR({ side: "long", entry: 100, stop: 95, exit: null })).toBeNull();
    expect(stopPct({ entry: 100, stop: 95 })).toBe(5);
  });

  it("closes a trade once and never before it opened", () => {
    const t = newTrade(OpenSchema.parse(base), ID, DAY);
    const input = CloseSchema.parse({ id: ID, exit: 110, closed_ms: DAY + 86_400_000, followed: true, exit_note: "" });
    const closed = closeTrade(t, input);
    expect(typeof closed).toBe("object");
    if (typeof closed === "string") throw new Error(closed);
    expect(closed.exit).toBe(110);
    expect(closeTrade(closed, input)).toBe("este trade já foi fechado");
    expect(closeTrade(t, { ...input, closed_ms: DAY - 86_400_000 })).toMatch(/antes da entrada/);
    expect(sorted([closed, t]).map((x) => x.closed_ms)).toEqual([null, DAY + 86_400_000]);
  });

  it("reads the numbers and dates typed in the form", () => {
    expect(parseNum("1.234,56")).toBe(1234.56);
    expect(parseNum("123,4")).toBe(123.4);
    expect(parseNum("0.00001234")).toBe(0.00001234);
    expect(parseNum("abc")).toBeNull();
    expect(parseNum("")).toBeNull();
    expect(parseNum("-5")).toBeNull();
    expect(dayMs("2026-11-16")).toBe(DAY);
    expect(dayMs("16/11/2026")).toBeNull();
  });
});
