use crate::prelude::*;
use comfy_table::modifiers::UTF8_ROUND_CORNERS;
use comfy_table::presets::UTF8_FULL;
use comfy_table::*;

const TABLE_WIDTH: u16 = 300;
pub fn display(cfg: &AppConfig, app: &App, account: &Account) -> Result<Vec<Table>> {
    let mut tables = Vec::new();
    if cfg.ui.dashboard {
        //Create comfy table
        let mut table_instids = Table::new();
        let vol_header = format!("Vol ({}m)", cfg.strategy.timeframe);
        let change_header = format!("Change ({}m)", cfg.strategy.timeframe);
        let range_header = format!("Range ({}m)", cfg.strategy.timeframe);
        let sd_header = format!("Std Dev ({}m)", cfg.strategy.timeframe);
        table_instids
            .load_preset(UTF8_FULL)
            .apply_modifier(UTF8_ROUND_CORNERS)
            .set_content_arrangement(ContentArrangement::Disabled)
            .set_width(TABLE_WIDTH)
            .set_header(vec![
                "Symbol",
                "Candles",
                "LastCand",
                &sd_header,
                &change_header,
                &range_header,
                &vol_header,
                "Change (24h)",
                "Volume (24h)",
                " Status ",
            ]);
        //print token rows
        for t in app.tokens.iter() {
            let mut token_row: Vec<Cell> = vec![Cell::new(t.instid.replace("-USDT", ""))
                .set_alignment(CellAlignment::Center)
                .fg(Color::White)];

            let candle_list = show_candles(t.candlesticks.clone());
            token_row.push(
                Cell::new(candle_list.join("").to_string())
                    .set_delimiter('.')
                    .add_attribute(Attribute::Fraktur),
            );
            //Last candle
            let blank = Candlestick::new();
            let last = t.candlesticks.last().unwrap_or(&blank);
            if last.change < 0.00 {
                token_row.push(
                    Cell::new(format!("{:.2}%", last.change))
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold)
                        .fg(Color::Red),
                );
            } else {
                token_row.push(
                    Cell::new(format!("{:.2}%", last.change))
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold)
                        .fg(Color::Green),
                );
            };
            //standard deviation
            if t.std_deviation <= 0.00 {
                token_row.push(
                    Cell::new(format!("{:.2}%", t.std_deviation))
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold)
                        .fg(Color::Red),
                );
            } else {
                token_row.push(
                    Cell::new(format!("{:.2}%", t.std_deviation))
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold)
                        .fg(Color::Green),
                );
            };
            //change
            if t.change <= 0.00 {
                token_row.push(
                    Cell::new(format!("{:.2}%", t.change))
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold)
                        .fg(Color::Red),
                );
            } else {
                token_row.push(
                    Cell::new(format!("+{:.2}%", t.change))
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold)
                        .fg(Color::Green),
                );
            };

            //range
            token_row.push(
                Cell::new(format!("{:.2}%", t.range))
                    .set_alignment(CellAlignment::Center)
                    .add_attribute(Attribute::Bold)
                    .fg(Color::Cyan),
            );
            //vol
            token_row.push(Cell::new(format!("{:.0}", t.vol)).set_alignment(CellAlignment::Center));

            //change 24h
            if t.change24h <= 0.00 {
                token_row.push(
                    Cell::new(format!("{:.2}%", t.change24h))
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold)
                        .fg(Color::Red),
                );
            } else {
                token_row.push(
                    Cell::new(format!("+{:.2}%", t.change24h))
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold)
                        .fg(Color::Green),
                );
            };
            //vol 24h
            token_row
                .push(Cell::new(format!("{:.0}", t.vol24h)).set_alignment(CellAlignment::Center));

            //Status
            if let Some(token) = account.portfolio.iter().find(|s| t.instid == s.instid) {
                token_row.push(
                    Cell::new(format!("{:?}", token.status))
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold)
                        .fg(Color::White),
                );
            } else {
                //print cooldown
                if t.cooldown.num_seconds() == cfg.strategy.cooldown - 1 {
                    token_row.push(
                        Cell::new("Waiting".to_string())
                            .set_alignment(CellAlignment::Center)
                            .fg(Color::DarkGrey),
                    );
                } else if t.cooldown.num_seconds() > 10 {
                    token_row.push(
                        Cell::new(format!("{} s", t.cooldown.num_seconds()))
                            .set_alignment(CellAlignment::Center)
                            .fg(Color::White),
                    );
                } else {
                    token_row.push(
                        Cell::new(format!("{} s", t.cooldown.num_seconds()))
                            .set_alignment(CellAlignment::Center)
                            .fg(Color::Yellow),
                    );
                }
            }
            table_instids.add_row(token_row);
        }
        for _ in app.tokens.len()..cfg.strategy.top {
            let token_row: Vec<Cell> = Vec::new();
            table_instids.add_row(token_row);
        }
        tables.push(table_instids);
    }
    if cfg.ui.portfolio {
        let mut table_instids = Table::new();
        table_instids
            .load_preset(UTF8_FULL)
            .apply_modifier(UTF8_ROUND_CORNERS)
            .set_content_arrangement(ContentArrangement::Disabled)
            .set_width(TABLE_WIDTH)
            .set_header(vec![
                "Symbol",
                "Candles",
                "LastCand",
                " Balance ",
                "Change",
                "Earnings",
                "Timeout",
                "[B] OrderID",
                "[B] OrderState",
                "[S] OrderID",
                "[S] OrderState",
                " Status ",
            ]);
        //print token rows
        for t in account.portfolio.iter() {
            let mut token_row: Vec<Cell> = Vec::new();

            //instid
            token_row.push(
                Cell::new(t.instid.replace("-USDT", ""))
                    .set_alignment(CellAlignment::Center)
                    .fg(Color::White),
            );

            let candle_list = show_candles(t.candlesticks.clone());

            token_row.push(
                Cell::new(candle_list.join("").to_string())
                    .set_delimiter('.')
                    .add_attribute(Attribute::Fraktur),
            );
            //Last candle
            let blank = Candlestick::new();
            let last = t.candlesticks.last().unwrap_or(&blank);
            if last.change < 0.00 {
                token_row.push(
                    Cell::new(format!("{:.2}%", last.change))
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold)
                        .fg(Color::Red),
                );
            } else {
                token_row.push(
                    Cell::new(format!("{:.2}%", last.change))
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold)
                        .fg(Color::Green),
                );
            };

            //Avail Balance

            token_row.push({
                let formatted_value = if t.balance.available == 0.0 {
                    "---".to_string()
                } else if t.balance.available > 0.0 && t.balance.available < 10.0 {
                    format!("{:.6}", t.balance.available)
                } else if t.balance.available > 10.0 && t.balance.available < 100.0 {
                    format!("{:.2}", t.balance.available)
                } else {
                    format!("{:.0}", t.balance.available)
                };

                Cell::new(formatted_value)
                    .set_alignment(CellAlignment::Center)
                    .add_attribute(Attribute::Bold)
            });

            //change
            if t.change < 0.00 {
                token_row.push(
                    Cell::new(format!("{:.2}%", t.change))
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold)
                        .fg(Color::Red),
                );
            } else if t.change > 0.00 {
                token_row.push(
                    Cell::new(format!("{:.2}%", t.change))
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold)
                        .fg(Color::Green),
                );
            } else {
                token_row.push(
                    Cell::new(format!("{:.2}%", t.change))
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold)
                        .fg(Color::DarkGrey),
                );
            }
            //Earnings
            if t.balance.available > 0.0 {
                let earnings = (t.balance.current * t.price) - (t.buy_price * t.balance.start);
                if earnings < 0.0 {
                    token_row.push(
                        Cell::new(format!("$ {:.2}", earnings))
                            .set_alignment(CellAlignment::Center)
                            .add_attribute(Attribute::Bold)
                            .fg(Color::Red),
                    );
                } else if earnings > 0.0 {
                    token_row.push(
                        Cell::new(format!("$ {:.2}", earnings))
                            .set_alignment(CellAlignment::Center)
                            .add_attribute(Attribute::Bold)
                            .fg(Color::Green),
                    );
                } else {
                    token_row.push(
                        Cell::new(format!("$ {:.2}", t.earnings))
                            .set_alignment(CellAlignment::Center)
                            .add_attribute(Attribute::Bold)
                            .fg(Color::DarkGrey),
                    );
                }
            } else {
                token_row.push(
                    Cell::new("---".to_string())
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold)
                        .fg(Color::DarkGrey),
                );
            }

            //timeout
            if t.timeout.num_seconds() == t.config.timeout.num_seconds() - 1 {
                token_row.push(
                    Cell::new("---".to_string())
                        .set_alignment(CellAlignment::Center)
                        .fg(Color::DarkGrey),
                );
            } else if t.timeout.num_seconds() > 10 {
                token_row.push(
                    Cell::new(format!("{} s", t.timeout.num_seconds()))
                        .set_alignment(CellAlignment::Center)
                        .fg(Color::White),
                );
            } else {
                token_row.push(
                    Cell::new(format!("{} s", t.timeout.num_seconds()))
                        .set_alignment(CellAlignment::Center)
                        .fg(Color::Yellow),
                );
            }
            let trade_enabled = app.exchange.enable_trading;
            //Buy Order IDs
            let order_ids: Vec<String> = t
                .orders
                .as_ref()
                .unwrap_or(&Vec::new())
                .iter()
                .filter(|o| o.side == Side::Buy)
                .map(|o| {
                    format!(
                        "{:.8}..",
                        if trade_enabled {
                            o.ord_id.clone()
                        } else {
                            o.cl_ord_id.clone()
                        }
                    )
                })
                .collect();

            token_row.push(
                Cell::new(order_ids.last().unwrap_or(&String::from("---")))
                    .set_alignment(CellAlignment::Center)
                    .add_attribute(Attribute::Bold),
            );
            //Buy Order States
            let order_states: Vec<String> = t
                .orders
                .as_ref()
                .unwrap_or(&Vec::new())
                .iter()
                .filter(|o| o.side == Side::Buy)
                .map(|o| format!("{}", o.state.to_string()))
                .collect();

            token_row.push(
                Cell::new(order_states.last().unwrap_or(&String::from("---")))
                    .set_alignment(CellAlignment::Center)
                    .add_attribute(Attribute::Bold),
            );
            //Sell Order IDs
            let order_ids: Vec<String> = t
                .orders
                .as_ref()
                .unwrap_or(&Vec::new())
                .iter()
                .filter(|o| o.side == Side::Sell)
                .map(|o| {
                    format!(
                        "{:.8}..",
                        if trade_enabled {
                            o.ord_id.clone()
                        } else {
                            o.cl_ord_id.clone()
                        }
                    )
                })
                .collect();

            token_row.push(
                Cell::new(order_ids.last().unwrap_or(&String::from("---")))
                    .set_alignment(CellAlignment::Center)
                    .add_attribute(Attribute::Bold),
            );
            //Sell Order States
            let order_states: Vec<String> = t
                .orders
                .as_ref()
                .unwrap_or(&Vec::new())
                .iter()
                .filter(|o| o.side == Side::Sell)
                .map(|o| format!("{}", o.state.to_string()))
                .collect();

            token_row.push(
                Cell::new(order_states.last().unwrap_or(&String::from("---")))
                    .set_alignment(CellAlignment::Center)
                    .add_attribute(Attribute::Bold),
            );

            //status
            match t.status {
                token::Status::Trading => token_row.push(
                    Cell::new(format!("{:?}", t.status))
                        .set_alignment(CellAlignment::Center)
                        .fg(Color::White),
                ),
                token::Status::Selling => token_row.push(
                    Cell::new(format!("{:?}", t.status))
                        .set_alignment(CellAlignment::Center)
                        .fg(Color::DarkYellow),
                ),
                token::Status::Buying => token_row.push(
                    Cell::new(format!("{:?}", t.status))
                        .set_alignment(CellAlignment::Center)
                        .fg(Color::DarkBlue),
                ),
                token::Status::Waiting => token_row.push(
                    Cell::new(format!("{:?}", t.status))
                        .set_alignment(CellAlignment::Center)
                        .fg(Color::DarkGrey),
                ),
            }

            table_instids.add_row(token_row);
        }

        for _ in account.portfolio.len()..cfg.strategy.portfolio_size as usize {
            let token_row: Vec<Cell> = Vec::new();
            table_instids.add_row(token_row);
        }
        tables.push(table_instids);
    }
    if cfg.ui.balance {
        let mut table_account = Table::new();
        table_account
            .load_preset(UTF8_FULL)
            .apply_modifier(UTF8_ROUND_CORNERS)
            .set_content_arrangement(ContentArrangement::Disabled)
            .set_width(TABLE_WIDTH)
            .set_header(vec![
                "Change",
                "Balance",
                "Available",
                "Earnings",
                "Fee Spend",
                "Spendable",
                "Strategy",
            ]);
        let mut token_row: Vec<Cell> = Vec::new();

        //change
        if account.change < 0.00 {
            token_row.push(
                Cell::new(format!("{:.2}%", account.change))
                    .set_alignment(CellAlignment::Center)
                    .add_attribute(Attribute::Bold)
                    .fg(Color::Red),
            );
        } else if account.change > 0.0 {
            token_row.push(
                Cell::new(format!("{:.2}%", account.change))
                    .set_alignment(CellAlignment::Center)
                    .add_attribute(Attribute::Bold)
                    .fg(Color::Green),
            );
        } else {
            token_row.push(
                Cell::new(format!("{:.2}%", account.change))
                    .set_alignment(CellAlignment::Center)
                    .add_attribute(Attribute::Bold)
                    .fg(Color::DarkGrey),
            );
        }
        //balance current
        token_row.push(
            Cell::new(format!("$ {:.2}", account.balance.current))
                .set_alignment(CellAlignment::Center)
                .fg(Color::White),
        );
        //Available
        if account.balance.available < account.balance.spendable {
            token_row.push(
                Cell::new(format!("$ {:.2}", &account.balance.available))
                    .fg(Color::Red)
                    .set_alignment(CellAlignment::Center),
            );
        } else {
            token_row.push(
                Cell::new(format!("$ {:.2}", &account.balance.available))
                    .set_alignment(CellAlignment::Center),
            );
        }
        //earnings
        token_row.push(
            Cell::new(format!("$ {:.2}", account.earnings))
                .set_alignment(CellAlignment::Center)
                .fg(Color::DarkGrey),
        );

        //Fee Spend
        token_row.push(
            Cell::new(format!("$ {:.2}", account.fee_spend)).set_alignment(CellAlignment::Center),
        );

        //Spendable per trade
        token_row.push(
            Cell::new(format!("$ {:.2}", account.balance.spendable))
                .set_alignment(CellAlignment::Center),
        );
        //Strategy Hash
        token_row.push(
            Cell::new(&cfg.strategy.hash)
                .set_alignment(CellAlignment::Center)
                .fg(Color::DarkGrey),
        );

        table_account.add_row(token_row);

        tables.push(table_account);
    }
    if cfg.ui.strategy {
        let mut table_config = Table::new();
        table_config
            .load_preset(UTF8_FULL)
            .apply_modifier(UTF8_ROUND_CORNERS)
            .set_content_arrangement(ContentArrangement::Disabled)
            .set_width(TABLE_WIDTH)
            .set_header(vec![
                "Timeframe",
                "Cooldown",
                "Min Vol",
                "Min Change",
                "Found",
                "Cashout",
                "Stop Loss",
                "Sell Floor",
                "Trades",
                "Round",
            ]);
        let mut row: Vec<Cell> = Vec::new();
        row.push(
            Cell::new(format!("{} minutes", cfg.strategy.timeframe))
                .set_alignment(CellAlignment::Center),
        );
        row.push(
            Cell::new(format!("{} secs", cfg.strategy.cooldown))
                .set_alignment(CellAlignment::Center),
        );
        row.push(
            Cell::new(format!("$ {}", cfg.strategy.min_vol.unwrap()))
                .set_alignment(CellAlignment::Center),
        );
        row.push(
            Cell::new(format!("{:.2} %", cfg.strategy.min_change))
                .set_alignment(CellAlignment::Center),
        );
        row.push(Cell::new(format!("{}", app.tokens.len())).set_alignment(CellAlignment::Center));

        row.push(
            Cell::new(format!("{:.2} %", cfg.strategy.cashout))
                .set_alignment(CellAlignment::Center),
        );
        row.push(
            Cell::new(format!("{:.2} %", (-cfg.strategy.stoploss)))
                .set_alignment(CellAlignment::Center),
        );
        row.push(
            Cell::new(if let Some(x) = cfg.strategy.sell_floor {
                format!("{:.2} %", x)
            } else {
                String::from("NotSet")
            })
            .set_alignment(CellAlignment::Center),
        );
        // Trade count
        row.push(
            Cell::new(format!("{}", account.trades))
                .set_alignment(CellAlignment::Center)
                .fg(Color::White),
        );
        // Round id
        row.push(
            Cell::new(format!("{}", app.round_id))
                .set_alignment(CellAlignment::Center)
                .fg(Color::White),
        );

        table_config.add_row(row);
        tables.push(table_config);
    }
    if cfg.ui.system {
        let mut table_time = Table::new();
        table_time
            .load_preset(UTF8_FULL)
            .apply_modifier(UTF8_ROUND_CORNERS)
            .set_content_arrangement(ContentArrangement::Disabled)
            .set_width(TABLE_WIDTH)
            .set_header(vec!["Started", "Date", "Uptime", "Cycles", "Latency"]);
        let mut row: Vec<Cell> = Vec::new();
        row.push(Cell::new(format!("{}", app.time.started)).set_alignment(CellAlignment::Center));
        row.push(Cell::new(format!("{}", app.time.utc)).set_alignment(CellAlignment::Center));
        row.push(
            Cell::new(format!("{} m", app.time.uptime.num_minutes()))
                .set_alignment(CellAlignment::Center),
        );
        row.push(Cell::new(format!("{}", app.cycles)).set_alignment(CellAlignment::Center));

        row.push(
            Cell::new(format!("{} ms", app.time.now.elapsed().as_millis()))
                .set_alignment(CellAlignment::Center),
        );

        table_time.add_row(row);
        tables.push(table_time);
    }
    if cfg.ui.deny_list {
        let mut table_denied = Table::new();
        table_denied
            .load_preset(UTF8_FULL)
            .apply_modifier(UTF8_ROUND_CORNERS)
            //.set_width(TABLE_WIDTH)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_width(120)
            .set_header(vec!["Deny list"]);
        let mut row: Vec<Cell> = Vec::new();
        row.push(Cell::new(format!("{:?}", app.deny_list)).set_alignment(CellAlignment::Center));
        table_denied.add_row(row);
        tables.push(table_denied);
    }
    if cfg.ui.logs {
        let mut table_logs = Table::new();
        if !app.logs.is_empty() {
            table_logs
                .load_preset(UTF8_FULL)
                .apply_modifier(UTF8_ROUND_CORNERS)
                //.set_width(TABLE_WIDTH)
                .set_content_arrangement(ContentArrangement::Dynamic)
                .set_width(400)
                .set_header(vec!["Logs"]);
            for log in app.logs.iter() {
                let row: Vec<Cell> = vec![Cell::new(log).set_alignment(CellAlignment::Left)];
                table_logs.add_row(row);
            }
            tables.push(table_logs);
        }
    };
    Ok(tables)
}

fn show_candles(candles: Vec<Candlestick>) -> Vec<String> {
    //Candles
    let mut candle_list: Vec<String> = Vec::new();
    for c in candles.iter() {
        if c.change > 0.01 {
            candle_list.push("▀".to_string())
        } else if c.change < -0.01 {
            candle_list.push("▄".to_string())
        } else if c.change == 0.0 {
            candle_list.push("-".to_string())
        }
    }
    candle_list
}
