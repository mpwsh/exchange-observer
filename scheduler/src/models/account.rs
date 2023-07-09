use crate::prelude::*;
use exchange_observer::{Authentication, Strategy};

#[derive(Serialize, Debug, Clone)]
pub struct Account {
    pub name: String,
    pub authentication: Authentication,
    pub balance: Balance,
    pub earnings: f64,
    pub trades: u64,
    pub fee_spend: f64,
    pub change: f32,
    pub deny_list: Vec<String>,
    pub portfolio: Vec<Token>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Balance {
    pub start: f64,
    pub current: f64,
    pub available: f64,
    pub spendable: f64,
}
impl Default for Account {
    fn default() -> Self {
        Account::new()
    }
}

impl Account {
    pub fn new() -> Self {
        Self {
            name: String::new(),
            change: 0.00,
            authentication: Authentication::default(),
            balance: Balance {
                start: 0.00,
                current: 0.00,
                available: 0.00,
                spendable: 0.00,
            },
            fee_spend: 0.0,
            trades: 0,
            deny_list: Vec::new(),
            earnings: 0.00,
            portfolio: Vec::new(),
        }
    }

    pub async fn calculate_balance(&mut self, app: &mut App) -> Result<&mut Self> {
        let mut open_order_value = 0.0;
        let mut token_balances = 0.0;
        for t in self.portfolio.iter_mut() {
            if let Some(orders) = t.orders.as_mut() {
                for order in orders.iter_mut() {
                    if order.state == order.prev_state {
                        continue;
                    }
                    if order.id.is_empty()
                        && order.state != OrderState::Created
                        && app.exchange.enable_trading
                    {
                        order.state = OrderState::Failed;
                    }

                    //Get price and size of the order
                    let (price, size) = match (order.px.parse::<f64>(), order.sz.parse::<f64>()) {
                        (Ok(price), Ok(size)) => (price, size),
                        _ => {
                            app.logs.push(format!(
                                "Failed to parse price or size for token {}",
                                t.instid
                            ));
                            continue;
                        }
                    };

                    //calculate fees based on the order
                    let usdt_taker_fee =
                        calculate_fees(self.balance.spendable, app.exchange.taker_fee);
                    let usdt_order_amount = (size * price) - usdt_taker_fee;
                    let token_balance_after_fees =
                        size - calculate_fees(size, app.exchange.taker_fee); //+ 0.0001);
                    match order.state {
                        OrderState::Live => match order.side {
                            Side::Buy => {
                                self.balance.available -= self.balance.spendable;
                                open_order_value += self.balance.spendable;
                            }
                            Side::Sell => {
                                t.balance.available -= size;
                                open_order_value += usdt_order_amount;
                            }
                        },
                        OrderState::Cancelled => match order.side {
                            Side::Buy => {
                                self.balance.available += self.balance.spendable;
                            }
                            Side::Sell => {
                                t.balance.available += size;
                            }
                        },
                        OrderState::Failed => match order.side {
                            Side::Buy => (),
                            Side::Sell => {
                                if app.exchange.enable_trading {
                                    t.balance.available = Account::get_balance(
                                        &t.instid.replace("-USDT", ""),
                                        &app.exchange.authentication,
                                    )
                                    .await
                                    .unwrap_or_default();
                                }
                            }
                        },
                        OrderState::Filled => {
                            match order.side {
                                Side::Buy => {
                                    if app.exchange.enable_trading {
                                        log::info!("Retrieving balance from exchange");
                                        let balance = Account::get_balance(
                                            &t.instid.replace("-USDT", ""),
                                            &app.exchange.authentication,
                                        )
                                        .await?;

                                        //Add the balance to the token. this will be balance with fees discounted.
                                        t.balance.available = balance;
                                        t.balance.current = t.balance.available;
                                    } else {
                                        //Calculate fees locally
                                        t.balance.available = token_balance_after_fees;
                                        t.balance.current = token_balance_after_fees;
                                    }
                                }
                                Side::Sell => {
                                    t.balance.current -= size;
                                    self.balance.available += token_balance_after_fees * price;
                                    app.logs.push(t.report.to_string());
                                }
                            }
                            self.trades += 1;
                            self.fee_spend += usdt_taker_fee;
                            t.buy_ts = Duration::milliseconds(Utc::now().timestamp_millis());
                        }
                        _ => {}
                    };
                    //log the order
                    if order.prev_state != OrderState::Created {
                        app.logs.push(app.build_order_log(order));
                    };
                    //lock the order
                    order.prev_state = order.state.clone();
                }
            }
            token_balances += t.balance.available * t.price;
        }
        self.balance.current += self.balance.available + open_order_value + token_balances;
        self.change = get_percentage_diff(self.balance.current, self.balance.start);
        Ok(self)
    }

