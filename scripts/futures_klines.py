"""Download Binance USD-M futures klines of any interval (monthly archives)
with SHA-256 checks and candle validation; one CSV per symbol.

Usage: python3 scripts/futures_klines.py 15m
Output: data/processed/binance/futures_<interval>/<SYMBOL>.csv
"""
import hashlib, io, sys, time, urllib.request, urllib.error, zipfile
from pathlib import Path

STEPS = {"5m": 300_000, "15m": 900_000, "30m": 1_800_000, "1h": 3_600_000}
BASE = "https://data.binance.vision/data/futures/um/monthly/klines"
# First month with a full archive for each perpetual.
SYMBOLS = {
    "BTCUSDT": (2019, 9),
    "ETHUSDT": (2019, 11),
    "SOLUSDT": (2020, 9),
    "XRPUSDT": (2020, 1),
    "BNBUSDT": (2020, 2),
    "DOGEUSDT": (2020, 7),
}
END = (2026, 10)


def get(url):
    for attempt in range(5):
        try:
            with urllib.request.urlopen(url, timeout=60) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            if e.code == 404:
                return None
            time.sleep(2 ** attempt)
        except urllib.error.URLError:
            time.sleep(2 ** attempt)
    raise SystemExit(f"STOPPED: could not download {url}")


def months(start, end):
    y, m = start
    while (y, m) <= end:
        yield y, m
        m += 1
        if m == 13:
            y, m = y + 1, 1


interval = sys.argv[1] if len(sys.argv) > 1 else "15m"
if interval not in STEPS:
    raise SystemExit(f"STOPPED: interval must be one of {sorted(STEPS)}")
step = STEPS[interval]
out_dir = Path(f"data/processed/binance/futures_{interval}")
out_dir.mkdir(parents=True, exist_ok=True)
for sym, start in SYMBOLS.items():
    rows, missing = {}, []
    for y, m in months(start, END):
        name = f"{sym}-{interval}-{y}-{m:02d}.zip"
        data = get(f"{BASE}/{sym}/{interval}/{name}")
        if data is None:
            missing.append(f"{y}-{m:02d}")
            continue
        check = get(f"{BASE}/{sym}/{interval}/{name}.CHECKSUM")
        if check is None or check.split()[0].decode() != hashlib.sha256(data).hexdigest():
            raise SystemExit(f"STOPPED: checksum mismatch for {name}")
        with zipfile.ZipFile(io.BytesIO(data)) as z:
            text = z.read(z.namelist()[0]).decode()
        for line in text.splitlines():
            f = line.split(",")
            if not f[0].isdigit():
                continue
            t = int(f[0])
            t = t // 1000 if t > 10**14 else t
            o, h, l, c, v = map(float, f[1:6])
            if not (0 < l <= min(o, c) and h >= max(o, c) and v >= 0):
                raise SystemExit(f"STOPPED: invalid candle {sym} {t}")
            if t % step:
                raise SystemExit(f"STOPPED: candle off the {interval} grid {sym} {t}")
            if t in rows and rows[t] != (o, h, l, c, v):
                raise SystemExit(f"STOPPED: conflicting duplicate {sym} {t}")
            rows[t] = (o, h, l, c, v)
    times = sorted(rows)
    gaps = sum(1 for a, b in zip(times, times[1:]) if b - a != step)
    with open(out_dir / f"{sym}.csv", "w") as out:
        out.write("open_time_ms,open,high,low,close,volume\n")
        for t in times:
            o, h, l, c, v = rows[t]
            out.write(f"{t},{o},{h},{l},{c},{v}\n")
    print(f"{sym}: {len(times)} candles, {gaps} gaps, months missing: {missing or 'none'}", flush=True)
