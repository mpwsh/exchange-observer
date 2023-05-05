#![feature(drain_filter)]
#![allow(unused_variables)]
use anyhow::Result;
use app::App;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use exchange_observer::AppConfig;
use models::{Account, TokenStatus};
use scylla::macros::FromRow;
use std::error::Error;
use std::time;
use utils::*;
mod app;
mod models;
mod ui;
mod utils;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut cfg: AppConfig = AppConfig::load()?;
    let mut app = App::init(&cfg).await;
    //setup account balance and spendable per token
    let mut account = Account::new();
    account.setup_balance(cfg.account.balance as f32, cfg.account.spendable as f32);

    //cfg.strategy.sane_defaults();
    app.set_cooldown(cfg.strategy.cooldown);
    app.term.hide_cursor()?;

    //hash and save the strategy to the DB
    cfg.strategy.hash = cfg.strategy.get_hash();
    app.save_strategy(&cfg.strategy).await;

    loop {
        // reset cursor
        app.term.move_cursor_to(0, 0)?;
        //reset time
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
        account.portfolio = app.reset_timeouts(
            account.portfolio,
            //cfg.strategy.timeout,
            //cfg.strategy.sell_floor.unwrap_or(0.0),
        );
        //buy tokens if cooldown == 0 and not already in portfolio
        account = app.buy_valid(account, &cfg.strategy).await?;

        account.portfolio = app
            .update_portfolio_candles(cfg.strategy.timeframe, account.portfolio)
            .await;
        //reset account balance (re-calculate from portfolio ticker prices)
        account.balance.set_current(0.0);
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

        //remove tokens with balance < 0 usd
        account.portfolio.retain(|t| t.balance.current > 0.0);

        //Selling and token removal validation occurs on every loop
        //let iter_portfolio = account.portfolio.clone();
        //let mut newdeny = cfg.strategy.denylist.clone().unwrap();
        account = app.sell_invalid(account, &cfg.strategy).await;

        //Save changes in actual account struct.
        //account.portfolio = iter_portfolio;
        for t in account.portfolio.iter_mut() {
            //send notifications
            if t.change >= cfg.strategy.cashout {
                app.notify(
                    "Cashout Triggered".to_string(),
                    format!(
                        "Token: {} | Change: %{:.2}\nEarnings: {:.2}\nTime Left: {} secs",
                        t.instid, t.report.change, t.report.earnings, t.report.time_left,
                    ),
                )
                .await?;
            }
            if t.change <= -cfg.strategy.stoploss {
                app.notify(
                    "Stoploss Triggered".to_string(),
                    format!(
                        "Token: {} | Change: %{:.2}\nLoss: {:.2}\nTime Left: {} secs",
                        t.instid, t.report.change, t.report.earnings, t.report.time_left,
                    ),
                )
                .await?;
            };
        }
        //Remove sold tokens
        account.token_cleanup();
        //cfg.strategy.denylist = Some(newdeny.clone());
        //app.denied_tokens = newdeny;
        if app.cycles.rem_euclid(950) == 0 {
            // || app.logs.len() >= 6 {
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

        //save reports every 15 seconds and clear account history
        // if unix_timestamp.rem_euclid(15) == 0 && !account.history.is_empty() {
        //    app.save_reports(&account.history).await;
        //    account.history.clear();
        //};

        //notify account status every hour
        if unix_timestamp.rem_euclid(1800) == 0 {
            app.notify(
                "Balance status".to_string(),
                format!(
                    "Current: ${:.2} | Ch: {:.2}\nFees: {:.2} | Earned: {:.2}\nUptime: {} min\nStrategy: {:.7}",
                    account.balance.current,
                    account.change,
                    account.fee_spend,
                    account.earnings,
                    app.time.uptime.num_minutes(),
                    cfg.strategy.hash.clone().unwrap()
                ),
            )
            .await?;
        }
        //remove with loop
        if 1 < 0 {
            break;
        };
        app.cycles += 1;
    }
    Ok(())
}
