"""Independent P&L arithmetic.

This module exists because the scheduler got its own P&L wrong for weeks and
nothing here could tell. `earnings` was overstated by exactly one entry fee —
`sell_tokens` charged the taker fee on the buy leg twice, once implicitly (OKX
takes the spot buy fee in the base currency, so the token balance is already
`size * (1 - f)`) and once explicitly (`total_cost = cost + entry_fee`).

The analysis tool read `earnings` and believed it. Every report was off by
~0.1% of notional, which on a strategy whose entire edge lives inside 0.5% is
not a rounding error — it's a meaningful fraction of the thing being measured.

So: never trust a single stored column for the number the whole report turns on.
We recompute `earnings` from `buy_price`, `sell_price` and `fees` — which are
independent of the buggy line — and flag any row where the two disagree.

## The arithmetic

With taker fee `f` (as a fraction, so 0.001 for OKX's 0.1%), buying `size` at
`buy` and selling at `sell`:

    cost      = size * buy                       USDT paid
    tokens    = size * (1 - f)                   fee taken in base on the buy
    proceeds  = tokens * sell * (1 - f)          fee taken in quote on the sell
    earnings  = proceeds - cost
              = cost * ((1-f)^2 * r - 1)         where r = sell / buy

    fees      = cost * f  +  tokens * sell * f
              = cost * f * (1 + (1-f) * r)

`size` is not stored in `reports`, but it cancels: dividing the two gives

    earnings = fees * ((1-f)^2 * r - 1) / (f * (1 + (1-f) * r))

which needs only columns we have. `f` comes from `[exchange] taker_fee` in the
scheduler's own config, so the tool and the bot can't drift apart on it.

## Break-even

Every trade is a taker on both legs (`ioc`, or `market` on the sell fallback),
so a flat round trip costs two taker fees. The gross price move needed just to
get back to zero:

    breakeven = (1 / (1-f)^2 - 1) * 100   ~= 2 * f * 100   ~= 0.20% at f = 0.001

This is the single most important number in the report and it did not appear
anywhere in it. A position whose *peak* never reached +0.20% could not have been
sold at a profit at any moment of its life, no matter what the exit rules said.
"""

from __future__ import annotations

import tomllib
from pathlib import Path

import pandas as pd

# Rows whose stored earnings differ from the recomputed value by more than this
# (in dollars) are treated as a reconciliation failure. Generous enough to
# absorb float noise and the price-vs-fill gap, tight enough to catch a whole
# fee being double-charged.
RECONCILE_TOLERANCE = 0.005


def load_taker_fee(path: Path, default: float = 0.1) -> float:
    """Read `[exchange] taker_fee` (a *percent*: 0.1 means 0.10%) from config.

    Defaults rather than raising: `[exchange]` is optional in the scheduler's
    config, and a missing section shouldn't take the whole report down. The
    default matches `exchange_observer::Exchange::default()`.
    """
    with path.open("rb") as fh:
        data = tomllib.load(fh)
    exchange = data.get("exchange") or {}
    return float(exchange.get("taker_fee", default))


def breakeven_pct(taker_fee_pct: float) -> float:
    """Gross price move (percent) a round trip must make to break even."""
    f = taker_fee_pct / 100.0
    return (1.0 / (1.0 - f) ** 2 - 1.0) * 100.0


def attach(reports: pd.DataFrame, taker_fee_pct: float) -> pd.DataFrame:
    """Add recomputed P&L columns and a reconciliation flag.

    New columns:
      gross_pct        price move implied by buy/sell price (a check on `change`)
      earnings_recomputed  independent of the stored `earnings`
      earnings_delta   stored minus recomputed; ~0 when the scheduler is right
      winnable         did the position's PEAK ever clear break-even?
    """
    if reports.empty:
        return reports

    f = taker_fee_pct / 100.0
    out = reports.copy()

    # Guard against buy_price = 0 (an entry that never filled shouldn't have a
    # report at all, but a NaN here is better than an inf).
    ratio = out["sell_price"] / out["buy_price"].where(out["buy_price"] > 0.0)

    out["gross_pct"] = (ratio - 1.0) * 100.0

    numerator = (1.0 - f) ** 2 * ratio - 1.0
    denominator = f * (1.0 + (1.0 - f) * ratio)
    out["earnings_recomputed"] = out["fees"] * numerator / denominator
    out["earnings_delta"] = out["earnings"] - out["earnings_recomputed"]

    # `highest` is the peak change reached during the hold — the best price the
    # position ever offered. If that never cleared break-even, no exit rule
    # could have made money on this trade. It was dead on entry.
    out["winnable"] = out["highest"] >= breakeven_pct(taker_fee_pct)

    return out


def reconciliation_note(reports: pd.DataFrame) -> str | None:
    """Human-readable warning when stored and recomputed earnings disagree.

    Returns None when everything reconciles.
    """
    if reports.empty or "earnings_delta" not in reports:
        return None

    bad = reports[reports["earnings_delta"].abs() > RECONCILE_TOLERANCE]
    if bad.empty:
        return None

    mean_delta = bad["earnings_delta"].mean()
    return (
        f"RECONCILIATION FAILED: {len(bad)} of {len(reports)} rows disagree with "
        f"recomputed P&L by more than ${RECONCILE_TOLERANCE:.3f} "
        f"(mean {mean_delta:+.4f}/trade). The scheduler's stored `earnings` is "
        f"not trustworthy for these rows. Totals below use the RECOMPUTED value."
    )
