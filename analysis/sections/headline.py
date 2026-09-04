"""Section 1: headline stats — one glance at the run's health.

Now reports on `earnings_recomputed` rather than the stored `earnings`, and
surfaces the two numbers that were missing and mattered most:

- **The gross / fees / net split.** Total $ alone cannot tell you whether you are
  losing on direction or bleeding on costs. Those want completely different
  fixes, and for a 26-trade run where fees were 79% of the loss, printing only
  the net number actively hides the diagnosis.

- **Break-even.** Every trade is a taker on both legs, so a flat round trip costs
  ~0.20%. Nothing in this report used to say so, which made it easy to look at a
  position that peaked at +0.12% and think of it as "nearly a winner". It wasn't
  near anything. It was never a trade.

If the stored earnings disagree with the recomputed ones, that's printed loudly
rather than silently averaged in. See `pnl.py` for why we no longer trust the
column.
"""

from __future__ import annotations

import pandas as pd

from pnl import reconciliation_note
from render import render_text_block


def render(reports: pd.DataFrame, breakeven: float) -> str:
    """Return an HTML fragment (image tag) with the headline card."""
    if reports.empty:
        return render_text_block("No reports.", title="Headline")

    n = len(reports)
    net = reports["earnings_recomputed"]
    fees_total = reports["fees"].sum()

    total = net.sum()
    gross = total + fees_total

    first_ts = reports["ts"].min()
    last_ts = reports["ts"].max()
    minutes = max((last_ts - first_ts).total_seconds() / 60.0, 1e-9)

    # Net-positive counts as a win. Small wins matter for a cover-the-fees
    # strategy, so this is deliberately not a deep-in-the-money bar.
    wins = int((net > 0.0).sum())
    win_rate = 100.0 * wins / n

    winnable = int(reports["winnable"].sum())
    avg_gross_pct = reports["gross_pct"].mean()

    lines = [
        f"Trades           {n}   ({n / minutes:.2f}/min over {minutes:.0f} min)",
        f"Net $            {total:+.2f}   ({net.mean():+.4f} / trade)",
        f"  Gross $        {gross:+.2f}   ({gross / n:+.4f} / trade)",
        f"  Fees $         {-fees_total:.2f}   ({fees_total / n:.4f} / trade)",
        "",
        f"Avg gross move   {avg_gross_pct:+.4f}%   (break-even is {breakeven:+.3f}%)",
        f"Win rate         {win_rate:.1f}%  ({wins} of {n})",
        f"Ever winnable    {100.0 * winnable / n:.1f}%  ({winnable} of {n} peaked above break-even)",
        "",
        f"Biggest win      {net.max():+.3f}",
        f"Biggest loss     {net.min():+.3f}",
        f"Window           {first_ts} → {last_ts}",
    ]

    warning = reconciliation_note(reports)
    if warning:
        lines = ["!! " + warning, ""] + lines

    return render_text_block("\n".join(lines), title="1. Headline")
