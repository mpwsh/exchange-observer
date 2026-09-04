use crate::prelude::*;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Report {
    pub round_id: u64,
    pub instid: String,
    pub buy_price: f64,
    pub sell_price: f64,
    pub earnings: f64,
    /// Total round-trip fees paid to the exchange (entry + exit taker fee).
    /// Kept as a separate column so tuning analysis can see the fee overhead
    /// per reason instead of it being silently netted into earnings.
    #[serde(default)]
    pub fees: f64,
    pub reason: String,
    pub highest: f32,
    pub highest_elapsed: i64,
    pub lowest: f32,
    pub lowest_elapsed: i64,
    pub change: f32,
    pub time_left: i64,
    pub strategy: String,
    pub ts: String,

    // --- Entry conditions ----------------------------------------------------
    //
    // The reports used to record what happened and never why we were there. So
    // "do deeper dips produce better trades?" — the question that decides
    // `min_dip` — could not be answered from the data at all, and the threshold
    // got set by argument. These four columns close that.
    //
    // Note these are the values that *cleared* the thresholds, not the thresholds
    // themselves. Recording `min_dip` would tell you nothing: every entry cleared
    // it by definition.
    /// Cumulative dip that triggered the entry, in percent (negative for a real
    /// dip). `0.0` for strategies with no dip window.
    #[serde(default)]
    pub dip: f64,
    /// Cumulative bounce that confirmed the entry, in percent.
    #[serde(default)]
    pub bounce: f64,
    /// The token's candle-change standard deviation at entry — its volatility.
    ///
    /// Already computed every cycle and thrown away (`max_deviation = 10.0` means
    /// nothing filters on it). This is the number that would let `stoploss` scale
    /// with the instrument instead of being one constant applied to tokens whose
    /// noise floors differ by 5-10x.
    #[serde(default)]
    pub std_deviation: f64,
    /// Quoted spread at entry, in basis points. What it cost to get in.
    #[serde(default)]
    pub spread_bps: f64,
}

impl Default for Report {
    fn default() -> Self {
        Self {
            round_id: 0,
            instid: String::new(),
            buy_price: 0.0,
            sell_price: 0.0,
            earnings: 0.00,
            fees: 0.0,
            reason: String::new(),
            lowest: 0.0,
            lowest_elapsed: 0,
            highest: 0.0,
            highest_elapsed: 0,
            change: 0.0,
            time_left: 0,
            strategy: String::new(),
            dip: 0.0,
            bounce: 0.0,
            std_deviation: 0.0,
            spread_bps: 0.0,
            // Placeholder: every live report is rebuilt via `Report::new`
            // before it is read or saved, so no ambient time is needed here.
            ts: String::from("0"),
        }
    }
}
impl Report {
    pub fn new(round_id: u64, strategy_hash: &str, t: &Token, now: DateTime<Utc>) -> Self {
        Self {
            round_id,
            reason: "None".to_string(),
            instid: t.instid.clone(),
            ts: now.timestamp_millis().to_string(),
            buy_price: t.price,
            strategy: strategy_hash.to_string(),
            change: t.change,
            sell_price: t.price,
            earnings: 0.0,
            time_left: t.timeout.num_seconds(),
            ..Default::default()
        }
    }

    pub async fn save(&self, db_session: &Session) -> Result<QueryResult> {
        let payload = serde_json::to_string_pretty(&self).unwrap();
        let payload = payload.replace("null", "0");
        let query = format!("INSERT INTO okx.reports JSON '{}'", payload);
        Ok(db_session.query_unpaged(&*query, &[]).await?)
    }
}
impl Report {
    /// Renders the report as a log line. `now` is the fallback timestamp for
    /// reports whose `ts` does not parse (which, since `ts` is stored as unix
    /// millis but parsed with a datetime format, is currently every report —
    /// preserved as-is from the original `ToString` impl).
    pub fn log_line(&self, now: DateTime<Utc>) -> String {
        let timestamp = match DateTime::parse_from_str(&self.ts, "%Y-%m-%d %H:%M:%S") {
            Ok(t) => t.with_timezone(&Utc),
            Err(_) => now,
        };

        format!(
            "[{}] - Round({}) - {} Report: Time left: {} - Change: [Highest: %{}, Lowest: %{}, Exit: %{}] - Earnings: {:.2} - ExitReason: {}",
            timestamp,
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
