use crate::prelude::*;
use exchange_observer::{Authentication, Strategy};

#[derive(Debug, Clone)]
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

    pub fn calculate_balance(&mut self, app: &mut crate::App) -> &mut Self {
        let usdt_taker_fee = calculate_fees(self.balance.spendable, 0.10);

        for t in self.portfolio.iter_mut() {
            if let Some(orders) = t.orders.as_mut() {
                for order in orders.iter_mut() {
                    if order.state == order.prev_state {
                        continue;
                    }
                    let (price, size) = match (order.px.parse::<f64>(), order.sz.parse::<f64>()) {
                        (Ok(price), Ok(size)) => (price, size),
                        _ => {
                            log::error!("Failed to parse price or size as f64");
                            continue;
                        }
                    };
                    match order.state {
                        OrderState::Live => match order.side {
                            Side::Buy => {
                                let order_amount = (size * price) + usdt_taker_fee;
                                self.balance.available -= order_amount;
                                self.balance.current += order_amount;
                            }
                            Side::Sell => {
                                t.balance.available -= size;
                            }
                        },
                        OrderState::Cancelled => match order.side {
                            Side::Buy => {
                                self.balance.available += (size * price) + usdt_taker_fee;
                            }
                            Side::Sell => {
                                t.balance.available = t.balance.start;
                            }
                        },
                        OrderState::Filled => {
                            self.trades += 1;
                            self.fee_spend += usdt_taker_fee;
                            match order.side {
                                Side::Buy => {
                                    t.balance.current = t.balance.start;
                                    t.balance.available = t.balance.start;
                                }
                                Side::Sell => {
                                    t.balance.current -= size;
                                    self.balance.available += (size * price) - usdt_taker_fee;
                                }
                            }
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
            self.balance.current += t.balance.current * t.price;
        }
        self.balance.current += self.balance.available;
        self.change = get_percentage_diff(self.balance.current, self.balance.start);
        self
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
            let exited = t.orders.as_ref().map_or(false, |orders| {
                orders
                    .iter()
                    .any(|o| o.side == Side::Sell && o.state == OrderState::Filled)
            });

            let cancelled = t.orders.as_ref().map_or(false, |orders| {
                orders
                    .iter()
                    .any(|o| o.side == Side::Buy && o.state == OrderState::Cancelled)
            });

            // Keep the token if it doesn't have a filled sell order or a cancelled buy order
            !(exited || cancelled)
        });

        self
    }
    pub fn add_token(&mut self, token: &Token, strategy: &Strategy) -> &Self {
        if self.balance.available >= self.balance.spendable
            && self.portfolio.len() < strategy.portfolio_size as usize
        {
            let mut s = Token::new(&token.instid);
            s.price = token.price;
            s.candlesticks = token.candlesticks.clone();
            s.buy_price = token.price;
            self.portfolio.push(s);
        }
        self
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
