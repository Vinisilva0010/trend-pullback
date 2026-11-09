import { describe, expect, it } from "vitest";
import { env } from "./env";

describe("env", () => {
  it("drops Windows line endings and spaces around the value", () => {
    process.env.TP_TEST_VALUE = "senha-forte-123\r\n";
    expect(env("TP_TEST_VALUE")).toBe("senha-forte-123");
    expect(env("TP_TEST_MISSING")).toBe("");
  });
});
