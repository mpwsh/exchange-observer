"""Analysis entrypoint.

Runs each section against the current Scylla state and writes a single
self-contained HTML file. No external assets, no CSS framework, no JS —
just section headings and inline image tags.

Usage:
    python analyze.py                        # writes to ./output/
    python analyze.py --config ../config.toml --output-dir /tmp/analysis
"""

from __future__ import annotations

import argparse
import datetime as dt
import html
import sys
from pathlib import Path

from db import load_db_config, load_reports, open_session
from pnl import attach, breakeven_pct, load_taker_fee, reconciliation_note

from sections import (
    entry_conditions,
    headline,
    loss_shape,
    reason_breakdown,
    timeout_dissection,
    top_tokens,
    trade_replay,
    winnable,
)


def _default_config_path() -> Path:
    """Look for config.toml next to the analysis dir. Fallback: current dir.

    The scheduler normally runs from repo root and the config is at the
    repo root too; this script lives in `analysis/` so we walk up once.
    """
    here = Path(__file__).resolve().parent
    candidate = here.parent / "config.toml"
    return candidate if candidate.exists() else Path("config.toml")


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate a self-contained HTML analysis report from Scylla data."
    )
    parser.add_argument(
        "--config",
        type=Path,
        default=_default_config_path(),
        help="Scheduler config.toml (for Scylla coordinates). Default: ../config.toml",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path(__file__).resolve().parent / "output",
        help="Where to write the HTML report. Default: ./analysis/output",
    )
    parser.add_argument(
        "--replay-count",
        type=int,
        default=3,
        help="How many top wins and bottom losses to render candle charts for. Default: 3.",
    )
    return parser.parse_args()


# Minimal HTML — no framework, no JS. Monospace + max-width + section
# spacing. That's the whole style budget; if we ever want more, be
# suspicious of the reason.
_HTML_HEAD = """<!DOCTYPE html>
<html><head>
<meta charset="utf-8">
<title>Trading Analysis — {ts}</title>
<style>
  body {{
    background: #14161a; color: #d2d6dc;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    max-width: 1200px; margin: 2em auto; padding: 0 1em;
    line-height: 1.5;
  }}
  h1 {{ color: #78dc82; margin-bottom: 0; }}
  h2 {{ color: #78c8e6; margin-top: 2em; border-bottom: 1px solid #2a2d34; padding-bottom: 4px; }}
  img {{ display: block; margin: 0.5em 0 1.5em 0; max-width: 100%; }}
  .stamp {{ color: #8c919b; margin-top: 0; }}
  .replay-note {{ color: #8c919b; margin: 1em 0; font-style: italic; }}
</style>
</head><body>
"""


def _write_report(sections: dict[str, str | list[str]], output_dir: Path) -> Path:
    """Write the assembled HTML. Returns the path written to."""
    output_dir.mkdir(parents=True, exist_ok=True)
    now = dt.datetime.now(dt.UTC).strftime("%Y-%m-%d-%H-%M-%S")
    path = output_dir / f"analysis-{now}.html"

    with path.open("w", encoding="utf-8") as fh:
        fh.write(_HTML_HEAD.format(ts=now))
        fh.write(f"<h1>Trading Analysis</h1>\n<p class='stamp'>Generated {now} UTC</p>\n")

        # Sections in reading order. Each is one image tag with the section
        # heading inside the image itself (via matplotlib title), so we
        # emit no extra <h2> for those — but replay pages get one shared
        # heading because they're a series of images.
        for title in ("Headline", "Winnable trades", "Entry conditions",
                      "Reason breakdown", "Per-token sequence", "Loss shape",
                      "Timeout dissection"):
            uri = sections.get(title)
            if uri:
                fh.write(f'<img src="{uri}" alt="{html.escape(title)}">\n')

        replay_uris = sections.get("Trade replay", [])
        if isinstance(replay_uris, list) and replay_uris:
            fh.write("<h2>Trade replay</h2>\n")
            fh.write(
                "<p class='replay-note'>Top wins first, then bottom losses. "
                "Yellow marker = entry. Dashed lines = buy_price + sell_floor / cashout / stoploss.</p>\n"
            )
            for uri in replay_uris:
                fh.write(f'<img src="{uri}" alt="Trade replay">\n')

        fh.write("</body></html>\n")
    return path


def main() -> int:
    args = _parse_args()

    if not args.config.exists():
        print(f"Config not found: {args.config}", file=sys.stderr)
        return 2

    db = load_db_config(args.config)
    print(f"Connecting to Scylla at {db.host}:{db.port} keyspace={db.keyspace}")
    session = open_session(db)

    print("Loading reports...")
    reports = load_reports(session, db.keyspace)
    print(f"Loaded {len(reports)} reports")

    if reports.empty:
        print("No reports to analyze. Exiting.")
        return 0

    fees_null = reports.attrs.get("fees_null_count", 0)
    if fees_null:
        print(
            f"Note: {fees_null} rows had null `fees` (pre-fee-accounting rows). "
            "Treated as 0.0 for aggregates."
        )

    # Recompute P&L from buy_price/sell_price/fees rather than trusting the
    # stored `earnings` column. The scheduler double-charged the entry fee for
    # weeks and nothing here could tell; see pnl.py.
    taker_fee = load_taker_fee(args.config)
    breakeven = breakeven_pct(taker_fee)
    reports = attach(reports, taker_fee)
    print(f"Taker fee {taker_fee:.3f}% -> round-trip break-even {breakeven:.3f}% gross")

    # Every downstream section aggregates `earnings`. Repoint it at the
    # recomputed value in one place, so no section can accidentally report the
    # stored one. The original is kept for the reconciliation check.
    reports["earnings_stored"] = reports["earnings"]
    reports["earnings"] = reports["earnings_recomputed"]

    warning = reconciliation_note(reports)
    if warning:
        print(f"\n  !! {warning}\n", file=sys.stderr)

    sections: dict[str, str | list[str]] = {}
    print("Rendering sections...")
    sections["Headline"] = headline.render(reports, breakeven)
    sections["Winnable trades"] = winnable.render(reports, breakeven)
    sections["Entry conditions"] = entry_conditions.render(reports)
    sections["Reason breakdown"] = reason_breakdown.render(reports)
    sections["Per-token sequence"] = top_tokens.render(reports)
    sections["Loss shape"] = loss_shape.render(reports)
    sections["Timeout dissection"] = timeout_dissection.render(reports)

    print(f"Rendering {args.replay_count * 2} trade replay charts...")
    sections["Trade replay"] = trade_replay.render(
        reports, session, db.keyspace, top_n=args.replay_count
    )

    path = _write_report(sections, args.output_dir)
    print(f"Report ready: {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
