use charts::{BalanceChart, CandlestickBoxPlot, ChangeChart, EarningsChart};
use chrono::{DateTime, Duration, Utc};
use eframe::egui::{
    menu,
    plot::{self, Corner, Legend, Line, Plot},
    CentralPanel, CollapsingHeader, Color32, Context, Frame, Key, ScrollArea, TextStyle,
    TopBottomPanel,
};
use ewebsock::{WsEvent, WsMessage, WsReceiver, WsSender};
use serde::Deserialize;
use std::collections::HashMap;
mod charts;
#[derive(Default)]
pub struct Console {
    pub url: String,
    error: String,
    frontend: Option<FrontEnd>,
}

#[derive(Deserialize)]
pub struct TextMsg {
    channel: String,
    data: String,
    ts: DateTime<Utc>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Account {
    balance: Balance,
    earnings: f64,
    change: f64,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Balance {
    pub start: f64,
    pub current: f64,
    pub available: f64,
    pub spendable: f64,
}

#[serde_with::serde_as]
#[derive(Deserialize, Debug, Clone)]
pub struct Token {
    pub round_id: u64,
    pub instid: String,
    pub buy_price: f64,
    #[serde(rename = "px")]
    pub price: f64,
    pub change: f32,
    pub std_deviation: f32,
    #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    pub timeout: Duration,
    pub balance: Balance,
    pub earnings: f64,
    pub fees_deducted: bool,
    pub vol: f64,
    pub vol24h: f64,
    pub change24h: f32,
    pub range: f32,
    pub range24h: f32,
    #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    pub cooldown: Duration,
    pub candlesticks: Vec<Candlestick>,
    pub status: String,
    //pub orders: Option<String>,
    pub exit_reason: Option<String>,
}

#[serde_with::serde_as]
#[derive(Deserialize, Debug, Clone)]
pub struct Candlestick {
    pub instid: String,
    #[serde_as(as = "serde_with::DurationMilliSeconds<i64>")]
    pub ts: Duration,
    pub change: f32,
    pub close: f64,
    pub high: f64,
    pub low: f64,
    pub open: f64,
    pub range: f32,
    pub vol: f64,
}

impl eframe::App for Console {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            TopBottomPanel::top("top_panel").show(ctx, |ui| {
                menu::bar(ui, |ui| {
                    ui.menu_button("File", |ui| {
                        if ui.button("Quit").clicked() {
                            _frame.close();
                        }
                    });
                });
            });
        }

        TopBottomPanel::top("server").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("URL:");
                if ui.text_edit_singleline(&mut self.url).lost_focus()
                    && ui.input(|i| i.key_pressed(Key::Enter))
                {
                    self.connect(ctx.clone());
                }
            });
        });

        if !self.error.is_empty() {
            TopBottomPanel::top("error").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Error:");
                    ui.colored_label(Color32::RED, &self.error);
                });
            });
        }

        if let Some(frontend) = &mut self.frontend {
            frontend.ui(ctx);
        }
    }
}

impl Console {
    fn connect(&mut self, ctx: Context) {
        let wakeup = move || ctx.request_repaint(); // wake up UI thread on new message
        match ewebsock::connect_with_wakeup(&self.url, wakeup) {
            Ok((ws_sender, ws_receiver)) => {
                self.frontend = Some(FrontEnd::new(ws_sender, ws_receiver));
                self.error.clear();
            }
            Err(error) => {
                log::error!("Failed to connect to {:?}: {}", &self.url, error);
                self.error = error;
            }
        }
    }
}

// ----------------------------------------------------------------------------

struct FrontEnd {
    ws_sender: WsSender,
    ws_receiver: WsReceiver,
    text_to_send: String,
    latest_event_per_channel: HashMap<String, WsEvent>,
    events_count_per_second: HashMap<String, u32>,
    new_events_count_per_second: HashMap<String, u32>,
    account_history: Vec<Account>,
    timestamps: Vec<i64>,
    last_update: std::time::Instant,
}

impl FrontEnd {
    fn new(ws_sender: WsSender, ws_receiver: WsReceiver) -> Self {
        Self {
            ws_sender,
            ws_receiver,
            text_to_send: Default::default(),
            latest_event_per_channel: Default::default(),
            events_count_per_second: Default::default(),
            new_events_count_per_second: Default::default(),
            account_history: Vec::new(),
            timestamps: Vec::new(),
            last_update: std::time::Instant::now(),
        }
    }

