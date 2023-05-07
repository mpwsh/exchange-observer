use crate::BASE_URL;
use anyhow::Result;
use serde::{Deserialize, Serialize};

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
