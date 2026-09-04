"""Rendering helpers.

Every section produces a PNG-as-base64 string. The HTML template embeds
those as `<img src="data:image/png;base64,...">`, so the report is one
self-contained file with no external assets.

Two shapes we render:
- Tables (pandas DataFrame → styled matplotlib figure → PNG).
- Candle charts (mplfinance OHLC + entry/exit markers + threshold lines).

Both go through `_figure_to_data_uri` so callers only ever return strings.
"""

from __future__ import annotations

import base64
import io

import matplotlib
matplotlib.use("Agg")  # noqa: E402 — no display in this environment.

import matplotlib.pyplot as plt  # noqa: E402
import mplfinance as mpf  # noqa: E402
import pandas as pd  # noqa: E402

# Terminal-ish palette matching the console. Kept as module constants so
# section code doesn't need to know rgb values.
UP = "#78dc82"
DOWN = "#e66464"
DIM = "#8c919b"
INFO = "#78c8e6"
WARN = "#e6c864"
FG = "#d2d6dc"
BG = "#14161a"


def _figure_to_data_uri(fig: plt.Figure) -> str:
    """Serialize a matplotlib figure into a data URI. Closes the figure."""
    buf = io.BytesIO()
    fig.savefig(buf, format="png", dpi=110, bbox_inches="tight", facecolor=fig.get_facecolor())
    plt.close(fig)
    encoded = base64.b64encode(buf.getvalue()).decode("ascii")
    return f"data:image/png;base64,{encoded}"


def render_table(
    df: pd.DataFrame, *, title: str, note: str | None = None, width: float = 11.0
) -> str:
    """Render a DataFrame as a table figure. Numeric columns right-aligned.

    Row height scales with row count so 3-row tables aren't stretched.
    A muted note printed below the table gives per-section context that
    the user shouldn't have to re-derive from column headers.

    Empty DataFrames render as a single "(no data)" cell — some sections
    legitimately produce nothing (e.g. no losing trades → no loss_shape
    rows), and we want the section header to still appear.
    """
    if df.empty:
        fig, ax = plt.subplots(figsize=(width, 1.4), facecolor=BG)
        ax.set_facecolor(BG)
        ax.axis("off")
        ax.set_title(title, color=FG, fontsize=12, weight="bold", loc="left", pad=10)
        ax.text(
            0.02, 0.4, note or "(no data)", color=DIM, fontsize=11, family="monospace",
            va="center", ha="left", transform=ax.transAxes,
        )
        return _figure_to_data_uri(fig)

    rows = len(df) + 1  # +1 header
    row_h = 0.35
    height = 0.5 + rows * row_h + (0.4 if note else 0.1)
    fig, ax = plt.subplots(figsize=(width, height), facecolor=BG)
    ax.set_facecolor(BG)
    ax.axis("off")

    # Format all cells as strings so matplotlib doesn't reformat floats.
    display_df = df.copy()
    for col in display_df.columns:
        if pd.api.types.is_float_dtype(display_df[col]):
            display_df[col] = display_df[col].map(lambda v: f"{v:.3f}" if pd.notna(v) else "")
        else:
            display_df[col] = display_df[col].astype(str)

    table = ax.table(
        cellText=display_df.values.tolist(),
        colLabels=list(display_df.columns),
        cellLoc="right",
        colLoc="center",
        loc="upper center",
    )
    table.auto_set_font_size(False)
    table.set_fontsize(10)
    table.scale(1.0, 1.4)

    # Style: header row white text on dim background, body rows FG on BG.
    for (row, _col), cell in table.get_celld().items():
        cell.set_edgecolor(DIM)
        cell.set_linewidth(0.5)
        if row == 0:
            cell.set_facecolor("#252830")
            cell.set_text_props(color=FG, weight="bold")
        else:
            cell.set_facecolor(BG)
            cell.set_text_props(color=FG)

    ax.set_title(title, color=FG, fontsize=12, weight="bold", loc="left", pad=10)
    if note:
        # Anchor the note beneath the table's rendered position, not
        # beneath the axes (which is much larger). Use figure coords.
        fig.text(0.05, 0.02, note, color=DIM, fontsize=9, style="italic")

    return _figure_to_data_uri(fig)


