use std::collections::HashMap;

use charts::{BalanceChart, CandlestickBoxPlot, ChangeChart, EarningsChart};
use chrono::{DateTime, Duration, Utc};
use eframe::egui::{
    menu,
    plot::{self, Corner, Legend, Line, Plot},
    CentralPanel, CollapsingHeader, Color32, Context, Frame, Key, RichText, ScrollArea, TextEdit,
    TextStyle, TopBottomPanel,
};
use ewebsock::{WsEvent, WsMessage, WsReceiver, WsSender};
use models::*;
use serde::Deserialize;
mod charts;
mod models;
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
            },
            Err(error) => {
                log::error!("Failed to connect to {:?}: {}", &self.url, error);
                self.error = error;
            },
        }
    }
}

// ----------------------------------------------------------------------------

struct FrontEnd {
    ws_sender: WsSender,
    ws_receiver: WsReceiver,
    text_to_send: String,
    latest_event_per_channel: HashMap<String, WsEvent>,
    account_history: Vec<Account>,
    timestamps: Vec<i64>,
}

impl FrontEnd {
    fn new(ws_sender: WsSender, ws_receiver: WsReceiver) -> Self {
        Self {
            ws_sender,
            ws_receiver,
            text_to_send: Default::default(),
            latest_event_per_channel: Default::default(),
            account_history: Vec::new(),
            timestamps: Vec::new(),
        }
    }

