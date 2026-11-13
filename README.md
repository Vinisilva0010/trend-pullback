# trend-pullback

A trading research and alert tool. Every setup is tested on years of history
before it can send an alert. A setup only alerts if it made money in both
test periods and also beat a simple base: entering on any candle with the
same stop and target. Nothing here places orders. There are no exchange
keys, and every trade in the books is a paper trade.

## What runs every day

Two scripts run on the Windows computer through WSL, started by the Windows
Task Scheduler.

| Script | When | What it does |
| --- | --- | --- |
| `scripts/alerts.sh` | every hour | `tp alerts --notify`: BTC, ETH and SOL. Breakouts of the 20 and 60 day high or low, key levels, news, the Stormer trap on the daily and weekly charts, and the Smart Money setups (dashboard only). |
| `scripts/daily.sh` | 21:15 Brasília | `tp daily --notify`: the written daily report. Then `tp markets --notify`: the stocks, the Mercados tab and the US stock trades. |

Each run writes to a log in `data/live/`:

- `alerts.log` and `run.log` get a start line and an end line with the exit
  code. `0` means it went fine. `124` means the run was cut by the time limit
  (9 minutes for alerts, 10 for markets), usually a slow network. The next
  run starts from the saved state, so nothing is lost.
- If no new lines show up in a log, the computer was asleep or off at that
  time. Keep sleep set to "Never" while plugged in.

## What is followed, and why

All results are from the history tests (`tp stormer-study`,
`tp market-study --universe`). Every test charges costs on each trade. The
development period runs until October 2024. The final period runs from
November 2024 to today.

**US stocks** (84 stocks and ETFs, `config/trade_universe.csv`). These are
the stock trades: Telegram, phone and their own paper book.

- Daily trap buy with the trend (8 and 80 day averages): +0.15R per trade in
  both periods. The base was +0.07R, then -0.06R. About 1.3 trades a week.
- Weekly trap buy: +0.27R, then +0.19R. The base was +0.24R, then +0.12R. It
  beat the base by a small margin. About 2.3 trades a week.
- Together, about 3.6 signals a week, which is less than one per trading day.

**Brazilian stocks** (62 assets): nothing beat the base. They are shown on
the dashboard without signals.

**BTC, ETH and SOL**: the trap (daily and weekly, buys and sells) was above
the base in both periods, but without enough trades to prove it. It sends
alerts marked "EM OBSERVAÇÃO" and has its own paper book. The daily 1-2-3
buy did not beat the base, so it is shown on the dashboard only.

**Not followed** (they lost in the tests): the mechanical Smart Money setup
on every chart, every sell on stocks, and every day-trade setup on 15-minute
and 1-hour charts, the base included.

## Alert stages

Each followed setup sends up to four messages:

1. **SINAL**: the setup appeared. It gives the trigger price, stop, target
   (2 times the risk) and deadline. Buy only if the price passes the trigger
   before the deadline: the next session for daily signals, the next week for
   weekly ones. Deadlines are in Brasília time. For US stocks, the deadline is
   the 16:00 New York close, and a Friday signal lasts until Monday's close.
2. **ENTROU**: the price passed the trigger. It says whether the trade went
   into the paper book, and if not, why.
3. **SAIU**: the trade hit the stop or the target, or reached its time limit
   (30 days on the daily chart, 12 weeks on the weekly). The result includes
   costs.
4. **VENCEU**: the trigger was not reached in time. Nothing to do.

**US stocks during the session.** Nomad takes only market and limit
orders, so the trap's buy stop and its stop and target cannot be left as
orders. Each hourly run during the New York session checks the stocks with
a signal or an open trade and alerts when the price passes the trigger
(with the number of whole shares to buy, for `STOCK_ACCOUNT_USD` in `.env`
or the paper balance), hits the stop or reaches the target. The test bought
exactly at the trigger: the alert says how much buying later changes the
reward to risk. The paper book still records entries and exits from closed
candles.

Only one "SINAL" is sent per setup. Sent stages are saved, so a message is
never repeated. On the first stock run, only the current signals are sent.
Trades still open from earlier weeks are marked as seen, without a message.

## Paper books

Each book starts with $2,200. Each trade risks 2% of the balance. A short
stop never makes the position larger than the cap below, so the risk can be
lower than 2%.

| Book | Trades | Position cap |
| --- | --- | --- |
| Ações EUA (trap) | US daily trap buys with the trend, and weekly trap buys. At most one new trade a day (daily before weekly, then list order), and three open at once. | a third of the balance |
| Stormer diário (trap) | BTC, ETH and SOL daily traps, buys and sells | the whole balance |
| curto (1h), swing (4h) | Smart Money springs and UTADs, still in test | the whole balance |

A book starts the first time the tool runs with it. Trades from before that
are not counted. The "Contas" tab shows every book with its balance curve.

## Dashboard and phone

