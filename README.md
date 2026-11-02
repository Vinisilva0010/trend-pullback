# trend-pullback

Validation-first research tool for a single trading method: buying short pullbacks
inside established uptrends, only when the broad market is also in an uptrend,
with volatility-based stops and fixed risk per trade.

Phase 1 backtests the method on historical daily data (crypto spot via Binance public
archive, US equities via Tiingo) with costs, survivorship-aware universes and
pre-registered acceptance criteria. Phase 2 reuses the same rules crate for daily alerts.

## Workspace

- `tp-core`: indicators and trading rules (shared by backtest and live scanner)
- `tp-data`: download, checksum verification, cleaning, point-in-time universes
- `tp-backtest`: day-by-day portfolio simulation
- `tp-report`: metrics and HTML report
- `tp-cli`: command-line interface (`tp`)

## Checks

    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    cargo audit

Requires Rust 1.98 or newer. No secrets are committed; see `.env.example`.