    fn ui(&mut self, ctx: &Context) {
        while let Some(event) = self.ws_receiver.try_recv() {
            if let WsEvent::Message(WsMessage::Text(text)) = &event {
                let data = serde_json::from_str::<TextMsg>(text).unwrap();

                // Store the latest event per channel
                self.latest_event_per_channel
                    .insert(data.channel.clone(), event.clone());
            }
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

            let mut channels: Vec<String> = self.latest_event_per_channel.keys().cloned().collect();
            channels.sort();

            let max_width = ui.available_width();
            for channel in channels {
                let event = self.latest_event_per_channel.get(&channel).unwrap();
                let msg = match event {
                    WsEvent::Message(WsMessage::Text(text)) => {
                        serde_json::from_str::<TextMsg>(text).unwrap()
                    },
                    _ => continue,
                };
                let legend = Legend {
                    text_style: TextStyle::Monospace,
                    position: Corner::LeftBottom,
                    background_alpha: 0.5,
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

                    ui.horizontal_wrapped(|ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.heading(RichText::new("Balances:").color(Color32::DARK_GRAY));

                                ui.add_space(3.0);
                                ui.label(format!("Current: {:.2}", latest_account.balance.current));
                                ui.label(" | ");
                                ui.add_space(3.0);
                                ui.label(format!(
                                    "Available: {:.2}",
                                    latest_account.balance.available
                                ));
                            });
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
                                            .name("Token balance"),
                                    );
                                    plot.line(
                                        Line::new(balance.lines_values[2].clone())
                                            .name("Open orders balance"),
                                    );
                                    plot.line(
                                        Line::new(balance.lines_values[3].clone())
                                            .name("Available balance"),
                                    );
                                });
                        });
                        ui.add_space(10.0);

                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.heading(RichText::new("Change:").color(Color32::DARK_GRAY));
                                ui.add_space(3.0);
                                ui.heading(
                                    RichText::new(format!("% {:.2}", latest_account.change))
                                        .color(get_change_color(latest_account.change)),
                                )
                            });
                            Plot::new("change")
                                .legend(legend.clone())
                                .view_aspect(100.0) // adjust this to your needs
                                .width(max_width / 3.0 - 20.0)
                                .height(200.0)
                                .show(ui, |plot| {
                                    plot.line(
                                        Line::new(change.lines_values[0].clone())
                                            .color(get_change_color(latest_account.change))
                                            .name("Change"),
                                    );
                                });
                        });
                        ui.add_space(10.0);
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.heading(RichText::new("Earnings:").color(Color32::DARK_GRAY));
                                ui.add_space(3.0);
                                ui.heading(
                                    RichText::new(format!("{:.2}", latest_account.earnings))
                                        .color(get_change_color(latest_account.earnings)),
                                )
                            });
                            Plot::new("earnings")
                                .legend(legend.clone())
                                .view_aspect(100.0) // adjust this to your needs
                                .width(max_width / 3.0 - 20.0)
                                .height(200.0)
                                .show(ui, |plot| {
                                    plot.line(
                                        Line::new(earnings.lines_values[0].clone())
                                            .color(get_change_color(latest_account.change))
                                            .name("Earnings"),
                                    );
                                    plot.line(
                                        Line::new(earnings.lines_values[1].clone())
                                            .color(Color32::LIGHT_BLUE)
                                            .name("Fees"),
                                    );
                                });
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
                                let box_plot = CandlestickBoxPlot::new(
                                    &token.candlesticks,
                                    token.buy_ts,
                                    token.buy_price,
                                );
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.heading(RichText::new(format!(
                                            "{} [{}]",
                                            &token.instid, token.status
                                        )));
                                        ui.add_space(3.0);
                                        ui.heading(RichText::new("| Change:"));
                                        ui.add_space(0.2);

                                        ui.heading(
                                            RichText::new(format!("% {:.2}", token.change))
                                                .color(get_change_color(token.change as f64)),
                                        );
                                        ui.add_space(3.0);
                                        //ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                                        if token.config.timeout == token.timeout {
                                            ui.add_space(10.0);
                                            ui.heading(
                                                RichText::new("Live")
                                                    .color(Color32::LIGHT_BLUE)
                                                    .strong(),
                                            );
                                        } else if let Some(reason) = &token.exit_reason {
                                            ui.heading(
                                                RichText::new(format!("{:.2}", reason))
                                                    .color(Color32::DARK_BLUE),
                                            );
                                        } else {
                                            ui.heading(RichText::new("| Timeout:"));
                                            ui.add_space(0.2);
                                            ui.heading(
                                                RichText::new(format!(
                                                    "{:.2}",
                                                    token.timeout.num_seconds()
                                                ))
                                                .color(get_time_color(token.timeout.num_seconds())),
                                            );
                                        }
                                    });
                                    Frame::dark_canvas(ui.style()).show(ui, |ui| {
                                        plot::Plot::new(token.instid.clone())
                                            .allow_drag(true)
                                            .allow_scroll(true)
                                            .legend(legend.clone())
                                            .allow_zoom(true)
                                            .show_y(false)
                                            .width(max_width / 3.0 - 20.0)
                                            .height(200.0)
                                            .show(ui, |plot_ui| {
                                                plot_ui
                                                    .box_plot(plot::BoxPlot::new(box_plot.boxes));
                                            });
                                    });
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(format!("Price: {:.5}", token.price));
                                        ui.label(format!(
                                            "Available Bal.: {:.4}",
                                            token.balance.available
                                        ));
                                        ui.label(format!(
                                            "Current Bal.: {:.4}",
                                            token.balance.current
                                        ));
                                        ui.label(format!("SD.: {:.4}", token.std_deviation));
                                    });
                                });
                                ui.add_space(10.0);
                            }
                            ui.add_space(10.0);
                        });
                    }
                };
                CollapsingHeader::new(format!("{} events", channel)).show(ui, |ui| {
                    ScrollArea::vertical().show(ui, |ui| {
                        if let WsEvent::Message(WsMessage::Text(text)) = event {
                            let mut data = serde_json::from_str::<TextMsg>(text).unwrap();
                            let text_edit = TextEdit::multiline(&mut data.data);
                            ui.horizontal(|ui| {
                                ui.add(text_edit);
                            });
                        }
                    });
                });
            }
        });
    }
}

fn get_change_color(v: f64) -> Color32 {
    match v {
        _ if v > 0.0 => Color32::GREEN,
        _ if v < 0.0 => Color32::RED,
        _ => Color32::DARK_GRAY,
    }
}

fn get_time_color(v: i64) -> Color32 {
    match v {
        _ if v > 10 => Color32::LIGHT_BLUE,
        _ => Color32::YELLOW,
    }
}
