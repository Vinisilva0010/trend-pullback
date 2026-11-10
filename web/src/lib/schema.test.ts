import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { EventSchema, IngestSchema, LiveSchema, MoodSchema } from "./schema";

describe("ingest schema", () => {
  it("accepts the snapshot the Rust tool writes", () => {
    const sample = JSON.parse(readFileSync(new URL("./__fixtures__/live.json", import.meta.url), "utf8"));
    const parsed = LiveSchema.safeParse(sample);
    if (!parsed.success) console.error(parsed.error.issues.slice(0, 3));
    expect(parsed.success).toBe(true);
  });

  it("rejects unknown kinds and oversized event lists", () => {
    expect(IngestSchema.safeParse({ kind: "other", data: {}, new_events: [] }).success).toBe(false);
    const event = { id: "a", ms: 1, asset: "BTC", kind: "smc", title: "t", text: "t", notify: true };
    const many = Array.from({ length: 201 }, (_, i) => ({ ...event, id: `e${i}` }));
    expect(IngestSchema.safeParse({ kind: "live", data: {}, new_events: many }).success).toBe(false);
  });

  it("gives old events the tool's defaults", () => {
    const base = { id: "a", ms: 1, title: "t", text: "t" };
    const trade = EventSchema.parse({ ...base, asset: "Apple", kind: "stormer", notify: true });
    expect([trade.level, trade.market, trade.lean]).toEqual([3, "acoes", null]);
    const news = EventSchema.parse({ ...base, asset: "mercado", kind: "noticia", notify: true, level: 3, lean: "queda", market: "cripto" });
    expect([news.level, news.market, news.lean]).toEqual([3, "cripto", "queda"]);
    expect(EventSchema.parse({ ...base, asset: "BTC", kind: "smc", notify: false }).level).toBe(1);
  });

  it("accepts the thermometer the tool sends", () => {
    const item = { title: "Fed signals rate cut", link: "https://a.com/1", source: "a.com", ms: 1, kind: "Juros", lean: "alta", level: 3 };
    const side = { hours: 24, up: 1, down: 0, unclear: 0, weight: 3, score: 1, items: [item] };
    expect(MoodSchema.safeParse({ cripto: side, acoes: side }).success).toBe(true);
    expect(MoodSchema.safeParse({ cripto: side, acoes: { ...side, items: [{ ...item, link: "javascript:alert(1)" }] } }).success).toBe(false);
  });
});
