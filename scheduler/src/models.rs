use crate::{account::Balance, okx::*, Duration, FromRow, Result, Utc, BASE_URL, ORDERS_ENDPOINT};
use exchange_observer::{Authentication, OffsetDateTime};
use serde_derive::{Deserialize, Serialize};
use std::str::FromStr;
use uuid::Uuid;

#[derive(Eq, PartialEq, Debug, Clone)]
pub enum TokenStatus {
    Buy,
    Sell,
    Buying,
    Selling,
    Waiting,
    Trading,
}

#[derive(Eq, PartialEq, Debug, Clone)]
pub enum Reason {
    Stoploss,
    Buy,
    LowVolume,
    LowChange,
    FloorReached,
    Timeout,
    Cashout,
}

impl ToString for Reason {
    fn to_string(&self) -> String {
        match self {
            Self::Stoploss => "stoploss".to_string(),
            Self::LowVolume => "low_volume".to_string(),
            Self::LowChange => "low_change".to_string(),
            Self::FloorReached => "floor_reached".to_string(),
            Self::Timeout => "timeout".to_string(),
            Self::Buy => "buy".to_string(),
            Self::Cashout => "cashout".to_string(),
        }
    }
}

impl FromStr for Reason {
    type Err = ();
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let lower = input.to_lowercase();
        match lower.as_ref() {
            "stoploss" => Ok(Self::Stoploss),
            "buy" => Ok(Self::Buy),
            "low_volume" => Ok(Self::LowVolume),
            "low_change" => Ok(Self::LowChange),
            "floor_reached" => Ok(Self::FloorReached),
            "timeoutc" => Ok(Self::Timeout),
            "cashout" => Ok(Self::Cashout),
            _ => Err(()),
        }
    }
}
#[derive(Debug, Clone)]
pub struct Token {
    pub instid: String,
    pub change: f32,
    pub std_deviation: f32,
    pub range: f32,
    pub vol: f64,
    pub vol24h: f64,
    pub change24h: f32,
    pub range24h: f32,
    pub px: f64,
    pub buys: i64,
    pub sells: i64,
    pub cooldown: Duration,
    pub candlesticks: Vec<Candlestick>,
}

#[derive(Debug, Clone)]
pub struct Selected {
    pub round_id: u64,
    pub instid: String,
    pub buy_price: f64,
    pub price: f64,
    pub change: f32,
    pub std_deviation: f32,
    pub timeout: Duration,
    pub balance: Balance,
    pub earnings: f64,
    pub status: TokenStatus,
    pub fees_deducted: bool,
    pub candlesticks: Vec<Candlestick>,
    pub config: SelectedConfig,
    pub order: TradeOrder,
    pub report: Report,
}

#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone)]
pub struct SelectedConfig {
    pub sell_floor: f32,
    pub timeout: Duration,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[serde(rename(serialize = "snake_case", deserialize = "camelCase"))]
pub struct TradeOrder {
    pub ord_id: String,
    pub inst_id: String,
    pub td_mode: String,
    pub cl_ord_id: String,
    pub side: String,
    pub ord_type: String,
    pub px: String,
    pub sz: String,
    pub ts: String,
    #[serde(skip_serializing)]
    pub state: TradeOrderState,
    pub strategy: String,
    #[serde(skip_serializing)]
    pub response: Option<OkxOrderResponse>,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize, Clone)]
pub enum TradeOrderState {
    #[default]
    Live,
    PartiallyFilled,
    Cancelled,
    Filled,
}
impl ToString for TradeOrderState {
    fn to_string(&self) -> String {
        match self {
            Self::Live => "Live".to_string(),
            Self::PartiallyFilled => "Partially Filled".to_string(),
            Self::Filled => "Filled".to_string(),
            Self::Cancelled => "Cancelled".to_string(),
        }
    }
}
impl FromStr for TradeOrderState {
    type Err = ();
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let lower = input.to_lowercase().replace('"', "");
        match lower.as_ref() {
            "live" => Ok(Self::Live),
            "partially_filled" => Ok(Self::PartiallyFilled),
            "filled" => Ok(Self::Filled),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(()),
        }
    }
}

