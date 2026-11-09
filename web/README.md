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
| `UPSTASH_REDIS_REST_URL`/`_TOKEN` or `KV_REST_API_URL`/`_TOKEN` | Added by the Upstash integration |

Set the Vercel project's Root Directory to `web`.

## Notifications on iPhone

iOS only delivers web push to apps added to the home screen: open the site
in Safari, Share, "Add to Home Screen", open it from the icon, then turn
notifications on in "Avisos".
