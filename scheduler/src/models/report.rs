use crate::prelude::*;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Report {
    pub round_id: u64,
    pub instid: String,
    pub buy_price: f64,
    pub sell_price: f64,
    pub earnings: f64,
    pub reason: String,
    pub highest: f32,
    pub highest_elapsed: i64,
    pub lowest: f32,
    pub lowest_elapsed: i64,
    pub change: f32,
    pub time_left: i64,
    pub strategy: String,
    pub ts: String,
}

impl Default for Report {
    fn default() -> Self {
        Self {
            round_id: 0,
            instid: String::new(),
            buy_price: 0.0,
            sell_price: 0.0,
            earnings: 0.00,
            reason: String::new(),
            lowest: 0.0,
            lowest_elapsed: 0,
            highest: 0.0,
            highest_elapsed: 0,
            change: 0.0,
            time_left: 0,
            strategy: String::new(),
            ts: Utc::now().timestamp().to_string(),
        }
    }
}
