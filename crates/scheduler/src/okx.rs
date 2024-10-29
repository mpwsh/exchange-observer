use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::BASE_URL;

pub type OkxAccountBalanceResponse = OkxApiResponse<OkxAccountBalance>;
pub type OkxOrderResponse = OkxApiResponse<OkxOrder>;
pub type OkxTimeResponse = OkxApiResponse<OkxTime>;
pub type OkxOrderDetailsResponse = OkxApiResponse<OkxOrderDetails>;

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
