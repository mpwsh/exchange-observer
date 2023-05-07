use crate::{
    models::{Selected, Token, TokenStatus, TradeOrderState},
    utils::calculate_fees,
};
use exchange_observer::{Authentication, Exchange, Strategy};

#[derive(Debug, Clone)]
pub struct Account {
    pub name: String,
    pub authentication: Authentication,
    pub balance: Balance,
    pub earnings: f64,
    pub fee_spend: f64,
    pub change: f32,
    pub deny_list: Vec<String>,
    pub portfolio: Vec<Selected>,
}

#[derive(Debug, Clone)]
pub struct Balance {
    pub start: f64,
    pub current: f64,
    pub available: f64,
    pub spendable: f64,
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
            deny_list: Vec::new(),
            earnings: 0.00,
            portfolio: Vec::new(),
        }
    }
    pub fn calculate_balance(&mut self) -> &mut Self {
        self.portfolio.iter().for_each(|t| {
            if t.status == TokenStatus::Trading {
                self.balance.current += t.price * t.balance.current;
            }
        });
        self.balance.current += self.balance.available;
        self
    }

    pub fn deduct_fees(&mut self, exchange: &Exchange) {
        for t in self.portfolio.iter_mut() {
            if t.order.state == TradeOrderState::Filled && !t.fees_deducted {
                let fee = calculate_fees(self.balance.spendable, exchange.maker_fee);
                //Add transaction fee to fee spend
                self.fee_spend += fee;

                t.fees_deducted = true;
            }
        }
    }

    pub fn calculate_earnings(&mut self) -> &mut Self {
        self.earnings = self.balance.current - self.balance.start;
        self
    }

    pub fn setup_balance(&mut self, balance: f64, spendable: f64) -> &mut Self {
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
            self.portfolio.remove(pos);
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

impl Balance {
    pub fn setup(&mut self, amount: f64, spendable: f64) -> &mut Self {
        self.start = amount;
        self.available = amount;
        self.spendable = spendable;
        self.available = amount;
        self
    }
    pub fn set_current(&mut self, amount: f64) -> &mut Self {
        self.current = amount;
        self
    }
}