    fn ui(&mut self, ctx: &Context) {
        while let Some(event) = self.ws_receiver.try_recv() {
            if let WsEvent::Message(WsMessage::Text(text)) = &event {
                let data = serde_json::from_str::<TextMsg>(text).unwrap();

                // Update the events per second count
                *self
                    .new_events_count_per_second
                    .entry(data.channel.clone())
                    .or_insert(0) += 1;

                // Store the latest event per channel
                self.latest_event_per_channel
                    .insert(data.channel.clone(), event.clone());
            }
        }

        // Every second, update the real events per second count
        let now = std::time::Instant::now();
        if self.last_update.elapsed().as_secs() >= 1 {
            self.events_count_per_second = self.new_events_count_per_second.clone();
            self.new_events_count_per_second.clear();
            self.last_update = now;
        }

        CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Message to send:");
                if ui.text_edit_singleline(&mut self.text_to_send).lost_focus()
                    && ui.input(|i| i.key_pressed(Key::Enter))
                {
                    self.ws_sender
                        .send(WsMessage::Text(std::mem::take(&mut self.text_to_send)));
                }
            });

            ui.separator();

            //ScrollArea::vertical().show(ui, |ui| {
            let mut channels: Vec<String> = self.latest_event_per_channel.keys().cloned().collect();
            channels.sort(); // Sort the keys alphabetically

            let max_width = ui.available_width();
            for channel in channels {
                let event = self.latest_event_per_channel.get(&channel).unwrap();
                let events_per_second = self.events_count_per_second.get(&channel).unwrap_or(&0);
                let msg = match event {
                    WsEvent::Message(WsMessage::Text(text)) => {
                        serde_json::from_str::<TextMsg>(text).unwrap()
                    }
                    _ => continue,
                };
                if channel == *"account" {
                    let account = match serde_json::from_str::<Account>(&msg.data) {
                        Ok(b) => b,
                        Err(e) => panic!("Unable to parse: {}\n{}", msg.data, e),
                    };

                    self.account_history.push(account);
                    self.timestamps.push(msg.ts.timestamp());
                    // Create a new line chart every time there's a new data point.
                    let balance = BalanceChart::new(&self.account_history, &self.timestamps);
                    let change = ChangeChart::new(&self.account_history, &self.timestamps);
                    let earnings = EarningsChart::new(&self.account_history, &self.timestamps);
                    let latest_account = self.account_history.last().unwrap();
                    let legend = Legend {
                        text_style: TextStyle::Monospace,
                        position: Corner::LeftBottom,
                        background_alpha: 0.5,
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.vertical(|ui| {
                            Plot::new("balance")
                                .legend(legend.clone())
                                .view_aspect(100.0) // adjust this to your needs
                                .width(max_width / 3.0 - 20.0)
                                .height(200.0)
                                .show(ui, |plot| {
                                    plot.line(
                                        Line::new(balance.lines_values[0].clone())
                                            .name("Current balance"),
                                    );
                                    plot.line(
                                        Line::new(balance.lines_values[1].clone())
                                            .name("Available balance"),
                                    );
                                });
                            ui.horizontal(|ui| {
                                // Add labels for current, available and spendable balance
                                ui.label(format!(
                                    "Current balance: {:.2}",
                                    latest_account.balance.current
                                ));
                                ui.label(format!(
                                    "Available balance: {:.2}",
                                    latest_account.balance.available
                                ));
                            });
                        });
                        ui.add_space(10.0);

                        ui.vertical(|ui| {
                            let change_color = if latest_account.change >= 0.0 {
                                Color32::GREEN
                            } else {
                                Color32::RED
                            };
                            Plot::new("change")
                                .legend(legend.clone())
                                .view_aspect(100.0) // adjust this to your needs
                                .width(max_width / 3.0 - 20.0)
                                .height(200.0)
                                .show(ui, |plot| {
                                    plot.line(
                                        Line::new(change.lines_values[0].clone())
                                            .color(change_color)
                                            .name("Change"),
                                    );
                                });

                            ui.colored_label(
                                change_color,
                                format!("Change: {:.2}", latest_account.change),
                            );
                        });
                        ui.add_space(10.0);
                        ui.vertical(|ui| {
                            let earnings_color = if latest_account.earnings >= 0.0 {
                                Color32::GREEN
                            } else {
                                Color32::RED
                            };
                            Plot::new("earnings")
                                .legend(legend.clone())
                                .view_aspect(100.0) // adjust this to your needs
                                .width(max_width / 3.0 - 20.0)
                                .height(200.0)
                                .show(ui, |plot| {
                                    plot.line(
                                        Line::new(earnings.lines_values[0].clone())
                                            .color(earnings_color)
                                            .name("Earnings"),
                                    );
                                });
                            ui.colored_label(
                                earnings_color,
                                format!("Earnings: {:.2}", latest_account.earnings),
                            );
                        });
                    });
                }
                if channel == *"portfolio" {
                    let tokens = match serde_json::from_str::<Vec<Token>>(&msg.data) {
                        Ok(t) => t,
                        Err(e) => panic!("Unable to parse: {}\n{}", msg.data, e),
                    };

                    let chunked_tokens = tokens.chunks(3); // Split the tokens into chunks of 3

                    for token_chunk in chunked_tokens {
                        ui.horizontal(|ui| {
                            // New horizontal group for each chunk
                            for token in token_chunk {
                                let box_plot = CandlestickBoxPlot::new(&token.candlesticks);

                                ui.vertical(|ui| {
                                    // Chart title
                                    //ui.heading(format!("{} | Price: {} | Cooldown: {} | {}", token.instid, token.price, token.cooldown, token.status));
                                    ui.heading(token.instid.to_string());

                                    // The chart itself
                                    Frame::dark_canvas(ui.style()).show(ui, |ui| {
                                        plot::Plot::new(token.instid.clone())
                                            .allow_drag(true)
                                            .allow_scroll(true)
                                            .allow_zoom(true)
                                            .show_y(false)
                                            .width(max_width / 3.0 - 20.0) // Fixed width adjusted to fit 3 in a row
                                            .height(200.0) // Fixed height
                                            .show(ui, |plot_ui| {
                                                plot_ui
                                                    .box_plot(plot::BoxPlot::new(box_plot.boxes));
                                            });
                                    });

                                    // Status details under the chart
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(format!("Price: {}", token.price));
                                        ui.label(format!("Cooldown: {}", token.cooldown));
                                        ui.label(format!("Timeout: {}", token.timeout));
                                        ui.label(format!("Status: {}", token.status));
                                    });
                                });
                                ui.add_space(10.0);
                            }
                        });
                    }
                };
                CollapsingHeader::new(format!("{} - {} events/s", channel, *events_per_second))
                    .show(ui, |ui| {
                        ScrollArea::vertical().show(ui, |ui| {
                            if let WsEvent::Message(WsMessage::Text(text)) = event {
                                let data = serde_json::from_str::<TextMsg>(text).unwrap();
                                ui.label(data.data);
                            }
                        });
                    });
            }
        });
    }
}
