"""Section 6: timeout dissection.

Timeouts are usually the biggest reason category and where most $ leak
from. Split by whether they ever brushed a winning level. Complements the
loss-shape section: if timeouts overwhelmingly "never went positive",
entries are the problem; if a chunk went positive then reversed,
timeouts are giving back real gains.
"""

from __future__ import annotations

import pandas as pd

from render import render_table


def _bucket(highest: float) -> str:
    if highest <= 0.0:
        return "never went positive"
    if highest < 0.3:
        return "peaked <+0.3% (noise)"
    if highest < 0.6:
        return "peaked +0.3% to +0.6%"
    return "peaked +0.6% or more"


def compute(reports: pd.DataFrame) -> pd.DataFrame:
    timeouts = reports[reports["reason"] == "timeout"].copy()
    if timeouts.empty:
        return timeouts
    timeouts["bucket"] = timeouts["highest"].apply(_bucket)
    grouped = timeouts.groupby("bucket").agg(
        n=("earnings", "size"),
        total_dollars=("earnings", "sum"),
        avg_dollars=("earnings", "mean"),
        avg_hi=("highest", "mean"),
    )
    order = [
        "never went positive",
        "peaked <+0.3% (noise)",
        "peaked +0.3% to +0.6%",
        "peaked +0.6% or more",
    ]
    grouped = grouped.reindex(order).dropna(how="all")
    grouped.index.name = "peak reached during hold"
    return grouped.reset_index()


def render(reports: pd.DataFrame) -> str:
    df = compute(reports)
    if df.empty:
        return render_table(
            pd.DataFrame({"peak reached during hold": [], "n": []}),
            title="6. Timeout dissection",
            note="No timeouts.",
        )
    df = df.rename(
        columns={"total_dollars": "total_$", "avg_dollars": "avg_$", "avg_hi": "avg_hi%"}
    )
    note = (
        "Rows 3+4 with negative avg_$ = timeouts that reached sell_floor territory "
        "and reversed. Those would have been floor_reached wins with a tighter "
        "sell_floor or a trailing stop."
    )
    return render_table(df, title="6. Timeout dissection", note=note)
