"""Section 9: trade replay.

For the top N winning trades and bottom N losing trades, render an OHLC
candle chart showing the ~20 minutes leading up to entry. Entry candle
marked with an up-arrow, exit candle marked with a down-arrow. Dashed
horizontal lines show sell_floor / cashout / stoploss thresholds
relative to buy_price so it's obvious whether the exit fired at a
threshold or off-piste.

Lets us eyeball what a winning setup vs a losing setup looked like —
was it a genuine dip-and-bounce, or did we enter at a top?
"""

from __future__ import annotations

from dataclasses import dataclass

import pandas as pd
from cassandra.cluster import Session

from db import load_candles
from render import render_candles


@dataclass
class Thresholds:
    sell_floor_pct: float | None
    cashout_pct: float | None
    stoploss_pct: float | None
    timeout_s: int | None       # NEW


def load_thresholds(session: Session, keyspace: str, strategy_hash: str) -> Thresholds:
    """Look up the strategy config the trade ran under.

    Trades log their strategy hash; the strategies table has the numeric
    thresholds. If the row isn't there (rare — usually means the table
    was truncated between the run and analysis), fall back to None so
    the chart still renders without threshold lines.
    """
    cql = (
    f"SELECT sell_floor, cashout, stoploss, timeout FROM {keyspace}.strategies "
    "WHERE hash = %s LIMIT 1"
    )
    row = session.execute(cql, (strategy_hash,)).one()
    if row is None:
        return Thresholds(None, None, None, None)
    return Thresholds(
        sell_floor_pct=float(row.sell_floor) if row.sell_floor is not None else None,
        cashout_pct=float(row.cashout) if row.cashout is not None else None,
        stoploss_pct=float(row.stoploss) if row.stoploss is not None else None,
        timeout_s=int(row.timeout) if row.timeout is not None else None,
    )


def _title(row: pd.Series, kind: str, rank: int) -> str:
    sign = "+" if row["earnings"] >= 0 else ""
    return (
        f"{kind} #{rank}  {row['instid']}  "
        f"({row['reason']}, earnings {sign}${row['earnings']:.3f})"
    )


def _render_one(row, session, keyspace, kind, rank, thresholds_cache):
    # `report.ts` is the exit timestamp (report saved when sell fired).
    exit_ts = pd.Timestamp(row["ts"])

    strategy_hash = row["strategy"]
    thresholds = thresholds_cache.get(strategy_hash)
    if thresholds is None:
        thresholds = load_thresholds(session, keyspace, strategy_hash)
        thresholds_cache[strategy_hash] = thresholds

    # Recover entry time by subtracting elapsed hold from exit_ts.
    entry_ts = exit_ts
    time_left = row["time_left"] if pd.notna(row["time_left"]) else None
    if thresholds.timeout_s is not None and time_left is not None:
        elapsed = thresholds.timeout_s - int(time_left)
        if elapsed >= 0:
            entry_ts = exit_ts - pd.Timedelta(seconds=elapsed)

    # Load candles anchored on entry_ts now, showing 20 min before + 10 after.
    candles = load_candles(
        session, keyspace, row["instid"], entry_ts,
        minutes_before=20, minutes_after=10,
    )

    sell_price = row["sell_price"] if pd.notna(row["sell_price"]) else None

    return render_candles(
        candles,
        title=_title(row, kind, rank),
        entry_ts=entry_ts,
        exit_ts=exit_ts,
        buy_price=float(row["buy_price"]),
        sell_price=float(sell_price) if sell_price is not None else None,
        sell_floor_pct=thresholds.sell_floor_pct,
        cashout_pct=thresholds.cashout_pct,
        stoploss_pct=thresholds.stoploss_pct,
    )


def render(reports: pd.DataFrame, session: Session, keyspace: str, top_n: int = 3) -> list[str]:
    """Return one image URI per replay trade, wins first then losses."""
    if reports.empty:
        return []
    thresholds_cache: dict[str, Thresholds] = {}
    out: list[str] = []

    wins = reports.nlargest(top_n, "earnings")
    losses = reports.nsmallest(top_n, "earnings")

    for rank, (_, row) in enumerate(wins.iterrows(), start=1):
        out.append(_render_one(row, session, keyspace, "WIN", rank, thresholds_cache))
    for rank, (_, row) in enumerate(losses.iterrows(), start=1):
        out.append(_render_one(row, session, keyspace, "LOSS", rank, thresholds_cache))
    return out