#[derive(FromRow, Debug, Clone)]
pub struct Candlestick {
    pub instid: String,
    pub ts: Duration,
    pub change: f32,
    pub close: f64,
    pub high: f64,
    pub low: f64,
    pub open: f64,
    pub range: f32,
    pub vol: f64,
}

impl Token {
    pub fn new(instid: String, cooldown: i64) -> Self {
        Self {
            instid,
            change: 0.0,
            range: 0.0,
            std_deviation: 0.0,
            vol: 0.0,
            vol24h: 0.0,
            change24h: 0.0,
            range24h: 0.0,
            px: 0.0,
            buys: 0,
            sells: 0,
            candlesticks: Vec::new(),
            cooldown: Duration::seconds(cooldown),
        }
    }
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
impl Default for SelectedConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::seconds(0),
            sell_floor: 0.0,
        }
    }
}
impl Selected {
    pub fn new(instid: &str) -> Self {
        Self {
            round_id: 0,
            instid: instid.to_string(),
            buy_price: 0.0,
            price: 0.0,
            std_deviation: 0.0,
            balance: Balance {
                current: 0.0,
                start: 0.0,
                available: 0.0,
                spendable: 0.0,
            },
            earnings: 0.00,
            fees_deducted: false,
            timeout: Duration::seconds(0),
            config: SelectedConfig::default(),
            change: 0.00,
            candlesticks: Vec::new(),
            order: TradeOrder::default(),
            report: Report::default(),
            status: TokenStatus::Waiting,
        }
    }
    pub async fn get_order_state(
        &mut self,
        trade_enabled: bool,
        auth: &Authentication,
    ) -> Result<(TradeOrderState, String)> {
        let ord_id = &self.order.ord_id;
        let inst_id = &self.instid;
        let query = &format!("?ordId={ord_id}&instId={inst_id}");
        let (mut order_state, mut response) = (TradeOrderState::Live, String::new());
        if trade_enabled {
            let signed = auth.sign(
                "GET",
                ORDERS_ENDPOINT,
                OffsetDateTime::now_utc(),
                false,
                query,
            )?;

            let res = reqwest::Client::new()
                .get(format!("{BASE_URL}{ORDERS_ENDPOINT}{query}"))
                .header("OK-ACCESS-KEY", &auth.access_key)
                .header("OK-ACCESS-PASSPHRASE", &auth.passphrase)
                .header("OK-ACCESS-TIMESTAMP", signed.timestamp.as_str())
                .header("OK-ACCESS-SIGN", signed.signature.as_str())
                .send()
                .await?
                .json::<OkxOrderDetailsResponse>()
                .await?;
            order_state = TradeOrderState::from_str(&res.data[0].state)
                .unwrap_or_else(|_| TradeOrderState::from_str("cancelled").unwrap());
            response = serde_json::to_string(&res)?;
            /*
            if res.status().is_success() {
                let state_response = res.json::<OkxOrderDetailsResponse>().await;
                match state_response {
                    Ok(res) => {
                        let res_state = &res.data[0].state;
                        match TradeOrderState::from_str(res_state) {
                            Ok(k) => { order_state = k },
                            Err(e) => log::error!("Unable to get t{:?} -- {:?}", res, e),
                        }
                    }
                    Err(e) => {
                        log::error!("{:?}", e)
                    }
                };
            } else {
                let body = res.text().await?;
                log::error!("{:?}", body);
            };*/
        }
        Ok((order_state, response))
    }
    pub async fn buy(
        &mut self,
        trade_enabled: bool,
        auth: Authentication,
        strategy: &str,
    ) -> Result<&Self> {
        self.status = TokenStatus::Buying;

        self.order = TradeOrder::new(
            &self.instid,
            &self.status,
            self.buy_price.to_string(),
            self.balance.start.to_string(),
            // if using market ord_type (self.balance.start*self.buy_price).to_string(),
            strategy,
        );
        let json_body = serde_json::to_string(&self.order)?;

        if trade_enabled {
            let signed = auth.sign(
                "POST",
                ORDERS_ENDPOINT,
                OffsetDateTime::now_utc(),
                false,
                &json_body,
            )?;
            let okx_timestamp = get_time().await?;
            let exp_time = okx_timestamp + self.timeout.num_milliseconds();

            let res = reqwest::Client::new()
                .post(format!("{BASE_URL}{ORDERS_ENDPOINT}"))
                .header("OK-ACCESS-KEY", auth.access_key)
                .header("OK-ACCESS-PASSPHRASE", auth.passphrase)
                .header("OK-ACCESS-TIMESTAMP", signed.timestamp.as_str())
                .header("OK-ACCESS-SIGN", signed.signature.as_str())
                .header("expTime", exp_time.to_string())
                .json(&self.order)
                .send()
                .await?;

            if res.status().is_success() {
                let order_response = res.json::<OkxOrderResponse>().await;
                match order_response {
                    Ok(res) => {
                        self.order.response = Some(res.clone());
                        self.order.ord_id = res.data[0].ord_id.clone();
                    }
                    Err(e) => {
                        self.order.response = None;
                        log::error!("{:?}", e)
                    }
                };
            } else {
                let body = res.text().await?;
                log::error!("{}", body);
            };
        }
        Ok(self)
    }

