import { describe, expect, it } from "vitest";
import { bookRs, bookSetup, MIN_TRADES, score, setupInfo, verdict } from "./score";
import type { Book } from "./schema";

describe("score", () => {
  it("counts wins, the mean, the total and the longest losing run", () => {
    const s = score([2, -1, -1, -1, 2, -1]);
    expect(s).toMatchObject({ trades: 6, wins: 2, totalR: 0, meanR: 0, worstStreak: 3, bestR: 2, worstR: -1 });
    expect(s.winShare).toBeCloseTo(1 / 3);
    expect(s.luck!.low).toBeLessThan(0);
    expect(s.luck!.high).toBeGreaterThan(0);
    expect(score([]).trades).toBe(0);
    expect(score([1]).luck).toBeNull();
  });

  it("says nothing before enough trades and compares with the test after", () => {
    const ref = setupInfo("acoes-1d").reference;
    expect(verdict(score([]), ref).text).toMatch(/Nenhum/);
    expect(verdict(score([2, 2, 2]), ref).tone).toBe("neutral");
    const mixed = Array.from({ length: 40 }, (_, i) => (i % 2 ? 2 : -1.4));
    expect(verdict(score(mixed), ref).text).toMatch(/Dentro do esperado/);
    const losing = Array.from({ length: MIN_TRADES * 2 }, (_, i) => (i % 5 ? -1 : 0.5));
    expect(verdict(score(losing), ref).tone).toBe("bad");
    const winning = Array.from({ length: MIN_TRADES * 2 }, (_, i) => (i % 5 ? 2 : -1));
    expect(verdict(score(winning), ref).tone).toBe("good");
    expect(verdict(score(winning), null).text).toMatch(/Ganhando/);
  });

  it("splits the paper books by setup", () => {
    expect(bookSetup("acoes", "NVDA|1w|trap|long|1")).toBe("acoes-1w");
    expect(bookSetup("acoes", "NVDA|1d|trap|long|1")).toBe("acoes-1d");
    expect(bookSetup("4h", "BTCUSDT|4h|long|1")).toBe("cripto-smc");
    expect(bookSetup("xx", "k")).toBeNull();
    const trade = (key: string, r: number, exit_ms: number) => ({ key, asset: "X", side: "long" as const, fill_ms: 0, exit_ms, entry: 1, stop: 0.9, exit: 1.2, r, status: "target", equity_after: 1 });
    const books: Book[] = [
      { tf: "acoes", label: "a", start_ms: 0, capital: 1, equity: 1, trades: [trade("A|1d|trap|long|1", 2, 5), trade("B|1d|trap|long|2", -1, 3), trade("C|1w|trap|long|3", 1, 4)] },
      { tf: "1h", label: "b", start_ms: 0, capital: 1, equity: 1, trades: [trade("BTCUSDT|1h|long|1", -1, 1)] },
    ];
    const rs = bookRs(books);
    expect(rs.get("acoes-1d")).toEqual([-1, 2]);
    expect(rs.get("acoes-1w")).toEqual([1]);
    expect(rs.get("cripto-smc")).toEqual([-1]);
  });
});