def render_text_block(text: str, *, title: str, width: float = 11.0) -> str:
    """Render a monospace text block — used by the headline card."""
    lines = text.count("\n") + 1
    fig, ax = plt.subplots(figsize=(width, 0.4 + lines * 0.25), facecolor=BG)
    ax.set_facecolor(BG)
    ax.axis("off")
    ax.set_title(title, color=FG, fontsize=12, weight="bold", loc="left", pad=10)
    ax.text(
        0.02,
        0.5,
        text,
        color=FG,
        fontsize=11,
        family="monospace",
        va="center",
        ha="left",
        transform=ax.transAxes,
    )
    return _figure_to_data_uri(fig)


def render_candles(
    candles: pd.DataFrame,
    *,
    title: str,
    entry_ts: pd.Timestamp,
    exit_ts: pd.Timestamp | None,
    buy_price: float,
    sell_price: float | None,
    sell_floor_pct: float | None,
    cashout_pct: float | None,
    stoploss_pct: float | None,
) -> str:
    """Render an OHLC candle chart with entry/exit markers.

    Threshold lines (sell_floor, cashout, stoploss) draw horizontal
    dashed lines relative to buy_price so it's obvious whether the exit
    fired at a threshold or off-piste.

    Returns an empty string if `candles` is empty — callers handle by
    skipping the trade entirely (or noting "no candles in window").
    """
    if candles.empty:
        # Placeholder: draw a small figure that says so, still returns a URI.
        fig, ax = plt.subplots(figsize=(11, 3), facecolor=BG)
        ax.set_facecolor(BG)
        ax.text(
            0.5,
            0.5,
            f"{title}\n[no candles in window]",
            color=DIM,
            fontsize=12,
            ha="center",
            va="center",
            transform=ax.transAxes,
        )
        ax.axis("off")
        return _figure_to_data_uri(fig)

    # mplfinance styling to match the terminal palette.
    mc = mpf.make_marketcolors(up=UP, down=DOWN, edge="inherit", wick="inherit")
    style = mpf.make_mpf_style(
        marketcolors=mc,
        facecolor=BG,
        edgecolor=DIM,
        gridcolor="#2a2d34",
        gridstyle=":",
        rc={"font.family": "monospace", "axes.labelcolor": FG, "text.color": FG,
            "xtick.color": FG, "ytick.color": FG},
    )

    # Threshold lines: compute prices, filter Nones.
    hlines_prices: list[float] = [buy_price]
    hlines_colors: list[str] = [WARN]
    if sell_floor_pct is not None:
        hlines_prices.append(buy_price * (1 + sell_floor_pct / 100.0))
        hlines_colors.append(UP)
    if cashout_pct is not None:
        hlines_prices.append(buy_price * (1 + cashout_pct / 100.0))
        hlines_colors.append(INFO)
    if stoploss_pct is not None:
        hlines_prices.append(buy_price * (1 - stoploss_pct / 100.0))
        hlines_colors.append(DOWN)

    # mplfinance markers via addplot: one series per marker so colors
    # don't blend. Use NaN-filled series for non-marker candles.
    def marker_series(ts: pd.Timestamp | None, price: float | None) -> pd.Series:
        s = pd.Series(index=candles.index, dtype=float)
        if ts is None or price is None:
            return s
        # Snap to the nearest candle timestamp — cqlsh ts and candle ts
        # may differ by seconds even when they represent the same minute.
        snapped = candles.index[candles.index.get_indexer([ts], method="nearest")[0]]
        s.loc[snapped] = price
        return s

    entry_marker = marker_series(entry_ts, buy_price)
    exit_marker = marker_series(exit_ts, sell_price)

    addplots = []
    if entry_marker.notna().any():
        addplots.append(
            mpf.make_addplot(entry_marker, type="scatter", markersize=100, marker="^", color=WARN)
        )
    if exit_marker.notna().any():
        addplots.append(
            mpf.make_addplot(exit_marker, type="scatter", markersize=100, marker="v", color=INFO)
        )

    fig, axes = mpf.plot(
        candles,
        type="candle",
        style=style,
        hlines={"hlines": hlines_prices, "colors": hlines_colors, "linestyle": "--", "linewidths": 0.8},
        addplot=addplots if addplots else None,
        returnfig=True,
        figsize=(11, 4),
        title=title,
        ylabel="",
        volume=False,
        tight_layout=True,
    )
    fig.patch.set_facecolor(BG)
    return _figure_to_data_uri(fig)
