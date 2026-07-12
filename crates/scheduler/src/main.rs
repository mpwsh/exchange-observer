pub use prelude::*;
use ws::{channel, server};
mod app;
mod models;
mod okx;
mod prelude;
mod ui;
mod utils;
mod ws;

const REFRESH_CYCLES: u64 = 950;
const NOTIFY_SECS: i64 = 1800;
const UI_LOG_LINES: usize = 8;

pub const BASE_URL: &str = "https://www.okx.com";
pub const ORDERS_ENDPOINT: &str = "/api/v5/trade/order";
pub const BALANCE_ENDPOINT: &str = "/api/v5/account/balance";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut cfg: AppConfig = AppConfig::load()?;
    //hash and save the strategy to the DB
    cfg.strategy.hash = cfg.strategy.get_hash();

    // Composition root: live adapters get injected here; tests and future
    // backtests substitute TestClock / other Strategy impls.
    let clock: Arc<dyn Clock> = Arc::new(LiveClock::default());
    let strategy = ThresholdStrategy;

    let mut app = App::init(&cfg, clock).await?;

    //setup account balance and spendable per token
    let mut account = Account::new().set_balance(cfg.account.balance, cfg.account.spendable);
    account.authentication = cfg.exchange.clone().unwrap_or_default().authentication;

    app.set_cooldown(cfg.strategy.cooldown);
    if cfg.ui.enable {
        app.term.hide_cursor()?;
    }
    app.save_strategy(&cfg.strategy).await?;

    if cfg.strategy.quickstart {
        app.set_cooldown(1);
    };
    let (sender, receiver) = tokio::sync::mpsc::channel(100);
    let ws_enabled = cfg.server.as_ref().is_some_and(|s| s.enable);

    if let Some(server) = &cfg.server {
        if server.enable {
            let address = format!("{}:{}", server.listen_address, server.port);
            let server = server::WebSocket::run(&address).await;

            tokio::spawn(async {
                channel::transmit(server, receiver).await.unwrap();
            });

            // Let the trade loop fire report events directly onto the same
            // channel — reports don't go through the per-cycle batch and
            // the console sees them as they close.
            app.report_tx = Some(sender.clone());
        }
    }

    let mut quickstart_completed = false;
    loop {
        if cfg.ui.enable {
            app.term.move_cursor_to(0, 0)?;
        }
        app.time.utc = app.clock.now_utc();
        let unix_timestamp = app.time.utc.timestamp();
        app.time.mono = app.clock.monotonic();

        //Retrieve and process top tokens
        app.fetch_tokens(cfg.strategy.timeframe).await?;
        app.tokens = app
            .update_candles(cfg.strategy.timeframe, app.tokens.clone())
            .await?;

        app.filter_invalid(&strategy, &cfg.strategy, &account.portfolio_view());
        app.clean_top(cfg.strategy.top).get_tickers().await?;

        //update timers in portfolio tokens
        account = app.buy_tokens(account, &strategy, &cfg.strategy).await?;

        //update portfolio and tracked tokens
        app.update_cooldowns(&account.portfolio);

        account.portfolio = app.update_timeouts(account.portfolio, &cfg.strategy);
        account.portfolio = app
            .update_candles(cfg.strategy.timeframe, account.portfolio)
            .await?;

        //update portfolio
        let portfolio_view = account.portfolio_view();
        for token in account.portfolio.iter_mut() {
            token
                .update_reports(cfg.strategy.timeout)
                .update_orders(app.exchange.enable_trading, &app.exchange.authentication)
                .await?;

            let still_listed = app.tokens.iter().any(|t| t.instid == token.instid);
            let ctx = Context {
                clock: app.clock.as_ref(),
                config: &cfg.strategy,
                portfolio: &portfolio_view,
            };
            let decision = strategy.should_exit(&ctx, &token.position_view(still_listed));
            token.apply_exit(decision);
        }

        account.balance.set_current(0.0);
        account
            .calculate_balance(&mut app)
            .await?
            .calculate_earnings();

        account = app.sell_tokens(account, &cfg.strategy).await?;
        account.clean_portfolio();

        // Websocket
        // Send state to the WS console — but only when the server (and thus
        // `channel::transmit`, the sole consumer of this bounded channel) is
        // running. With no consumer, the channel fills after 100 cycles and
        // the next `send().await` parks the scheduler loop forever.
        if ws_enabled {
            // Only send tokens that are actively trading
            let trading_tokens: Vec<Token> = account
                .portfolio
                .clone()
                .into_iter()
                .filter(|t| {
                    t.orders.as_ref().map_or(false, |orders| {
                        orders.iter().any(|order| order.state == OrderState::Filled)
                    })
                })
                .collect();

            let ws_data = ws::channel::Data::State(ws::channel::StateSnapshot {
                tokens: trading_tokens,
                balance: account.balance.clone(),
                fee_spend: account.fee_spend,
                earnings: account.earnings,
                change: account.change,
                ts: app.time.utc,
            });

            if (sender.send(ws_data).await).is_err() {
                app.logs
                    .push("Error sending data to transmit function".to_string());
            };
        }

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
        app.time.elapsed = Duration::milliseconds(
            app.clock
                .monotonic()
                .saturating_sub(app.time.mono)
                .as_millis() as i64,
        );

        if !quickstart_completed {
            app.set_cooldown(cfg.strategy.cooldown);
            quickstart_completed = true;
        }

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
