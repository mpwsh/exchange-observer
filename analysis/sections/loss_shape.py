"""Section 5: loss shape — entry-side or exit-side?

For losing trades, was the failure entry-side (straight down from entry)
or exit-side (touched profitable territory, gave it back)?

Bucket a losing trade as:
- "bad entry" if it never got above +0.1% during the hold
- "bad exit" if it touched at least +0.3% then ended negative
- "middle" otherwise

Fix direction is entry-tune vs exit-tune, and they're different code changes.
"""

from __future__ import annotations

import pandas as pd

from render import render_table


def _bucket(highest: float) -> str:
    if highest < 0.1:
        return "bad entry (never rose above +0.1%)"
    if highest < 0.3:
        return "middle (peaked +0.1% to +0.3%)"
    return "bad exit (peaked +0.3% or more)"


def compute(reports: pd.DataFrame) -> pd.DataFrame:
    losers = reports[reports["earnings"] < 0].copy()
    if losers.empty:
        return losers
    losers["bucket"] = losers["highest"].apply(_bucket)
    grouped = losers.groupby("bucket").agg(
        n=("earnings", "size"),
        total_dollars=("earnings", "sum"),
        avg_dollars=("earnings", "mean"),
    )
    # Fixed order: bad-entry first, then middle, then bad-exit.
    order = [
        "bad entry (never rose above +0.1%)",
        "middle (peaked +0.1% to +0.3%)",
        "bad exit (peaked +0.3% or more)",
    ]
    grouped = grouped.reindex(order).dropna(how="all")
    grouped.index.name = "loss shape"
    return grouped.reset_index()


def render(reports: pd.DataFrame) -> str:
    df = compute(reports)
    if df.empty:
        return render_table(
            pd.DataFrame({"loss shape": [], "n": []}),
            title="5. Loss shape",
            note="No losing trades.",
        )
    df = df.rename(columns={"total_dollars": "total_$", "avg_dollars": "avg_$"})
    note = (
        "bad_entry dominant → entries catch tops; tighten entry filter. "
        "bad_exit dominant → entries fine, exits leave money on table; tighten sell_floor or add trailing stop."
    )
    return render_table(df, title="5. Loss shape (entry-side vs exit-side)", note=note)