    pub async fn sell(
        &mut self,
        trade_enabled: bool,
        auth: Authentication,
        strategy: &str,
    ) -> Result<&Self> {
        self.order = TradeOrder::new(
            &self.instid,
            &self.status,
            self.price.to_string(),
            self.balance.current.to_string(),
            strategy,
        );

        let json_body = serde_json::to_string(&self.order)?;
        if trade_enabled {
            let signed = auth.sign(
                "POST",
                ORDERS_ENDPOINT,
                OffsetDateTime::now_utc(),
                false,
                &json_body,
            )?;

            let res = reqwest::Client::new()
                .post(format!("{BASE_URL}{ORDERS_ENDPOINT}"))
                .header("OK-ACCESS-KEY", auth.access_key)
                .header("OK-ACCESS-PASSPHRASE", auth.passphrase)
                .header("OK-ACCESS-TIMESTAMP", signed.timestamp.as_str())
                .header("OK-ACCESS-SIGN", signed.signature.as_str())
                .json(&self.order)
                .send()
                .await?;
            if res.status().is_success() {
                let order_response = res.json::<OkxOrderResponse>().await;
                match order_response {
                    Ok(res) => {
                        self.order.response = Some(res.clone());
                        self.order.ord_id = res.data[0].ord_id.clone();
                    }
                    Err(e) => {
                        self.order.response = None;
                        log::error!("{:?}", e)
                    }
                };
            } else {
                let body = res.text().await?;
                log::error!("{}", body);
            };
        }
        self.status = TokenStatus::Selling;
        Ok(self)
    }
}

impl Candlestick {
    pub fn new() -> Self {
        Self {
            instid: String::new(),
            ts: Duration::seconds(0),
            change: 0.0,
            close: 0.0,
            high: 0.0,
            low: 0.0,
            open: 0.0,
            range: 0.0,
            vol: 0.0,
        }
    }
}
impl TradeOrder {
    pub fn new(
        instid: &str,
        status: &TokenStatus,
        price: String,
        size: String,
        strategy: &str,
    ) -> Self {
        let side = match status {
            TokenStatus::Buying | TokenStatus::Buy => "buy",
            TokenStatus::Selling | TokenStatus::Sell => "sell",
            TokenStatus::Waiting | TokenStatus::Trading => {
                panic!("Token entered trade mode in Trading/Waiting State. this should not happen")
            }
        };
        let ord_type = match status {
            TokenStatus::Buying | TokenStatus::Buy => "ioc",
            TokenStatus::Selling | TokenStatus::Sell => "market",
            _ => "ioc",
        };

        Self {
            cl_ord_id: Uuid::new_v4().hyphenated().to_string().replace('-', ""),
            ord_id: String::from("Simulated"),
            inst_id: instid.to_string(),
            td_mode: String::from("cash"),
            side: side.to_string(),
            ord_type: ord_type.to_string(),
            px: price,
            sz: size,
            strategy: strategy.to_string(),
            response: None,
            state: TradeOrderState::Live,
            ts: Utc::now().timestamp_millis().to_string(),
        }
    }
}
