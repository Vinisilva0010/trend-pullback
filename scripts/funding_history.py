"""Fetch Binance USDT-M funding history (public API, read-only), save it per
symbol for the simulator, and print the yearly cost for a long."""
import json, time, urllib.request, urllib.error
from collections import defaultdict
from pathlib import Path

FIRST_FUNDING_MS = 1567296000000  # 2019-09-01, start of Binance USDT-M funding history
OUT_DIR = Path("data/processed/binance/funding")


def fetch(symbol):
    out, start = [], FIRST_FUNDING_MS
    while True:
        url = f"https://fapi.binance.com/fapi/v1/fundingRate?symbol={symbol}&startTime={start}&limit=1000"
        try:
            with urllib.request.urlopen(url, timeout=30) as r:
                rows = json.load(r)
        except urllib.error.HTTPError as e:
            raise SystemExit(f"STOPPED: Binance answered {e.code} for {symbol}")
        if not rows:
            break
        out.extend(rows)
        start = rows[-1]["fundingTime"] + 1
        if len(rows) < 1000:
            break
        time.sleep(0.3)
    return out


OUT_DIR.mkdir(parents=True, exist_ok=True)
cutoff = time.time() * 1000 - 365 * 86400 * 1000
for sym in ["BTCUSDT", "ETHUSDT", "SOLUSDT"]:
    rows = fetch(sym)
    lines = ["funding_time_ms,rate"] + [f'{r["fundingTime"]},{r["fundingRate"]}' for r in rows]
    (OUT_DIR / f"{sym}.csv").write_text("\n".join(lines) + "\n")
    years = defaultdict(lambda: [0.0, 0, 0])
    last = [0.0, 0, 0]
    for r in rows:
        rate = float(r["fundingRate"])
        y = time.gmtime(r["fundingTime"] / 1000).tm_year
        for bucket in [years[y]] + ([last] if r["fundingTime"] >= cutoff else []):
            bucket[0] += rate
            bucket[1] += 1
            bucket[2] += rate < 0
    print(f"=== {sym}: {len(rows)} payments saved (positive = long paid) ===")
    for y in sorted(years):
        s, n, neg = years[y]
        print(f"  {y}: {s*100:+6.1f}% of position value | {neg/n*100:3.0f}% negative payments")
    s, n, neg = last
    print(f"  last 12 months: {s*100:+6.1f}% | {neg/n*100:3.0f}% negative payments")
