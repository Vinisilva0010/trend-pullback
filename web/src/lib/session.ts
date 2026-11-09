// Login cookie: "v1.<expiry ms>.<HMAC-SHA256 signature>", signed with
// SESSION_SECRET. Uses Web Crypto so it works in the proxy and in routes.

export const SESSION_COOKIE = "tp_session";
export const SESSION_DAYS = 30;

const encoder = new TextEncoder();

function toBase64Url(bytes: ArrayBuffer): string {
  let text = "";
  for (const b of new Uint8Array(bytes)) text += String.fromCharCode(b);
  return btoa(text).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

function fromBase64Url(value: string): Uint8Array<ArrayBuffer> | null {
  if (!/^[A-Za-z0-9_-]+$/.test(value)) return null;
  const padded = value.replaceAll("-", "+").replaceAll("_", "/") + "===".slice((value.length + 3) % 4);
  try {
    const text = atob(padded);
    const out = new Uint8Array(new ArrayBuffer(text.length));
    for (let i = 0; i < text.length; i++) out[i] = text.charCodeAt(i);
    return out;
  } catch {
    return null;
  }
}

async function hmacKey(secret: string): Promise<CryptoKey> {
  if (secret.length < 32) throw new Error("SESSION_SECRET must be at least 32 characters");
  return crypto.subtle.importKey("raw", encoder.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, [
    "sign",
    "verify",
  ]);
}

export async function createSession(secret: string, now = Date.now()): Promise<string> {
  const expiry = now + SESSION_DAYS * 24 * 3600 * 1000;
  const payload = `v1.${expiry}`;
  const signature = await crypto.subtle.sign("HMAC", await hmacKey(secret), encoder.encode(payload));
  return `${payload}.${toBase64Url(signature)}`;
}

export async function verifySession(token: string | undefined, secret: string | undefined, now = Date.now()): Promise<boolean> {
  if (!token || !secret || secret.length < 32 || token.length > 200) return false;
  const parts = token.split(".");
  if (parts.length !== 3 || parts[0] !== "v1") return false;
  const [, expiryText, signatureText] = parts as [string, string, string];
  if (!/^\d{1,15}$/.test(expiryText)) return false;
  if (Number(expiryText) <= now) return false;
  const signature = fromBase64Url(signatureText);
  if (!signature) return false;
  return crypto.subtle.verify("HMAC", await hmacKey(secret), signature, encoder.encode(`v1.${expiryText}`));
}
