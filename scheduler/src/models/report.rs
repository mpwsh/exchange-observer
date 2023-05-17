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
impl Report {
    pub fn new(round_id: u64, strategy_hash: &str, t: &Token) -> Self {
        Self {
            round_id,
            reason: "None".to_string(),
            instid: t.instid.clone(),
            ts: Utc::now().timestamp_millis().to_string(),
            buy_price: t.price,
            strategy: strategy_hash.to_string(),
            change: t.change,
            sell_price: t.price,
            earnings: 0.0,
            time_left: t.timeout.num_seconds(),
            .. Default::default()
        }
    }
    pub async fn save(&self, db_session: &Session) -> Result<QueryResult> {
        let payload = serde_json::to_string_pretty(&self).unwrap();
        let payload = payload.replace("null", "0");
        let query = format!("INSERT INTO okx.reports JSON '{}'", payload);
        Ok(db_session.query(&*query, &[]).await?)
    }
}
impl ToString for Report {
    fn to_string(&self) -> String {
        //let timestamp = DateTime::<Utc>::from_utc(NaiveDateTime::from_timestamp_opt(0, 0)?, Utc) + self.ts;
        //timestamp.format("%Y-%m-%d %H:%M:%S").to_string(),
        format!(
                    "[{}] - Round({}) - {} Report: Time left: {} - Change: [Highest: %{}, Lowest: %{}, Exit: %{}] - Earnings: {:.2} - ExitReason: {}",
                    self.ts,
                    self.round_id,
                    self.instid,
                    self.time_left,
                    self.highest,
                    self.lowest,
                    self.change,
                    self.earnings,
                    self.reason
                )
    }
}
