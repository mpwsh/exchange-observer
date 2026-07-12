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
/// OKX's public `/api/v5/public/instruments` endpoint. Fields we care
/// about for order construction; the endpoint returns more that we
/// don't need (base/quote ccy, contract types, timestamps, etc.).
///
/// All numeric fields arrive as strings ("0.001", "1", "0.00000001")
/// so we keep them as `String` here and parse at consumption time —
/// avoids losing precision on tokens with tiny lot sizes.
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
}

impl InstrumentMeta {
    /// Fallback used when we don't have metadata for an instrument (cache
    /// miss). Steps of zero mean `floor_to_step` passes values through
    /// unchanged, so behavior matches the pre-precision code.
    pub const UNKNOWN: Self = Self { lot_sz: 0.0, tick_sz: 0.0, min_sz: 0.0 };
}

/// Fetch every spot instrument's precision metadata in one call.
///
/// The OKX endpoint is unauthenticated, returns ~600 instruments (~200KB
/// JSON) in one response, and rarely changes — so we do this once at
/// scheduler startup and cache in memory for the session. If OKX is
/// slow or unreachable, the caller falls back to an empty map and the
/// order path uses `InstrumentMeta::UNKNOWN` per instrument, which is
/// the pre-rounding behavior.
pub async fn fetch_spot_instruments() -> Result<Vec<OkxInstrument>> {
    let res = reqwest::Client::new()
        .get(format!("{BASE_URL}/api/v5/public/instruments?instType=SPOT"))
        .send()
        .await?
        .json::<OkxInstrumentResponse>()
        .await?;
    Ok(res.data)
}

/// Round `value` down to the nearest multiple of `step`. Floor, not
/// round-to-nearest, because we never want to imply a size/price that
/// exceeds what we asked for — a bid at `price + epsilon` might miss the
/// book, a size that exceeds spendable might overspend.
///
/// `step == 0.0` (the `InstrumentMeta::UNKNOWN` case) passes the value
/// through unchanged, preserving pre-rounding behavior.
pub fn floor_to_step(value: f64, step: f64) -> f64 {
    if step <= 0.0 || !step.is_finite() {
        return value;
    }
    (value / step).floor() * step
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_to_step_rounds_size_down_to_lot() {
        // Buying $50 of SLX at $0.15 with lotSz=0.01: 333.333... → 333.33
        assert_eq!(floor_to_step(333.333_333_333_333_37, 0.01), 333.33);
    }

    #[test]
    fn floor_to_step_never_exceeds_input() {
        // Property: rounded value must be <= input, or the size the
        // exchange sees would imply more spend than we authorized.
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
        assert_eq!(floor_to_step(333.333_333_333_333_37, 0.0), 333.333_333_333_333_37);
    }

    #[test]
    fn floor_to_step_negative_step_passes_through() {
        // Malformed metadata should not panic or corrupt the value.
        assert_eq!(floor_to_step(100.0, -0.01), 100.0);
    }
}
