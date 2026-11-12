import { describe, expect, it, vi } from "vitest";

vi.mock("server-only", () => ({}));

import { quietHours, staleStep, STALE_MS } from "./watch";

const HOUR = 3_600_000;
// 2026-11-11 15:00 UTC = 12:00 in Brasília.
const noon = Date.UTC(2026, 10, 11, 15);

describe("robot stopped warning", () => {
  it("is quiet from 23h to 7h in Brasília", () => {
    expect(quietHours(noon)).toBe(false);
    expect(quietHours(Date.UTC(2026, 10, 11, 2))).toBe(true); // 23h
    expect(quietHours(Date.UTC(2026, 10, 11, 9, 59))).toBe(true); // 6h59
    expect(quietHours(Date.UTC(2026, 10, 11, 10))).toBe(false); // 7h
  });

  it("warns once per stop and says when the robot is back", () => {
    const stopped = noon - STALE_MS - 1;
    expect(staleStep(noon, noon - HOUR, null)).toEqual({ send: null, notifiedFor: null });
    expect(staleStep(noon, stopped, null)).toEqual({ send: "parado", notifiedFor: String(stopped) });
    expect(staleStep(noon + HOUR, stopped, String(stopped))).toEqual({ send: null, notifiedFor: String(stopped) });
    expect(staleStep(noon + 2 * HOUR, noon + 2 * HOUR - 60_000, String(stopped))).toEqual({ send: "voltou", notifiedFor: null });
  });

  it("waits for the morning to warn about a stop at night", () => {
    const night = Date.UTC(2026, 10, 11, 6); // 3h in Brasília
    const stopped = night - 3 * HOUR;
    expect(staleStep(night, stopped, null).send).toBeNull();
    expect(staleStep(Date.UTC(2026, 10, 11, 10, 15), stopped, null).send).toBe("parado");
  });
});
