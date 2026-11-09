import { describe, expect, it } from "vitest";
import { createSession, verifySession } from "./session";

const secret = "s".repeat(40);

describe("session cookie", () => {
  it("accepts its own token and rejects tampering, expiry and other secrets", async () => {
    const now = 1_800_000_000_000;
    const token = await createSession(secret, now);
    expect(await verifySession(token, secret, now + 1000)).toBe(true);
    expect(await verifySession(token, "x".repeat(40), now)).toBe(false);
    expect(await verifySession(token, secret, now + 31 * 24 * 3600 * 1000)).toBe(false);
    const [v, exp, sig] = token.split(".");
    expect(await verifySession(`${v}.${Number(exp) + 1}.${sig}`, secret, now)).toBe(false);
    expect(await verifySession(`${v}.${exp}.${sig}x`, secret, now)).toBe(false);
    expect(await verifySession(undefined, secret, now)).toBe(false);
    expect(await verifySession(token, "short", now)).toBe(false);
  });
});
