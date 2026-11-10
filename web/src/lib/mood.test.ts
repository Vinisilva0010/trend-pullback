import { describe, expect, it } from "vitest";
import { newsReading, priceReading, verdict } from "./mood";

const mood = (score: number, weight: number) => ({ hours: 24, up: 0, down: 0, unclear: 0, weight, score, items: [] });

describe("thermometer", () => {
  it("needs enough news to read it", () => {
    expect(newsReading(mood(0.9, 2)).reading).toBe("pouca");
    expect(newsReading(mood(0.5, 10)).reading).toBe("alta");
    expect(newsReading(mood(-0.3, 10)).reading).toBe("queda");
    expect(newsReading(null).reading).toBe("pouca");
  });

  it("mixes trend breadth and the change", () => {
    expect(priceReading({ up: 3, down: 0, total: 3, change: 2, scale: 2 }).reading).toBe("alta");
    expect(priceReading({ up: 3, down: 0, total: 3, change: -2, scale: 2 }).reading).toBe("neutro");
    expect(priceReading({ up: 0, down: 30, total: 40, change: -0.5, scale: 1 }).reading).toBe("queda");
  });

  it("says when news and price disagree", () => {
    expect(verdict("alta", "alta").label).toBe("Alta");
    expect(verdict("alta", "queda").label).toBe("Dividido");
    expect(verdict("neutro", "queda").label).toBe("Queda fraca");
    expect(verdict("pouca", "neutro").label).toBe("Neutro");
  });
});
