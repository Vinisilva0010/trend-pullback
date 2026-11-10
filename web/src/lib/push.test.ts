import { describe, expect, it, vi } from "vitest";

vi.mock("server-only", () => ({}));
vi.mock("./store", () => ({ KEYS: { subscriptions: "s" }, store: () => ({}) }));

const { payloadsFor } = await import("./push");

describe("push payloads", () => {
  it("sends only important events and folds the rest into one summary", () => {
    const base = { ms: 1, asset: "BTC", kind: "smc", level: 2, lean: null, market: "cripto" as const };
    const events = Array.from({ length: 7 }, (_, i) => ({ ...base, id: `e${i}`, title: `Título ${i}`, text: `Título ${i}\nLinha ${i}`, notify: i !== 3 }));
    const out = payloadsFor(events);
    expect(out).toHaveLength(5);
    expect(out[0]).toMatchObject({ title: "Título 0", body: "Linha 0", url: "/avisos#e0" });
    expect(out[4]?.title).toBe("Mais 2 avisos novos");
  });
});
