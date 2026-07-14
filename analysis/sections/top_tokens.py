"""Section 4: per-token trade sequence.

For tokens with 3+ trades, show chronological W/L/· pattern and net earnings.
Reveals whether frequently-traded tokens were in a losing streak (persistent
bad regime — deny-list candidate) or oscillating (regime unclear).

Legend: W = earnings > +$0.05,  L = earnings < -$0.05,  · = flat.
"""

from __future__ import annotations

import pandas as pd

from render import render_table


def _classify(earnings: float) -> str:
    if earnings > 0.05:
        return "W"
    if earnings < -0.05:
        return "L"
    return "·"


def compute(reports: pd.DataFrame, min_trades: int = 3) -> pd.DataFrame:
    """Group by token, produce sequence + net."""
    if reports.empty:
        return reports.iloc[0:0]  # empty frame with same schema

    # Sort by timestamp within each token before joining W/L glyphs.
    per_token = (
        reports.sort_values("ts")
        .groupby("instid")
        .agg(
            n=("earnings", "size"),
            net_dollars=("earnings", "sum"),
            sequence=("earnings", lambda s: "".join(_classify(v) for v in s)),
        )
    )
    per_token = per_token[per_token["n"] >= min_trades].sort_values("net_dollars")
    per_token.index.name = "instid"
    return per_token.reset_index()


def render(reports: pd.DataFrame) -> str:
    df = compute(reports)
    if df.empty:
        return render_table(
            pd.DataFrame({"instid": [], "n": [], "net_$": [], "sequence": []}),
            title="4. Per-token sequence",
            note="No tokens with 3+ trades.",
        )
    df = df.rename(columns={"net_dollars": "net_$", "sequence": "sequence (oldest → newest)"})
    note = "Sorted worst net first. W = won, L = lost, · = ~flat."
    return render_table(df, title="4. Per-token sequence (3+ trades)", note=note)
