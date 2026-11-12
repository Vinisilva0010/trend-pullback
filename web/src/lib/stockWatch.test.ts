import { describe, expect, it } from "vitest";
import { alertsFor, inSession, parseIntraday, shares, type Bar, type Watched } from "./stockWatch";

const HOUR = 3_600_000;
const watched = (status: string): Watched => ({
  key: "AAPL|1d|trap|long|0", symbol: "AAPL", name: "Apple", tf: "1d", status, trigger: 100, stop: 95, target: 110, from_ms: 1000, until_ms: 10_000, in_book: true,
});
const bar = (ms: number, high: number, low: number): Bar => [ms, low, high, low, high];
const real = { value: 3000, real: true };
const paper = { value: 2200, real: false };

describe("stock watch", () => {
  it("runs on weekdays during the New York session", () => {
    const friday = Date.UTC(2025, 10, 7);
    expect(inSession(friday + 15 * HOUR)).toBe(true);
    expect(inSession(friday + 23 * HOUR)).toBe(false);
    expect(inSession(friday + 24 * HOUR + 15 * HOUR)).toBe(false);
  });

  it("sizes whole shares by 2% risk and a third of the account", () => {
    expect(shares(3000, 101, 96)).toBe(9);
    expect(shares(10_000, 100, 80)).toBe(10);
    expect(shares(100, 500, 490)).toBe(0);
    expect(shares(1000, 100, 100)).toBe(0);
  });

  it("alerts the trigger once, with the share count", () => {
    const d = { bars: [bar(500, 120, 90), bar(2000, 99, 97), bar(3000, 101, 98)], price: 101, delayMin: 0 };
    const a = alertsFor(watched("sinal"), d, new Set(), real);
    expect(a).toHaveLength(1);
    expect(a[0]!.key).toBe("AAPL|1d|trap|long|0|gatilho");
    expect(a[0]!.message).toContain("PASSOU DO GATILHO");
    expect(a[0]!.message).toContain("Quantidade: 9 ações");
    expect(alertsFor(watched("sinal"), d, new Set([a[0]!.key]), real)).toHaveLength(0);
  });

  it("flags a late entry and ignores a break after the deadline", () => {
    const late = alertsFor(watched("sinal"), { bars: [bar(2000, 106, 99)], price: 106, delayMin: 0 }, new Set(), paper);
    expect(late[0]!.message).toContain("já correu");
    expect(alertsFor(watched("sinal"), { bars: [bar(2000, 99, 97), bar(12_000, 105, 99)], price: 104, delayMin: 0 }, new Set(), paper)).toHaveLength(0);
  });

  it("alerts the stop before the target on an open trade", () => {
    const a = alertsFor(watched("aberto"), { bars: [bar(2000, 104, 94), bar(3000, 111, 100)], price: 108, delayMin: 0 }, new Set(), paper);
    expect(a.map((x) => x.key)).toEqual(["AAPL|1d|trap|long|0|stopdia"]);
    const b = alertsFor(watched("aberto"), { bars: [bar(3000, 111, 100)], price: 110.5, delayMin: 0 }, new Set(), paper);
    expect(b[0]!.key).toBe("AAPL|1d|trap|long|0|alvodia");
  });

  it("reads an intraday chart", () => {
    const body = { chart: { result: [{ meta: { regularMarketPrice: 10.5, exchangeDataDelayedBy: 15 }, timestamp: [1, 2], indicators: { quote: [{ open: [10, null], high: [11, 12], low: [9, 10], close: [10.5, 11] }] } }] } };
    expect(parseIntraday(body)).toEqual({ bars: [[1000, 10, 11, 9, 10.5]], price: 10.5, delayMin: 15 });
    expect(parseIntraday({})).toBeNull();
  });
});
