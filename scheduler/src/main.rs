pub use prelude::*;
mod app;
mod models;
mod okx;
pub mod prelude;
mod server;
mod ui;
mod utils;
mod ws;
const REFRESH_CYCLES: u64 = 950;
const NOTIFY_SECS: i64 = 1800;
const ORDER_CHECK_DELAY_SECS: i64 = 3;
const UI_LOG_LINES: usize = 8;

pub const BASE_URL: &str = "https://www.okx.com";
pub const ORDERS_ENDPOINT: &str = "/api/v5/trade/order";
pub const BALANCE_ENDPOINT: &str = "/api/v5/account/balance";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut cfg: AppConfig = AppConfig::load()?;
    //hash and save the strategy to the DB
    cfg.strategy.hash = cfg.strategy.get_hash();

    let mut app = App::init(&cfg).await?;

    //setup account balance and spendable per token
    let mut account = Account::new().set_balance(cfg.account.balance, cfg.account.spendable);
    account.authentication = cfg.exchange.clone().unwrap_or_default().authentication;
    //cfg.strategy.sane_defaults();
    app.set_cooldown(cfg.strategy.cooldown);
    if cfg.ui.enable {
        app.term.hide_cursor()?;
    }
    app.save_strategy(&cfg.strategy).await?;

    if cfg.strategy.quickstart {
        app.set_cooldown(1);
    };
    let server = if cfg.server.clone().unwrap_or_default().enable {
        Some(server::WebSocket::run("0.0.0.0:9002").await)
    } else {
        None
    };

    // Websocket message channel
    let (sender, receiver) = tokio::sync::mpsc::channel(100);
    if let Some(server) = server {
        tokio::spawn(async {
            ws::transmit(server, receiver).await.unwrap();
        });
    }

    loop {
        if cfg.ui.enable {
            app.term.move_cursor_to(0, 0)?;
        }
        app.time.utc = Utc::now();
        let unix_timestamp = app.time.utc.timestamp();
        app.time.now = time::Instant::now();

        // Prepare the data to send
        let ws_data = ws::WsData {
            account: account.clone(),
            tokens: app.tokens.clone(),
            ts: app.time.utc,
        };

        // Send the data
        if let Err(_) = sender.send(ws_data).await {
            app.logs
                .push("Error sending data to transmit function".to_string());
        };

        //Retrieve and process top tokens
        app.fetch_tokens(cfg.strategy.timeframe).await;
        app.tokens = app
            .update_candles(cfg.strategy.timeframe, app.tokens.clone())
            .await
            .unwrap();
        app.filter_invalid(&cfg.strategy, account.balance.spendable);
        app.clean_top(cfg.strategy.top).get_tickers().await;

        //update timers in portfolio tokens
        account = app.buy_tokens(account, &cfg.strategy).await?;
        //Portoflio
        app.update_cooldown(&account.portfolio);

        account.portfolio = app.update_timeouts(account.portfolio, &cfg.strategy);
        account.portfolio = app
            .update_candles(cfg.strategy.timeframe, account.portfolio)
            .await?;

        //update reports
        account.portfolio = app.update_reports(account.portfolio, cfg.strategy.timeout);

        //if unix_timestamp.rem_euclid(ORDER_CHECK_DELAY_SECS) == 0 {
        //Check and update order states
        account.portfolio = app.update_order_states(account.portfolio).await?;
        //TODO: Cancel live orders if above order_ttl
        //account.portfolio = app.cancel_expired_orders(account.portfolio).await?;
        // }

        //keep tokens with balance > 0 usd
        //account.portfolio.retain(|t| t.balance.current > 0.0);

        //Selling and token removal validation occurs on every loop

        account.balance.set_current(0.0);
        account
            .calculate_balance(&mut app)
            .await
            .calculate_earnings();

        account = app.tag_invalid_tokens(account, &cfg.strategy)?;
        account = app.sell_tokens(account, &cfg.strategy).await?;
        account.clean_portfolio();

        // UI Display
        if cfg.ui.enable {
            if app.cycles.rem_euclid(REFRESH_CYCLES) == 0 {
                app.term.clear_screen()?;
            }
            if app.logs.len() > UI_LOG_LINES {
                app.logs.drain(..app.logs.len() - UI_LOG_LINES);
            };

            for line in ui::display(&cfg, &app, &account)?.iter() {
                app.term.write_line(&format!("{}", line))?
            }
        } else {
            for log in app.logs.iter() {
                log::info!("{}", log);
            }
            app.logs.clear();
        }

        app.time.uptime = app.time.uptime + app.time.elapsed;
        app.time.elapsed = Duration::milliseconds(app.time.now.elapsed().as_millis() as i64);

        app.set_cooldown(cfg.strategy.cooldown);

        //Send notifications
        if app.pushover.enable {
            app.send_notifications(&account).await?;
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
        }
        app.cycles += 1;
    }
}
