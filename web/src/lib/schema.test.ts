import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { IngestSchema, LiveSchema } from "./schema";

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
});
