import "server-only";
import { createHash, timingSafeEqual } from "node:crypto";

/** Constant-time comparison of two secrets (hashing first evens the lengths). */
export function sameSecret(given: string, expected: string): boolean {
  if (!expected) return false;
  const a = createHash("sha256").update(given).digest();
  const b = createHash("sha256").update(expected).digest();
  return timingSafeEqual(a, b);
}

export function bearer(header: string | null): string {
  if (!header?.startsWith("Bearer ")) return "";
  return header.slice("Bearer ".length).trim();
}