    pub fn calculate_earnings(&mut self) -> &mut Self {
        self.earnings = self.balance.current - self.balance.start;
        self
    }

    pub fn set_balance(mut self, balance: f64, spendable: f64) -> Self {
        self.balance.setup(balance, spendable);
        self
    }

    pub fn clean_portfolio(&mut self) -> &Self {
        self.portfolio.retain(|t| {
            let waiting = t.status == token::Status::Waiting;
            let live_orders = if let Some(orders) = t.orders.clone() {
                orders.iter().any(|o| o.state == OrderState::Live)
            } else {
                true
            };

            let remaining_balance = t.balance.available * t.price > 2.0;
            let retain = waiting || live_orders || remaining_balance;
            if !retain {
                log::info!(
                    "Removing token: {} -- token state: {:?}",
                    t.instid,
                    t.status
                );
            }
            retain
        });
        self
    }

    pub fn add_token(&mut self, token: &Token, strategy: &Strategy) -> &Self {
        if self.balance.available >= self.balance.spendable
            && self.portfolio.len() < strategy.portfolio_size as usize
        {
            let mut t = Token::new(&token.instid);
            t.price = token.price;
            t.status = token::Status::Buying;
            t.candlesticks = token.candlesticks.clone();
            self.portfolio.push(t);
        }
        self
    }

    pub async fn get_balance(token_id: &str, auth: &Authentication) -> Result<f64> {
        let query = &format!("?ccy={token_id}", token_id = token_id);
        let signed = auth.sign(
            "GET",
            BALANCE_ENDPOINT,
            OffsetDateTime::now_utc(),
            false,
            query,
        )?;
        log::info!("Retrieving balance of: {}", token_id);
        let res = reqwest::Client::new()
            .get(format!("{BASE_URL}{BALANCE_ENDPOINT}{query}"))
            .header("OK-ACCESS-KEY", &auth.access_key)
            .header("OK-ACCESS-PASSPHRASE", &auth.passphrase)
            .header("OK-ACCESS-TIMESTAMP", signed.timestamp.as_str())
            .header("OK-ACCESS-SIGN", signed.signature.as_str())
            .send()
            .await?
            .json::<OkxAccountBalanceResponse>()
            .await?;

        log::info!("Response: {}", serde_json::to_string_pretty(&res)?);

        let balance =
            if let Some(balance) = res.data.get(0).and_then(|balance| balance.details.get(0)) {
                balance
                    .avail_bal
                    .parse::<f64>()
                    .map_err(|_| anyhow::anyhow!("Failed to parse balance"))?
            } else {
                return Err(anyhow::anyhow!("Failed to retrieve balance details"));
            };
        Ok(balance.to_owned())
    }
}

impl Balance {
    pub fn setup(&mut self, amount: f64, spendable: f64) -> &mut Self {
        self.start = amount;
        self.available = amount;
        self.spendable = spendable;
        self.current = amount;
        self
    }
    pub fn set_current(&mut self, amount: f64) -> &mut Self {
        self.current = amount;
        self
    }
}
