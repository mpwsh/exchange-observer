"""Section 3: earnings breakdown by exit reason.

The key diagnostic. `avg_hi` = how high did positions of this reason type
pop during the hold; `avg_lo` = how low did they dip. For timeouts, if
`avg_hi` is meaningfully positive but `avg_$` is negative, holds are
giving back winners — a trailing-stop conversation. For stoploss /
floor_reached, tells you where exits fired vs where prices actually went.
"""

from __future__ import annotations

import pandas as pd

from render import render_table


def compute(reports: pd.DataFrame) -> pd.DataFrame:
    """Aggregate per-reason stats. Returns a sorted DataFrame."""
    grouped = reports.groupby("reason").agg(
        n=("earnings", "size"),
        total_dollars=("earnings", "sum"),
        avg_dollars=("earnings", "mean"),
        avg_chg=("change", "mean"),
        avg_time_s=("time_left", "mean"),
        avg_hi=("highest", "mean"),
        avg_lo=("lowest", "mean"),
        avg_fees=("fees", "mean"),
    )
    grouped = grouped.sort_values("total_dollars", ascending=False)
    grouped.index.name = "reason"
    return grouped.reset_index()


def render(reports: pd.DataFrame) -> str:
    if reports.empty:
        return render_table(
            pd.DataFrame({"reason": [], "n": []}),
            title="3. Reason breakdown",
            note="No reports.",
        )

    df = compute(reports)
    # Human-friendly column labels for display without touching the data model.
    df = df.rename(
        columns={
            "total_dollars": "total_$",
            "avg_dollars": "avg_$",
            "avg_chg": "avg_chg%",
            "avg_time_s": "avg_time_s",
            "avg_hi": "avg_hi%",
            "avg_lo": "avg_lo%",
            "avg_fees": "avg_fees$",
        }
    )
    note = (
        "avg_hi/avg_lo are the position's peak/trough during the hold. "
        "Timeouts with high avg_hi but negative avg_$ = winners giving gains back."
    )
    return render_table(df, title="3. Reason breakdown", note=note)
