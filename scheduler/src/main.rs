#![feature(drain_filter)]
use account::*;
use anyhow::Result;
use app::App;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use exchange_observer::AppConfig;
use models::TokenStatus;
use scylla::macros::FromRow;
use std::error::Error;
use std::time;
use utils::*;
mod account;
mod app;
mod models;
mod okx;
mod ui;
pub mod utils;

const REFRESH_CYCLES: u64 = 950;
const NOTIFY_SECS: i64 = 1800;
const ORDER_CHECK_DELAY_SECS: i64 = 3;
const BASE_URL: &str = "https://www.okx.com";
const ORDERS_ENDPOINT: &str = "/api/v5/trade/order";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut cfg: AppConfig = AppConfig::load()?;
    //hash and save the strategy to the DB
    cfg.strategy.hash = cfg.strategy.get_hash();

    let mut app = App::init(&cfg).await;

    //setup account balance and spendable per token
    let mut account = Account::new();
    account.setup_balance(cfg.account.balance, cfg.account.spendable);
    account.authentication = cfg.exchange.clone().unwrap_or_default().authentication;
    //cfg.strategy.sane_defaults();
    app.set_cooldown(cfg.strategy.cooldown);
    app.term.hide_cursor()?;

    app.save_strategy(&cfg.strategy).await;

    loop {
        app.term.move_cursor_to(0, 0)?;
        app.time.utc = Utc::now();
        let unix_timestamp = app.time.utc.timestamp();
        app.time.now = time::Instant::now();

        //get candles from all tokens on the selected timeframe
        app.fetch_candles(cfg.strategy.timeframe)
            .await
            .sum_candles()
            .filter_invalid(&cfg.strategy, account.balance.spendable);

        //print amount of tokens before poping
        app.token_count = app.tokens.len();
        app.keep(cfg.strategy.top);

        app.update_cooldown(&account.portfolio);

        //buy tokens instantly if quickstart enabled
        if cfg.strategy.quickstart && app.time.uptime.num_milliseconds() < 3000 {
            app.set_cooldown(1);
        } else {
            app.set_cooldown(cfg.strategy.cooldown);
        }
        //get current price of tokens
        app.get_tickers().await;

        //update timers in portfolio tokens
        account.portfolio = app.reset_timeouts(account.portfolio);

        //buy tokens if cooldown == 0 and not already in portfolio
        account = app.buy_valid(account, &cfg.strategy).await?;

        account.portfolio = app
            .update_portfolio_candles(cfg.strategy.timeframe, account.portfolio)
            .await;
        //reset account balance (re-calculate from portfolio ticker prices)
        account.balance.set_current(0.0);

        if unix_timestamp.rem_euclid(ORDER_CHECK_DELAY_SECS) == 0 {
            //Check and update order states
            account.portfolio = app.update_order_states(account.portfolio).await?;
            //Cancel live orders if above order_ttl
            //account.portfolio = app.cancel_expired_orders(account.portfolio).await?;
        }
        //deduct trade fees
        account.deduct_fees(&app.exchange);

        //update tickers
        account.portfolio = app.fetch_portfolio_tickers(account.portfolio).await;

        //update reports
        account.portfolio = app
            .update_reports(account.portfolio, cfg.strategy.timeout)
            .await;

        //update account balance
        account.calculate_balance().calculate_earnings();
        //Update change
        account.change = get_percentage_diff(account.balance.current, account.balance.start);

        //keep tokens with balance > 0 usd
        account.portfolio.retain(|t| t.balance.current > 0.0);

        //Selling and token removal validation occurs on every loop
        //let iter_portfolio = account.portfolio.clone();
        //let mut newdeny = cfg.strategy.denylist.clone().unwrap();

        account = app.sell_invalid(account, &cfg.strategy).await?;

        //Save changes in actual account struct.
        //account.portfolio = iter_portfolio;
        if app.pushover.enable {
            app.send_notifications(&account, &cfg).await?;
        }

        //Remove sold tokens
        account.token_cleanup();

        if app.cycles.rem_euclid(REFRESH_CYCLES) == 0 {
            app.term.clear_screen()?;
        }

        if app.logs.len() > 3 {
            app.logs.drain(..app.logs.len() - 3);
        };

        for line in ui::display(&cfg, &app, &account)?.iter() {
            app.term.write_line(&format!("{}", line))?
        }
        app.time.uptime = app.time.uptime + app.time.elapsed;
        app.time.elapsed = Duration::milliseconds(app.time.now.elapsed().as_millis() as i64);

        if unix_timestamp.rem_euclid(NOTIFY_SECS) == 0 {
            app.notify(
                "Balance status".to_string(),
                format!(
                    "Current: ${:.2} | Ch: {:.2}\nFees: {:.2} | Earned: {:.2}\nUptime: {} min\nStrategy: {:.7}",
                    account.balance.current,
                    account.change,
                    account.fee_spend,
                    account.earnings,
                    app.time.uptime.num_minutes(),
                    &cfg.strategy.hash
                ),
            )
            .await?;
        }
        app.cycles += 1;
    }
}
