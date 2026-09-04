"""Scylla loaders.

Everything downstream works from pandas DataFrames — this module is the
only place that touches the driver. Two entry points:

- `load_reports(...)`: pulls the whole `okx.reports` table into a DataFrame.
- `load_candles(...)`: per-trade candle windows for the replay section.

`open_session()` reads Scylla host/port/keyspace from the same TOML the
scheduler uses. We only touch `[database]` — schema drift on other fields
doesn't affect us. If the TOML file grows a section we don't recognize we
silently ignore it.
"""

from __future__ import annotations

import tomllib
from dataclasses import dataclass
from pathlib import Path

import pandas as pd
from cassandra.cluster import Cluster, Session
from cassandra.query import SimpleStatement


@dataclass
class Db:
    """Scylla connection configuration parsed out of config.toml."""

    host: str
    port: int
    keyspace: str


def load_db_config(path: Path) -> Db:
    """Parse a scheduler-style config.toml and return the DB coordinates."""
    with path.open("rb") as fh:
        data = tomllib.load(fh)
    db = data["database"]
    return Db(host=db["ip"], port=int(db["port"]), keyspace=db["keyspace"])


def open_session(db: Db) -> Session:
    """Open a Scylla session on the given keyspace.

    The driver's default consistency (LOCAL_ONE) is fine here — we're
    running post-hoc, not competing with the scheduler for the same rows.
    """
    cluster = Cluster([db.host], port=db.port)
    session = cluster.connect(db.keyspace)
    # Keep responses reasonably sized on the way back — 20k rows fits.
    session.default_fetch_size = 20_000
    return session


# Report column order for the DataFrame. Explicit so the SELECT and the
# frame columns stay in lockstep; also survives future ALTER TABLEs.
_REPORT_COLUMNS = [
    "round_id",
    "instid",
    "reason",
    "earnings",
    "change",
    "time_left",
    "highest",
    "lowest",
    "ts",
    "strategy",
    "fees",
    "buy_price",
    "sell_price",
    # Entry conditions. Written from the instrumented build onward; null on
    # anything older, which `load_reports` leaves as NaN so downstream sections
    # can drop those rows rather than silently bucket them as zero.
    "dip",
    "bounce",
    "std_deviation",
    "spread_bps",
]


def load_reports(session: Session, keyspace: str) -> pd.DataFrame:
    """Load every report row into a DataFrame.

    Columns arrive as native Python types via the driver; we convert
    into a DataFrame with sensible dtypes. `fees` may be null on rows
    written before that column existed — we fill with 0 so aggregates
    stay honest and note the count in the returned frame's attrs.
    """
    cql = f"SELECT {', '.join(_REPORT_COLUMNS)} FROM {keyspace}.reports"
    stmt = SimpleStatement(cql, fetch_size=20_000)
    rows = session.execute(stmt)

    frame = pd.DataFrame(rows, columns=_REPORT_COLUMNS)
    if frame.empty:
        return frame

    # Fill fees nulls with 0 and record the count so callers can flag it.
    fees_null = int(frame["fees"].isna().sum())
    frame["fees"] = frame["fees"].fillna(0.0)
    frame.attrs["fees_null_count"] = fees_null

    # Ensure numeric types are numeric — driver mostly gets this right
    # but explicit is safer against future schema wobbles.
    for col in (
        "earnings",
        "change",
        "highest",
        "lowest",
        "fees",
        "buy_price",
        "sell_price",
        "dip",
        "bounce",
        "std_deviation",
        "spread_bps",
    ):
        frame[col] = pd.to_numeric(frame[col], errors="coerce")

    # Sort newest first so `.head(N)` in downstream sections is "recent".
    frame = frame.sort_values("ts", ascending=False).reset_index(drop=True)
    return frame


_CANDLE_COLUMNS = ["ts", "open", "close", "high", "low", "change", "volume"]


def load_candles(
    session: Session,
    keyspace: str,
    instid: str,
    entry_ts: pd.Timestamp,
    minutes_before: int = 20,
    minutes_after: int = 0,
) -> pd.DataFrame:
    """Fetch OHLC candles around a trade entry.

    Returned frame is oldest-first, indexed by ts, and ready to hand to
    mplfinance (which expects Open/High/Low/Close/Volume columns with a
    DateTimeIndex).
    """
    lower = entry_ts - pd.Timedelta(minutes=minutes_before)
    upper = entry_ts + pd.Timedelta(minutes=minutes_after)

    # `candle1m`'s partition key is `instid`, so this query hits one
    # partition — no ALLOW FILTERING needed.
    cql = (
        f"SELECT {', '.join(_CANDLE_COLUMNS)} FROM {keyspace}.candle1m "
        f"WHERE instid = %s AND ts >= %s AND ts <= %s"
    )
    rows = session.execute(cql, (instid, lower.to_pydatetime(), upper.to_pydatetime()))

    frame = pd.DataFrame(rows, columns=_CANDLE_COLUMNS)
    if frame.empty:
        return frame
    frame = frame.sort_values("ts").set_index("ts")
    # mplfinance expects capitalized column names.
    frame = frame.rename(
        columns={
            "open": "Open",
            "high": "High",
            "low": "Low",
            "close": "Close",
            "volume": "Volume",
        }
    )
    return frame
