pub use prelude::*;

mod app;
mod models;
mod okx;
pub mod prelude;
mod ui;
mod utils;

const REFRESH_CYCLES: u64 = 950;
const NOTIFY_SECS: i64 = 1800;
const ORDER_CHECK_DELAY_SECS: i64 = 3;

pub const BASE_URL: &str = "https://www.okx.com";
pub const ORDERS_ENDPOINT: &str = "/api/v5/trade/order";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut cfg: AppConfig = AppConfig::load()?;
    //hash and save the strategy to the DB
    cfg.strategy.hash = cfg.strategy.get_hash();

    let mut app = App::init(&cfg).await;

    //setup account balance and spendable per token
    let mut account = Account::new().set_balance(cfg.account.balance, cfg.account.spendable);
    account.authentication = cfg.exchange.clone().unwrap_or_default().authentication;
    //cfg.strategy.sane_defaults();
    app.set_cooldown(cfg.strategy.cooldown);
    app.term.hide_cursor()?;

    app.save_strategy(&cfg.strategy).await?;

    if cfg.strategy.quickstart {
        app.set_cooldown(1);
    };
    loop {
        app.term.move_cursor_to(0, 0)?;
        app.time.utc = Utc::now();
        let unix_timestamp = app.time.utc.timestamp();
        app.time.now = time::Instant::now();

        //Retrieve and process top tokens
        app.fetch_top_tokens(cfg.strategy.timeframe)
            .await
            .filter_invalid(&cfg.strategy, account.balance.spendable)
            .get_tickers_full()
            .await
            .clean_top(cfg.strategy.top);

        //Portoflio
        app.update_cooldown(&account.portfolio);

        //update timers in portfolio tokens
        account = app.buy_tokens(account, &cfg.strategy).await?;

        account.portfolio = app.reset_timeouts(account.portfolio, &cfg.strategy);
        account.portfolio = app
            .update_candles(cfg.strategy.timeframe, account.portfolio)
            .await;

        if unix_timestamp.rem_euclid(ORDER_CHECK_DELAY_SECS) == 0 {
            //Check and update order states
            account.portfolio = app.update_order_states(account.portfolio).await?;
            //TODO: Cancel live orders if above order_ttl
            //account.portfolio = app.cancel_expired_orders(account.portfolio).await?;
        }

        //update reports
        account.portfolio = app.update_reports(account.portfolio, cfg.strategy.timeout);

        //keep tokens with balance > 0 usd
        //account.portfolio.retain(|t| t.balance.current > 0.0);

        //Selling and token removal validation occurs on every loop
        account = app.tag_invalid_tokens(account, &cfg.strategy)?;
        account = app.sell_tokens(account, &cfg.strategy).await?;

        //System && UI
        //Save changes in actual account struct.
        if app.pushover.enable {
            app.send_notifications(&account).await?;
        }

        account.balance.set_current(0.0);
        account
            .calculate_balance(&mut app)
            .calculate_earnings()
            .clean_portfolio();

        //update tickers
        account.portfolio = app.get_tickers_simple(account.portfolio).await;

        //clear screen
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

        app.set_cooldown(cfg.strategy.cooldown);
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
