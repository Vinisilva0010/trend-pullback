import { describe, expect, it } from "vitest";
import { parseQuote } from "./quotes";

const day = 86_400;
// Monday 2025-11-10 14:30 UTC (09:30 in New York) and the Friday before.
const fri = 1_762_525_800;
const mon = fri + 3 * day;

function body(price: number, timestamps: number[], period: { start: number; end: number }) {
  return {
    chart: {
      result: [
        {
          meta: { regularMarketPrice: price, regularMarketTime: mon + 3600, exchangeDataDelayedBy: 0, currentTradingPeriod: { regular: period } },
          timestamp: timestamps,
          indicators: { quote: [{ open: [100, 102], high: [103, 104], low: [99, 101], close: [102, 103.5] }] },
        },
      ],
    },
  };
}

describe("parseQuote", () => {
  it("takes today's candle and the previous close", () => {
    const q = parseQuote("SPY", body(105, [fri, mon], { start: mon, end: mon + 23_400 }), (mon + 3600) * 1000)!;
    expect(q.price).toBe(105);
    expect(q.prev_close).toBe(102);
    expect(q.day).toEqual([mon * 1000, 102, 105, 101, 105]);
    expect(q.open).toBe(true);
  });

  it("before the session, the last candle is the last session's", () => {
    const b = body(102, [fri - day, fri], { start: mon, end: mon + 23_400 });
    b.chart.result[0]!.meta.regularMarketTime = fri + 23_400;
    const q = parseQuote("SPY", b, (mon - 3600) * 1000)!;
    expect(q.day).toEqual([fri * 1000, 102, 104, 101, 102]);
    expect(q.prev_close).toBe(102);
    expect(q.open).toBe(false);
  });

  it("rejects what is not a chart", () => {
    expect(parseQuote("X", { chart: { result: null } }, 0)).toBeNull();
  });
});