The web app in `web/` runs on Vercel (free plan) with Upstash Redis (free
plan). After each run, the tool sends its state to the app. The app shows the
live setups, charts, the Mercados tab, the paper books and the alert feed. It
also sends phone notifications. See `web/README.md` for setup and security.

## Setup

1. Copy `.env.example` to `.env` and fill it in. `.env` is never committed.
2. Build: `cargo build --release`.
3. Test without sending anything: `./target/release/tp markets --dry-run`
   and `./target/release/tp alerts --dry-run`.

`.env` keys:

- `TELEGRAM_BOT_TOKEN`, `TELEGRAM_CHAT_ID`: the Telegram bot that gets the
  alerts.
- `DASHBOARD_URL`, `DASHBOARD_TOKEN`: the dashboard address (`https://...`)
  and the same token as `INGEST_TOKEN` on Vercel. Leave them empty to run
  without the dashboard.
- `TIINGO_API_TOKEN`: only for the old US equities backtest.

Stock and ETF prices come from Yahoo Finance's public chart data, with about
15 minutes of delay. Crypto prices come from Binance's public data.

## Lists

- `config/watchlist.csv`: the assets on the Mercados tab (`group,symbol,name`).
- `config/trade_universe.csv`: the larger list for the studies. Its US part
  is searched for the stock trades.

A US stock outside the watchlist shows up on the Mercados tab only while it
has a signal or an open trade.

## Files in `data/live/`

| File | What it holds |
| --- | --- |
| `smc_books_v2.csv` | start time of each paper book |
| `smc_trades_v2.csv` | every closed paper trade, of every book |
| `stock_sent.txt` | stock stages already sent, and the trades that went into the book (`|conta`) |
| `stormer_sent.txt`, `smc_sent.txt` | crypto stages already sent |
| `events.jsonl` | the last 1000 alert feed items |
| `dashboard_*.json` | the last state sent to the dashboard |

Do not edit these files by hand while a run is going. To restart a book from
zero, stop the scheduled tasks and remove its lines from `smc_trades_v2.csv`
and `smc_books_v2.csv`.

## Studies

| Command | What it tests |
| --- | --- |
| `tp market-study --universe` | Stormer setups on the 146 stocks and ETFs, each group pooled, against the base |
| `tp market-study` | the same on the watchlist |
| `tp stormer-study` | Stormer setups on BTC, ETH and SOL, against the base |
| `tp smc-study` | the Smart Money / Wyckoff setup per chart |
| `tp runner-study` | long trades with no fixed target: a trailing stop (highest high minus 3 ATRs) on the weekly and daily traps and on 52-week and all-time high breakouts, counting how many trades reach 3R, 5R, 10R and 20R, against the base |
| `tp trend-study` | the long trend: hold only while the weekly close is above its 10-month average, against buying and holding, with the worst fall of each |
| `tp ipo-study` | ways of buying a new listing (first day, first-day and first-week breakouts, return to the IPO price) on the US IPOs of $300 million or more since 2019, against the S&P 500; list from the Nasdaq IPO calendar. `tp markets` also follows the calendar: upcoming IPOs on the IPOs page, alerts for the ones of $1 billion or more, and the large recent listings as a group on Mercados |
| `tp refine-study` | the two US stock setups that passed, split by seven conditions known at the signal (S&P 500 above its 200-day average, relative strength, volume, stop size, distance from the 52-week high, close of the signal candle, weekly trend); a condition counts only when it is better in both periods |
| `tp crypto-study` | the Stormer setups on the daily and weekly charts of every coin of the crypto trade universe (`universe.csv`), a coin counting only in the months it was in it; a setup passes only with proof against the base, the same without BTC, ETH and SOL, twice the costs and trades spread across coins |
| `tp support-study` | buying at the most-touched daily levels: a quick bounce in up and down markets, a pullback in an uptrend, and two entries in a fall (support and the liquidity zone below) with quick or late break-even; BTC, ETH and SOL followed hour by hour, the other universe coins on daily candles; each plan against buying on any day in the same situation, plus the depth of past falls |
| `tp day-study` | one trade a day on 15m and 1h charts (needs `python3 scripts/futures_klines.py 15m` and `1h` first) |

Every result line shows trades per week and its verdict: "GANHOU NOS DOIS
PERÍODOS E GANHOU DA BASE", positive without proof, did not beat the base,
or "PERDEU".

## Workspace

- `tp-core`: indicators and trading rules (Stormer setups, Smart Money, trend
  pullback), shared by the studies and the live alerts
- `tp-data`: downloads (Binance archives, Yahoo Finance), checks, Telegram
  and dashboard publishing
- `tp-backtest`: day-by-day portfolio simulation
- `tp-report`: metrics and the HTML report
- `tp-cli`: the `tp` command
- `web/`: the dashboard (Next.js)

## Checks

    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    cargo audit
    cd web && npm test && npm run build

Requires Rust 1.98 or newer. No secrets are committed.
