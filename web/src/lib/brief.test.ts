import { describe, expect, it } from "vitest";
import { cryptoBrief, stocksBrief } from "./brief";
import type { Daily, MarketAsset } from "./schema";

const asset = (symbol: string, trend: MarketAsset["trend"], change: number): MarketAsset => ({
  symbol, name: symbol, group: "EUA", price: 100, change_day: change, change_week: change, trend, weekly_trend: trend, daily: [], weekly: [], stormer: [],
});

describe("day brief", () => {
  it("explains crypto with the turn of trend and crowded futures", () => {
    const daily = {
      version: 1, generated_ms: 1, date: "2026-11-10", sections: [],
      assets: [
        { name: "BTC", close: 1, change: -2.1, trend: "queda", previous_trend: "alta", positioning: { long_share: 0.68, long_share_before: 0.6, top_long_share: null, oi_change_24h: null, longs_zone: null, shorts_zone: null, exchanges: [] } },
      ],
    } as Daily;
    const b = cryptoBrief(daily, { label: "CUIDADO", why: "o mercado está em queda" }, null);
    expect(b.tone).toBe("down");
    expect(b.lines[0]).toBe("O mercado está em queda.");
    expect(b.lines.some((l) => l.includes("era alta e agora é queda"))).toBe(true);
    expect(b.lines.some((l) => l.includes("68% do dinheiro"))).toBe(true);
  });

  it("reads stocks from the S&P 500 and the breadth", () => {
    const b = stocksBrief([asset("SPY", "alta", 1.2), asset("AAPL", "alta", 1), asset("XLE", "baixa", -1)], null, 2);
    expect(b.lines[0]).toContain("S&P 500");
    expect(b.lines).toContain("Das 3 ações e ETFs acompanhados, 2 estão em tendência de alta e 1 em queda.");
    expect(b.label).toBe("ALTA FRACA");
  });
});
