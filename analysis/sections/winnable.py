"""Section 2: winnable trades — the entry-vs-exit question, settled.

Every other section in this report measures what the exits *did*. None of them
asks the prior question: **could this trade have made money at all?**

A round trip costs two taker fees, so a position has to move `breakeven_pct`
(~0.20% at 0.1% taker) just to get back to zero. `reports.highest` records the
peak change the position reached during the hold — the best price it ever
offered. If `highest` never cleared break-even, then selling at the *perfect*
moment would still have lost money. No exit rule can fix that trade: not a
trailing stop, not a wider stoploss, not a longer timeout, not a different
cashout. It was dead the instant it was entered.

That splits every trade three ways:

    never winnable   peak never reached break-even   -> entry problem
    winnable, kept   peak cleared it, and we made $  -> exits working
    winnable, lost   peak cleared it, and we didn't  -> exit problem

The ratio between row 1 and rows 2+3 is the entry hit rate. The ratio between
rows 2 and 3 is exit efficiency. They are completely different failure modes and
the rest of the report cannot tell them apart — which is how you end up tuning a
stoploss when the entries are the thing that's broken.
"""

from __future__ import annotations

import pandas as pd

from render import render_table

_ORDER = [
    "never winnable (peak < breakeven)",
    "winnable — captured",
    "winnable — missed",
]


def _bucket(row: pd.Series) -> str:
    if not row["winnable"]:
        return "never winnable (peak < breakeven)"
    if row["earnings_recomputed"] > 0.0:
        return "winnable — captured"
    return "winnable — missed"


def compute(reports: pd.DataFrame) -> pd.DataFrame:
    if reports.empty or "winnable" not in reports:
        return pd.DataFrame()

    df = reports.copy()
    df["bucket"] = df.apply(_bucket, axis=1)

    grouped = df.groupby("bucket").agg(
        n=("earnings_recomputed", "size"),
        total_dollars=("earnings_recomputed", "sum"),
        avg_dollars=("earnings_recomputed", "mean"),
        avg_hi=("highest", "mean"),
    )
    grouped["share"] = 100.0 * grouped["n"] / len(df)
    grouped = grouped.reindex(_ORDER).dropna(how="all")
    grouped.index.name = "outcome"
    return grouped.reset_index()


def render(reports: pd.DataFrame, breakeven: float) -> str:
    df = compute(reports)
    if df.empty:
        return render_table(
            pd.DataFrame({"outcome": [], "n": []}),
            title="2. Winnable trades",
            note="No reports.",
        )

    total = int(df["n"].sum())
    winnable = int(df[df["outcome"] != _ORDER[0]]["n"].sum())
    captured = int(df[df["outcome"] == _ORDER[1]]["n"].sum())

    entry_rate = 100.0 * winnable / total if total else 0.0
    exit_rate = 100.0 * captured / winnable if winnable else 0.0

    df = df[["outcome", "n", "share", "total_dollars", "avg_dollars", "avg_hi"]]
    df = df.rename(
        columns={
            "share": "share_%",
            "total_dollars": "total_$",
            "avg_dollars": "avg_$",
            "avg_hi": "avg_hi%",
        }
    )

    note = (
        f"Break-even = {breakeven:.3f}% gross (two taker fees). 'Winnable' = the "
        f"position's PEAK cleared it, i.e. some exit existed that made money.  "
        f"ENTRY hit rate {entry_rate:.0f}% ({winnable}/{total} trades were ever "
        f"winnable).  EXIT efficiency {exit_rate:.0f}% ({captured}/{winnable} of "
        f"those were captured).  Low entry rate -> fix entries; exits cannot save "
        f"a trade whose peak never cleared break-even."
    )
    return render_table(df, title="2. Winnable trades", note=note)
