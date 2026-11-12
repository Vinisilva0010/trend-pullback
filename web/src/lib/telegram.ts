import "server-only";
import { env } from "./env";

// Telegram messages sent from the cloud (the watch and the "robot stopped"
// warning). The bot token stays in the Vercel environment and never in logs.

const MAX_CHARS = 3800;

export function telegramConfigured(): boolean {
  return /^\d+:[\w-]{30,}$/.test(env("TELEGRAM_BOT_TOKEN")) && /^-?\d+$/.test(env("TELEGRAM_CHAT_ID"));
}

/** Sends one message (cut at Telegram's size limit); false when it fails or is not configured. */
export async function sendTelegram(text: string): Promise<boolean> {
  if (!telegramConfigured()) return false;
  try {
    const r = await fetch(`https://api.telegram.org/bot${env("TELEGRAM_BOT_TOKEN")}/sendMessage`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ chat_id: env("TELEGRAM_CHAT_ID"), text: text.slice(0, MAX_CHARS), disable_web_page_preview: true }),
      signal: AbortSignal.timeout(10_000),
      cache: "no-store",
    });
    return r.ok;
  } catch {
    return false;
  }
}
