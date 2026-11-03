"""Download Binance USD-M futures 1h klines (monthly archives) with SHA-256
checks and candle validation; one CSV per symbol for the research code."""
import hashlib, io, time, urllib.request, urllib.error, zipfile
from pathlib import Path

BASE = "https://data.binance.vision/data/futures/um/monthly/klines"
SYMBOLS = {"BTCUSDT": (2019, 9), "ETHUSDT": (2019, 11), "SOLUSDT": (2020, 9)}
END = (2026, 10)
OUT = Path("data/processed/binance/futures_1h")
HOUR = 3_600_000


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


OUT.mkdir(parents=True, exist_ok=True)
for sym, start in SYMBOLS.items():
    rows, missing = {}, []
    for y, m in months(start, END):
        name = f"{sym}-1h-{y}-{m:02d}.zip"
        data = get(f"{BASE}/{sym}/1h/{name}")
        if data is None:
            missing.append(f"{y}-{m:02d}")
            continue
        check = get(f"{BASE}/{sym}/1h/{name}.CHECKSUM")
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
            if t % HOUR:
                raise SystemExit(f"STOPPED: candle not on the hour {sym} {t}")
            if t in rows and rows[t] != (o, h, l, c, v):
                raise SystemExit(f"STOPPED: conflicting duplicate {sym} {t}")
            rows[t] = (o, h, l, c, v)
    times = sorted(rows)
    gaps = sum(1 for a, b in zip(times, times[1:]) if b - a != HOUR)
    with open(OUT / f"{sym}.csv", "w") as out:
        out.write("open_time_ms,open,high,low,close,volume\n")
        for t in times:
            o, h, l, c, v = rows[t]
            out.write(f"{t},{o},{h},{l},{c},{v}\n")
    print(f"{sym}: {len(times)} hours, {gaps} gaps, months missing: {missing or 'none'}")
