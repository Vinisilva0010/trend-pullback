# trend-pullback web dashboard

Next.js app that shows what the Rust tool publishes: daily report, live
SMC/Wyckoff setups drawn on Binance candles, paper books and the alert
feed, with web push notifications. Deployed on Vercel (Hobby plan) with
Upstash Redis (free plan) as the only storage.

## How data flows

1. `tp alerts` (hourly), `tp daily` and `tp markets` (21:15) write
   `data/live/dashboard_*.json`.
2. When the tool's `.env` has `DASHBOARD_URL` and `DASHBOARD_TOKEN`, they
   `POST {DASHBOARD_URL}/api/ingest` with `Authorization: Bearer <token>`.
3. `/api/ingest` validates the body with zod, stores it in Redis and sends a
   push notification for each new event marked `notify` (at most 4, then a
   summary).
4. Pages read Redis on each request; candles come straight from Binance's
   public market-data API in the browser (`/api/klines` is the fallback).

## Security

- Single password (`DASHBOARD_PASSWORD`), compared in constant time; 5 wrong
  attempts lock that IP for 15 minutes.
- Session cookie: HMAC-SHA256 signed, httpOnly, secure, SameSite=Lax, 30 days.
- `proxy.ts` redirects every page to `/login` without a valid session and
  sets a strict nonce-based Content Security Policy.
- `/api/ingest` needs `INGEST_TOKEN` (constant-time compare), limits the body
  to 4 MB and rejects anything that does not match the schema.
- No exchange keys anywhere: the dashboard only reads public market data.

## Local run

```bash
npm install
cp .env.example .env.local   # fill DASHBOARD_PASSWORD, SESSION_SECRET, INGEST_TOKEN
npm run dev                  # http://localhost:3000, data stored in .data/
npm test
npm run build
```

## Environment variables (Vercel)

| Name | What |
| --- | --- |
| `DASHBOARD_PASSWORD` | Login password, 12+ characters |
| `SESSION_SECRET` | 32+ random characters |
| `INGEST_TOKEN` | Same value as `DASHBOARD_TOKEN` in the tool's `.env` |
| `VAPID_PUBLIC_KEY`, `VAPID_PRIVATE_KEY` | From `npm run vapid` |
| `VAPID_SUBJECT` | `mailto:` address |
| `CRON_SECRET` | Optional. Random secret (32+ characters) for the cloud watch |
| `TELEGRAM_BOT_TOKEN`, `TELEGRAM_CHAT_ID` | Optional. Same values as in the tool's `.env`, for the cloud watch |
| `STOCK_ACCOUNT_USD` | Optional. Real US stock balance, for the share count in buy alerts |
| `UPSTASH_REDIS_REST_URL`/`_TOKEN` or `KV_REST_API_URL`/`_TOKEN` | Added by the Upstash integration |

Set the Vercel project's Root Directory to `web`.

## Cloud watch

Vercel's free plan schedules a function once a day at most, so an outside
scheduler (cron-job.org, free) calls `GET /api/cron/watch` every 15 minutes
with the header `Authorization: Bearer <CRON_SECRET>`. Each call:

- warns on Telegram when the computer's robot has not published for two
  hours (not between 23:00 and 07:00 Brasília), and again when it is back;
- during the New York session, checks the US stock signals and open trades
  (the list the tool sends with the Mercados data) against Yahoo's
  15-minute candles and alerts when a trigger, stop or target is hit.

While this runs (a call in the last 40 minutes), the tool on the computer
skips its own hourly stock watch (`GET /api/watch/status`, ingest token).
Crypto stays on the computer: Binance refuses most cloud servers.

## Notifications on iPhone

iOS only delivers web push to apps added to the home screen: open the site
in Safari, Share, "Add to Home Screen", open it from the icon, then turn
notifications on in "Avisos".
