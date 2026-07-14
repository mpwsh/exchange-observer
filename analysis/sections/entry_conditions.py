"""Section 3: entry conditions vs outcome.

This is the section the report has been missing, and its absence is why `min_dip`
kept getting argued about instead of measured.

Every other section starts *after* the entry. They can tell you a trade lost
money; none of them can tell you whether the setup that produced it was any good,
because the setup was never recorded. So the only way to reason about `min_dip`
was from first principles ("0.4% is only twice your costs, there can't be much to
revert from") — which sounds convincing, is untestable, and is exactly the
epistemic position that produced `configure_from_report`.

Now the dip that actually fired is on the row. Bucket by it and read the answer:

- **winnable_%** — did the position's peak ever clear break-even? This is entry
  quality with the exits factored out entirely. If it rises with dip depth, deeper
  dips are better setups and `min_dip` should go up. If it's flat, dip depth is not
  the discriminator and raising the threshold just costs you volume.
- **avg_net$** — what the bucket actually earned.
- **avg_hi%** — how far the winners ran. A deeper dip should give a bounce more
  room; if it doesn't, the reversion premise itself is in question.

Watch `n` per bucket. Four trades in a bucket is a rumor, not a finding.
"""

from __future__ import annotations

import pandas as pd

from render import render_table

# Dip magnitude (positive), in percent. `dip` is stored negative.
_EDGES = [0.0, 0.5, 0.75, 1.0, 1.5, 2.5, float("inf")]
_LABELS = [
    "0.0 - 0.5%",
    "0.5 - 0.75%",
    "0.75 - 1.0%",
    "1.0 - 1.5%",
    "1.5 - 2.5%",
    "2.5%+",
]


def compute(reports: pd.DataFrame) -> pd.DataFrame:
    if reports.empty or "dip" not in reports:
        return pd.DataFrame()

    df = reports.copy()
    # Rows written before these columns existed come back as null -> 0.0, which
    # would pile into the shallowest bucket and poison it. Drop them.
    df = df[df["dip"].fillna(0.0) != 0.0]
    if df.empty:
        return pd.DataFrame()

    df["dip_magnitude"] = df["dip"].abs()
    df["bucket"] = pd.cut(
        df["dip_magnitude"], bins=_EDGES, labels=_LABELS, right=False
    )

    grouped = df.groupby("bucket", observed=False).agg(
        n=("earnings_recomputed", "size"),
        winnable_pct=("winnable", lambda s: 100.0 * s.mean() if len(s) else 0.0),
        total_dollars=("earnings_recomputed", "sum"),
        avg_dollars=("earnings_recomputed", "mean"),
        avg_hi=("highest", "mean"),
        avg_bounce=("bounce", "mean"),
        avg_std=("std_deviation", "mean"),
        avg_spread=("spread_bps", "mean"),
    )
    grouped = grouped[grouped["n"] > 0]
    grouped.index.name = "dip depth at entry"
    return grouped.reset_index()


def render(reports: pd.DataFrame) -> str:
    df = compute(reports)
    if df.empty:
        return render_table(
            pd.DataFrame({"dip depth at entry": [], "n": []}),
            title="3. Entry conditions vs outcome",
            note=(
                "No rows carry entry conditions yet. These columns (dip, bounce, "
                "std_deviation, spread_bps) are written by the scheduler from this "
                "build onward — run a session and they will populate."
            ),
        )

    df = df.rename(
        columns={
            "winnable_pct": "winnable_%",
            "total_dollars": "total_$",
            "avg_dollars": "avg_$",
            "avg_hi": "avg_hi%",
            "avg_bounce": "avg_bounce%",
            "avg_std": "avg_std",
            "avg_spread": "avg_spread_bps",
        }
    )
    note = (
        "winnable_% = share of the bucket whose PEAK cleared break-even, i.e. entry "
        "quality with exits factored out. If it CLIMBS with dip depth, raise min_dip. "
        "If it is FLAT, dip depth is not the discriminator and raising min_dip only "
        "costs you volume. Mind n per bucket — four trades is a rumor, not a finding."
    )
    return render_table(df, title="3. Entry conditions vs outcome", note=note)
