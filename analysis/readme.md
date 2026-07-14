# analysis

Post-hoc analysis of scheduler runs. Reads reports + candles from Scylla,
writes a single self-contained HTML report with tables and candle charts.

## Setup

Requires [uv](https://docs.astral.sh/uv/) and a running Scylla with the
scheduler's schema.

```sh
uv sync
```

That creates `.venv/`, resolves the lockfile, and installs everything.

## Run

```sh
uv run python -m analyze
```

Writes `output/analysis-YYYY-MM-DD-HH-MM-SS.html`. Open in a browser.

By default reads Scylla coordinates from `../config.toml` (the scheduler's
config). Override with flags:

```sh
uv run python -m analyze \
  --config /path/to/config.toml \
  --output-dir /tmp/reports \
  --replay-count 5
```

## Layout

- `analyze.py` — entrypoint, assembles the HTML
- `db.py` — Scylla loaders (only place that touches the driver)
- `render.py` — matplotlib helpers (tables + candle charts as base64 PNGs)
- `sections/` — one module per report section
