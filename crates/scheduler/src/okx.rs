use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::BASE_URL;

pub type OkxAccountBalanceResponse = OkxApiResponse<OkxAccountBalance>;
pub type OkxOrderResponse = OkxApiResponse<OkxOrder>;
pub type OkxTimeResponse = OkxApiResponse<OkxTime>;
pub type OkxOrderDetailsResponse = OkxApiResponse<OkxOrderDetails>;
pub type OkxInstrumentResponse = OkxApiResponse<OkxInstrument>;

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OkxApiResponse<T> {
    pub code: String,
    pub data: Vec<T>,
    pub msg: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OkxOrderDetails {
    pub inst_type: String,
    pub inst_id: String,
    pub ccy: String,
    pub ord_id: String,
    pub cl_ord_id: String,
    pub tag: String,
    pub px: String,
    pub sz: String,
    pub pnl: String,
    pub ord_type: String,
    pub side: String,
    pub pos_side: String,
    pub td_mode: String,
    pub acc_fill_sz: String,
    pub fill_px: String,
    pub trade_id: String,
    pub fill_sz: String,
    pub fill_time: String,
    pub state: String,
    pub avg_px: String,
    pub lever: String,
    pub tp_trigger_px: String,
    pub tp_trigger_px_type: String,
    pub tp_ord_px: String,
    pub sl_trigger_px: String,
    pub sl_trigger_px_type: String,
    pub sl_ord_px: String,
    pub fee_ccy: String,
    pub fee: String,
    pub rebate_ccy: String,
    pub rebate: String,
    pub tgt_ccy: String,
    pub category: String,
    pub reduce_only: String,
    pub cancel_source: String,
    pub cancel_source_reason: String,
    pub quick_mgn_type: String,
    pub algo_cl_ord_id: String,
    pub algo_id: String,
    pub u_time: String,
    pub c_time: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OkxAccountBalance {
    pub adj_eq: String,
    pub details: Vec<OkxAccountBalanceDetail>,
    pub imr: String,
    pub iso_eq: String,
    pub mgn_ratio: String,
    pub mmr: String,
    pub notional_usd: String,
    pub ord_froz: String,
    pub total_eq: String,
    pub u_time: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OkxAccountBalanceDetail {
    pub avail_bal: String,
    pub avail_eq: String,
    pub cash_bal: String,
    pub ccy: String,
    pub cross_liab: String,
    pub dis_eq: String,
    pub eq: String,
    pub eq_usd: String,
    pub frozen_bal: String,
    pub interest: String,
    pub iso_eq: String,
    pub iso_liab: String,
    pub iso_upl: String,
    pub liab: String,
    pub max_loan: String,
    pub mgn_ratio: String,
    pub notional_lever: String,
    pub ord_frozen: String,
    pub twap: String,
    pub u_time: String,
    pub upl: String,
    pub upl_liab: String,
    pub stgy_eq: String,
    pub spot_in_use_amt: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OkxOrder {
    pub cl_ord_id: String,
    pub ord_id: String,
    pub s_code: String,
    pub s_msg: String,
    pub tag: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OkxTime {
    pub ts: String,
}

pub async fn get_time() -> Result<i64> {
    let res = reqwest::Client::new()
        .get(format!("{BASE_URL}/api/v5/public/time"))
        .send()
        .await?
        .json::<OkxTimeResponse>()
        .await?;
    Ok(res.data[0].ts.parse::<i64>()?)
}

/// One instrument's precision / minimum-size metadata as returned by
/// OKX's public `/api/v5/public/instruments` endpoint.
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OkxInstrument {
    pub inst_id: String,
    pub inst_type: String,
    /// Price increment; all order prices must be a multiple of this.
    pub tick_sz: String,
    /// Size increment; all order sizes must be a multiple of this.
    pub lot_sz: String,
    /// Minimum order size the exchange will accept.
    pub min_sz: String,
    /// "live" / "suspend" / "expired" / "preopen". We only trade "live".
    pub state: String,
}

/// Parsed, in-memory version of `OkxInstrument`. Constructed once at
/// startup from the REST response, kept in `App` for the session.
#[derive(Debug, Clone, Copy)]
pub struct InstrumentMeta {
    pub lot_sz: f64,
    pub tick_sz: f64,
    pub min_sz: f64,
    /// Decimal places implied by the raw `lotSz` string ("0.001" → 3).
    ///
    /// Kept because `f64::to_string()` on a floored value happily prints
    /// `333.33000000000004`, and OKX rejects that. `None` means we have no
    /// metadata for the instrument and fall back to `to_string()` — the
    /// pre-existing behavior.
    pub lot_dp: Option<usize>,
    /// Decimal places implied by the raw `tickSz` string.
    pub tick_dp: Option<usize>,
}

impl InstrumentMeta {
    /// Fallback used when we don't have metadata for an instrument (cache
    /// miss). Steps of zero mean the rounding helpers pass values through
    /// unchanged, so behavior matches the pre-precision code.
    pub const UNKNOWN: Self = Self {
        lot_sz: 0.0,
        tick_sz: 0.0,
        min_sz: 0.0,
        lot_dp: None,
        tick_dp: None,
    };

    /// Builds metadata from the raw strings OKX sent, keeping the decimal
    /// precision that `parse::<f64>()` throws away.
    pub fn from_raw(raw: &OkxInstrument) -> Option<Self> {
        let (Ok(lot_sz), Ok(tick_sz), Ok(min_sz)) = (
            raw.lot_sz.parse::<f64>(),
            raw.tick_sz.parse::<f64>(),
            raw.min_sz.parse::<f64>(),
        ) else {
            return None;
        };
        Some(Self {
            lot_sz,
            tick_sz,
            min_sz,
            lot_dp: Some(decimals_of(&raw.lot_sz)),
            tick_dp: Some(decimals_of(&raw.tick_sz)),
        })
    }

    /// Base-currency size for an order worth `size_quote` at `price`,
    /// floored to `lot_sz`.
    ///
    /// Returns `None` when the result would be below the instrument's
    /// `min_sz`. We fetch `min_sz` from OKX and, until now, never used it —
    /// so an order too small for the instrument was rejected by the exchange
    /// and came back looking exactly like a missed fill, which is how it hid.
    #[must_use]
    pub fn order_size(&self, size_quote: f64, price: f64) -> Option<f64> {
        if !price.is_finite() || price <= 0.0 || !size_quote.is_finite() {
            return None;
        }
        let size = floor_to_step(size_quote / price, self.lot_sz);
        if size <= 0.0 || size < self.min_sz {
            return None;
        }
        Some(size)
    }

    /// Renders a size for the wire at the instrument's lot precision.
    #[must_use]
    pub fn format_size(&self, value: f64) -> String {
        format_at(value, self.lot_dp)
    }

    /// Renders a price for the wire at the instrument's tick precision.
    #[must_use]
    pub fn format_price(&self, value: f64) -> String {
        format_at(value, self.tick_dp)
    }
}

fn format_at(value: f64, dp: Option<usize>) -> String {
    match dp {
        Some(dp) => format!("{value:.dp$}"),
        None => value.to_string(),
    }
}

/// Decimal places in an OKX step string: `"0.001"` → 3, `"1"` → 0,
/// `"0.00000001"` → 8.
#[must_use]
pub fn decimals_of(step: &str) -> usize {
    step.split_once('.')
        .map_or(0, |(_, frac)| frac.trim_end_matches('0').len())
}

/// Fetch every spot instrument's precision metadata in one call.
pub async fn fetch_spot_instruments() -> Result<Vec<OkxInstrument>> {
    let res = reqwest::Client::new()
        .get(format!("{BASE_URL}/api/v5/public/instruments?instType=SPOT"))
        .send()
        .await?
        .json::<OkxInstrumentResponse>()
        .await?;
    Ok(res.data)
}

/// Round `value` **down** to the nearest multiple of `step`.
///
/// Correct for *sizes* (never imply more than we hold) and for *sell* limit
/// prices (a sell below the bid crosses). Wrong for buy limit prices — see
/// [`ceil_to_step`].
///
/// `step == 0.0` (the `InstrumentMeta::UNKNOWN` case) passes the value
/// through unchanged.
#[must_use]
pub fn floor_to_step(value: f64, step: f64) -> f64 {
    if step <= 0.0 || !step.is_finite() {
        return value;
    }
    (value / step).floor() * step
}

/// Round `value` **up** to the nearest multiple of `step`.
///
/// This is the correct rounding for a *buy* limit price. Flooring a bid
/// pushes it away from the ask, so an IOC buy can only fill when a seller
/// comes down onto it — every fill is then, by construction, a fill into
/// weakness. Ceiling keeps the bid on the aggressive side of the tick.
#[must_use]
pub fn ceil_to_step(value: f64, step: f64) -> f64 {
    if step <= 0.0 || !step.is_finite() {
        return value;
    }
    (value / step).ceil() * step
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(lot: &str, tick: &str, min: &str) -> InstrumentMeta {
        InstrumentMeta::from_raw(&OkxInstrument {
            inst_id: "TEST-USDT".into(),
            inst_type: "SPOT".into(),
            tick_sz: tick.into(),
            lot_sz: lot.into(),
            min_sz: min.into(),
            state: "live".into(),
        })
        .expect("parses")
    }

    #[test]
    fn floor_to_step_rounds_size_down_to_lot() {
        assert_eq!(floor_to_step(333.333_333_333_333_37, 0.01), 333.33);
    }

    #[test]
    fn floor_to_step_never_exceeds_input() {
        let inputs = [50.0, 0.0005, 6_250_000.0, 333.333];
        let steps = [0.01, 0.000_000_01, 1.0, 0.001];
        for &v in &inputs {
            for &s in &steps {
                assert!(floor_to_step(v, s) <= v);
            }
        }
    }

    #[test]
    fn floor_to_step_zero_step_passes_through() {
        assert_eq!(
            floor_to_step(333.333_333_333_333_37, 0.0),
            333.333_333_333_333_37
        );
    }

    #[test]
    fn ceil_to_step_never_undercuts_input() {
        assert!(ceil_to_step(1.0001, 0.001) >= 1.0001);
    }

    #[test]
    fn ceil_to_step_leaves_exact_multiples_alone() {
        assert!((ceil_to_step(1.230, 0.01) - 1.23).abs() < 1e-12);
    }

    #[test]
    fn decimals_of_reads_precision_from_the_raw_string() {
        assert_eq!(decimals_of("1"), 0);
        assert_eq!(decimals_of("0.001"), 3);
        assert_eq!(decimals_of("0.00000001"), 8);
    }

    #[test]
    fn format_size_does_not_leak_float_noise_onto_the_wire() {
        let m = meta("0.01", "0.0001", "1");
        // 333.33000000000004 is what `to_string()` would have sent.
        assert_eq!(m.format_size(floor_to_step(333.333_333_3, 0.01)), "333.33");
    }

    #[test]
    fn order_size_rejects_orders_below_min_sz() {
        let m = meta("0.01", "0.0001", "100");
        // $50 at $1.00 → 50 units, below a min_sz of 100.
        assert_eq!(m.order_size(50.0, 1.0), None);
    }

    #[test]
    fn order_size_accepts_orders_at_or_above_min_sz() {
        let m = meta("0.01", "0.0001", "100");
        assert_eq!(m.order_size(200.0, 1.0), Some(200.0));
    }
}
