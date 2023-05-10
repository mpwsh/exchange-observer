use crate::prelude::*;
use serde::Serializer;
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[serde(rename(serialize = "snake_case", deserialize = "camelCase"))]
pub struct Order {
    pub ord_id: String,
    pub inst_id: String,
    pub td_mode: String,
    pub cl_ord_id: String,
    #[serde(serialize_with = "serialize_side_lower")]
    pub side: Side,
    pub ord_type: String,
    pub px: String,
    pub sz: String,
    pub ts: String,
    #[serde(skip_serializing)]
    pub state: OrderState,
    #[serde(skip_serializing)]
    pub prev_state: OrderState,
    #[serde(skip_serializing)]
    pub fees_deducted: bool,
    pub strategy: String,
    #[serde(skip_serializing)]
    pub response: Option<OkxOrderResponse>,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize, Clone)]
pub enum OrderState {
    #[default]
    Created,
    Live,
    PartiallyFilled,
    Cancelled,
    Filled,
}
impl ToString for OrderState {
    fn to_string(&self) -> String {
        match self {
            Self::Created => "Live".to_string(),
            Self::Live => "Live".to_string(),
            Self::PartiallyFilled => "Partially Filled".to_string(),
            Self::Filled => "Filled".to_string(),
            Self::Cancelled => "Cancelled".to_string(),
        }
    }
}
impl FromStr for OrderState {
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

#[derive(Serialize, Deserialize, Eq, PartialEq, Debug, Clone)]
pub enum ExitReason {
    Stoploss,
    LowVolume,
    LowChange,
    FloorReached,
    Timeout,
    Cashout,
}
#[derive(Eq, PartialEq, Debug, Default, Serialize, Deserialize, Clone)]
pub enum Side {
    #[default]
    Buy,
    Sell,
}
impl ToString for Side {
    fn to_string(&self) -> String {
        match self {
            Self::Buy => "buy".to_string(),
            Self::Sell => "sell".to_string(),
        }
    }
}

impl FromStr for Side {
    type Err = ();
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let lower = input.to_lowercase();
        match lower.as_ref() {
            "buy" => Ok(Self::Buy),
            "sell" => Ok(Self::Sell),
            _ => Err(()),
        }
    }
}
impl ToString for ExitReason {
    fn to_string(&self) -> String {
        match self {
            Self::Stoploss => "stoploss".to_string(),
            Self::LowVolume => "low_volume".to_string(),
            Self::LowChange => "low_change".to_string(),
            Self::FloorReached => "floor_reached".to_string(),
            Self::Timeout => "timeout".to_string(),
            Self::Cashout => "cashout".to_string(),
        }
    }
}

impl FromStr for ExitReason {
    type Err = ();
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let lower = input.to_lowercase();
        match lower.as_ref() {
            "stoploss" => Ok(Self::Stoploss),
            "low_volume" => Ok(Self::LowVolume),
            "low_change" => Ok(Self::LowChange),
            "floor_reached" => Ok(Self::FloorReached),
            "timeout" => Ok(Self::Timeout),
            "cashout" => Ok(Self::Cashout),
            _ => Err(()),
        }
    }
}

impl Order {
    pub fn new(instid: &str, price: String, size: String, side: Side, strategy: &str) -> Self {
        let ord_type = match side {
            Side::Buy => "ioc",
            Side::Sell => "ioc",
        };
        let num = size.parse::<f64>().unwrap();
        let size = (num * 1_000_000.0).floor() / 1_000_000.0;

        Self {
            cl_ord_id: Uuid::new_v4().hyphenated().to_string().replace('-', ""),
            ord_id: String::new(),
            inst_id: instid.to_string(),
            td_mode: String::from("cash"),
            side,
            ord_type: ord_type.to_string(),
            px: price,
            sz: size.to_string(),
            strategy: strategy.to_string(),
            response: None,
            fees_deducted: false,
            prev_state: OrderState::Created,
            state: OrderState::Live,
            ts: Utc::now().timestamp_millis().to_string(),
        }
    }

    pub async fn get_state(&self, auth: &Authentication) -> Result<trade::OrderState> {
        let inst_id = self.inst_id.clone();
        let query = &format!("?ordId={ord_id}&instId={inst_id}", ord_id = self.ord_id);
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
        let order_state =
            trade::OrderState::from_str(&res.data[0].state).unwrap_or(trade::OrderState::Cancelled);
        Ok(order_state)
    }

    pub fn update_order_state(&mut self, state: OrderState) {
        self.state = state
    }
    pub async fn publish(&mut self, trade_enabled: bool, auth: &Authentication) -> Result<()> {
        let json_body = serde_json::to_string(&self)?;

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
                .header("OK-ACCESS-KEY", auth.access_key.clone())
                .header("OK-ACCESS-PASSPHRASE", auth.passphrase.clone())
                .header("OK-ACCESS-TIMESTAMP", signed.timestamp.as_str())
                .header("OK-ACCESS-SIGN", signed.signature.as_str())
                .json(&self)
                .send()
                .await?;

            if res.status().is_success() {
                let order_response = res.json::<OkxOrderResponse>().await;
                match order_response {
                    Ok(res) => {
                        self.response = Some(res.clone());
                        self.ord_id = res.data[0].ord_id.clone();
                    }
                    Err(e) => {
                        self.response = None;
                        log::error!("{:?}", e);
                    }
                };
            } else {
                let body = res.text().await?;
                log::error!("{}", body);
            };
        }
        Ok(())
    }
    pub fn deduct_fees(&mut self, expense: f64, exchange: &Exchange) -> f64 {
        if self.state == OrderState::Filled && !self.fees_deducted {
            let fee = calculate_fees(expense, exchange.taker_fee);

            // Set fees_deducted to true
            self.fees_deducted = true;

            // Return the calculated fee
            return fee;
        }
        0.0
    }
}

fn serialize_side_lower<S>(side: &Side, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&side.to_string())
}
