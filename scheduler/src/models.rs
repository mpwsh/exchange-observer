use crate::{Duration, FromRow, Result, Utc};
use exchange_observer::Strategy;
use serde_derive::{Deserialize, Serialize};
use std::str::FromStr;
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
    pub vol: f32,
    pub vol24h: f32,
    pub change24h: f32,
    pub range24h: f32,
    pub px: f32,
    pub buys: i64,
    pub sells: i64,
    pub cooldown: Duration,
    pub candlesticks: Vec<Candlestick>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub round_id: u64,
    pub instid: String,
    pub buy_price: f32,
    pub sell_price: f32,
    pub earnings: f32,
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
#[derive(Debug, Clone)]
pub struct Account {
    pub name: String,
    pub balance: Balance,
    pub earnings: f32,
    pub fee_spend: f32,
    pub change: f32,
    pub deny_list: Vec<String>,
    pub portfolio: Vec<Selected>,
}

#[derive(Debug, Clone)]
pub struct Balance {
    pub start: f32,
    pub current: f32,
    pub available: f32,
    pub spendable: f32,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TradeOrder {
    pub instid: String,
    pub td_mode: String,
    pub cl_ord_id: Option<String>,
    pub side: String,
    pub ord_type: String,
    pub px: String,
    pub sz: String,
}
#[derive(Debug, Clone)]
pub struct Selected {
    pub round_id: u64,
    pub instid: String,
    pub buy_price: f32,
    pub price: f32,
    pub change: f32,
    pub timeout: Duration,
    pub balance: Balance,
    pub earnings: f32,
    pub status: TokenStatus,
    pub candlesticks: Vec<Candlestick>,
    pub config: SelectedConfig,
    pub report: Report,
}

#[derive(FromRow, Debug, Clone)]
pub struct Candlestick {
    pub instid: String,
    pub ts: Duration,
    pub change: f32,
    pub close: f32,
    pub high: f32,
    pub low: f32,
    pub open: f32,
    pub range: f32,
    pub vol: f32,
}
impl Account {
    pub fn new() -> Self {
        Self {
            name: String::new(),
            change: 0.00,
            balance: Balance {
                start: 0.00,
                current: 0.00,
                available: 0.00,
                spendable: 0.00,
            },
            fee_spend: 0.0,
            deny_list: Vec::new(),
            earnings: 0.00,
            portfolio: Vec::new(),
        }
    }
    pub fn calculate_balance(&mut self) -> &mut Self {
        self.portfolio.iter().for_each(|t| {
            self.balance.current += t.price * t.balance.current;
        });
        self.balance.current += self.balance.available;
        self
    }

    pub fn calculate_earnings(&mut self) -> &mut Self {
        self.earnings = self.balance.current - self.balance.start;
        self
    }

    pub fn setup_balance(&mut self, balance: f32, spendable: f32) -> &mut Self {
        self.balance.setup(balance, spendable);
        self
    }

    pub fn token_cleanup(&mut self) -> &Self {
        if let Some(pos) = self
            .portfolio
            .iter()
            //remove marked as selling
            //.position(|s| instid == s.instid && (s.price * s.balance.available < 10.0))
            .position(|t| t.status == TokenStatus::Selling)
        {
            let token = self.portfolio.remove(pos);
        }
        self
    }
    pub fn add_token(&mut self, token: &Token, strategy: &Strategy) -> &Self {
        if self.balance.available >= self.balance.spendable
            && self.portfolio.len() < strategy.portfolio_size as usize
        {
            let mut s = Selected::new(&token.instid);
            s.price = token.px;
            s.candlesticks = token.candlesticks.clone();
            s.status = TokenStatus::Buy;
            s.buy_price = token.px;
            self.portfolio.push(s);
        }
        self
    }
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
            balance: Balance {
                current: 0.0,
                start: 0.0,
                available: 0.0,
                spendable: 0.0,
            },
            earnings: 0.00,
            timeout: Duration::seconds(0),
            config: SelectedConfig::default(),
            change: 0.00,
            candlesticks: Vec::new(),
            report: Report::default(),
            status: TokenStatus::Waiting,
        }
    }
    pub async fn buy(&mut self) -> Result<&Self> {
        self.status = TokenStatus::Buying;

        let order = TradeOrder::new(
            &self.instid,
            "buy",
            self.balance.start.to_string(),
            self.buy_price.to_string(),
        );
        /*
        let trade: TradeOrder = reqwest::Client::new()
            .post("https://jsonplaceholder.typicode.com/posts")
            .json(&order)
            .send()
            .await?
            .json()
            .await?;
        log::info!("{:?}", trade);
        */
        self.status = TokenStatus::Trading;
        Ok(self)
    }
    pub async fn sell(&mut self) -> Result<&Self> {
        let order = TradeOrder::new(
            &self.instid,
            "sell",
            self.balance.start.to_string(),
            self.price.to_string(),
        );
        /*
        let trade: TradeOrder = reqwest::Client::new()
            .post("https://jsonplaceholder.typicode.com/sell")
            .json(&order)
            .send()
            .await?
            .json()
            .await?;
        log::info!("{:?}", trade);
            */
        self.status = TokenStatus::Selling;
        Ok(self)
    }
}
impl Balance {
    pub fn setup(&mut self, amount: f32, spendable: f32) -> &mut Self {
        self.start = amount;
        self.available = amount;
        self.spendable = spendable;
        self.available = amount;
        self
    }
    pub fn set_current(&mut self, amount: f32) -> &mut Self {
        self.current = amount;
        self
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
    pub fn new(instid: &str, side: &str, price: String, size: String) -> Self {
        Self {
            instid: instid.to_string(),
            td_mode: String::from("cash"),
            cl_ord_id: None,
            side: side.to_string(),
            ord_type: String::from("limit"),
            px: price,
            sz: size,
        }
    }
}
