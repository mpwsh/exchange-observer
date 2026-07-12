//! Console UI: connects to the scheduler's websocket and renders a live
//! dashboard of account state + per-token trading progress.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use egui::{
    CentralPanel, CollapsingHeader, Color32, Context, Frame, Key, RichText, ScrollArea, TextEdit,
    TextStyle, TopBottomPanel, menu,
};
use egui_plot::{BoxPlot, Corner, Legend, Line, Plot, PlotPoints};
use ewebsock::{Options, WsEvent, WsMessage, WsReceiver, WsSender};
use serde::Deserialize;

use self::{
    charts::{BalanceChart, CandlestickBoxPlot, ChangeChart, EarningsChart},
    models::{Account, Token},
};

mod charts;
mod models;

/// Maximum number of account samples to keep in memory. At the default
/// ~1 sample/30s from the scheduler this is ~8 hours of history — plenty
/// for a debug view, and bounded so a long-running console doesn't OOM.
const MAX_HISTORY: usize = 1000;

// ---------------------------------------------------------------------------
// Top-level app
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct Console {
    pub url: String,
    error: String,
    frontend: Option<FrontEnd>,
}

impl eframe::App for Console {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        #[cfg(not(target_arch = "wasm32"))]
        self.draw_menu_bar(ctx);

        self.draw_connect_bar(ctx);
        self.draw_error_bar(ctx);

        if let Some(frontend) = &mut self.frontend {
            frontend.ui(ctx);
        }
    }
}

impl Console {
    #[cfg(not(target_arch = "wasm32"))]
    fn draw_menu_bar(&mut self, ctx: &Context) {
        TopBottomPanel::top("top_panel").show(ctx, |ui| {
            menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
        });
    }

    fn draw_connect_bar(&mut self, ctx: &Context) {
        TopBottomPanel::top("server").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("URL:");
                let resp = ui.text_edit_singleline(&mut self.url);
                if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    self.connect(ctx.clone());
                }
            });
        });
    }

    fn draw_error_bar(&self, ctx: &Context) {
        if self.error.is_empty() {
            return;
        }
        TopBottomPanel::top("error").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Error:");
                ui.colored_label(Color32::RED, &self.error);
            });
        });
    }

    fn connect(&mut self, ctx: Context) {
        let wakeup = move || ctx.request_repaint();
        match ewebsock::connect_with_wakeup(&self.url, Options::default(), wakeup) {
            Ok((sender, receiver)) => {
                self.frontend = Some(FrontEnd::new(sender, receiver));
                self.error.clear();
            },
            Err(err) => {
                log::error!("Failed to connect to {:?}: {err}", self.url);
                self.error = err.to_string();
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Frontend: everything that requires an active WS connection
// ---------------------------------------------------------------------------

/// Envelope: every scheduler → console message has this shape.
#[derive(Deserialize, Clone)]
struct TextMsg {
    channel: String,
    data: String,
    ts: DateTime<Utc>,
}

/// One decoded message, keyed by the `channel` field of the envelope.
#[derive(Clone)]
enum ParsedMsg {
    Account { account: Account, ts: DateTime<Utc> },
    Portfolio { tokens: Vec<Token> },
    Raw { channel: String, text: String },
}

impl ParsedMsg {
    fn from_envelope(envelope: TextMsg) -> Option<Self> {
        match envelope.channel.as_str() {
            "account" => match serde_json::from_str::<Account>(&envelope.data) {
                Ok(account) => Some(ParsedMsg::Account {
                    account,
                    ts: envelope.ts,
                }),
                Err(e) => {
                    log::warn!("Bad account payload: {e}");
                    None
                },
            },
            "portfolio" => match serde_json::from_str::<Vec<Token>>(&envelope.data) {
                Ok(tokens) => Some(ParsedMsg::Portfolio { tokens }),
                Err(e) => {
                    log::warn!("Bad portfolio payload: {e}");
                    None
                },
            },
            _ => Some(ParsedMsg::Raw {
                channel: envelope.channel,
                text: envelope.data,
            }),
        }
    }
}

struct FrontEnd {
    ws_sender: WsSender,
    ws_receiver: WsReceiver,
    text_to_send: String,
    /// Latest parsed message per channel, for rendering.
    latest_per_channel: HashMap<String, ParsedMsg>,
    /// Rolling window of account samples for the balance/change/earnings
    /// charts. Bounded at `MAX_HISTORY`.
    account_history: Vec<Account>,
    timestamps: Vec<i64>,
}

impl FrontEnd {
    fn new(ws_sender: WsSender, ws_receiver: WsReceiver) -> Self {
        Self {
            ws_sender,
            ws_receiver,
            text_to_send: String::new(),
            latest_per_channel: HashMap::new(),
            account_history: Vec::new(),
            timestamps: Vec::new(),
        }
    }

    fn ui(&mut self, ctx: &Context) {
        self.pump_incoming();

        CentralPanel::default().show(ctx, |ui| {
            self.draw_send_row(ui);
            ui.separator();
            self.draw_channels(ui);
        });
    }

    /// Drain any pending WS events into `latest_per_channel` and, for
    /// account messages, into the rolling history.
    fn pump_incoming(&mut self) {
        while let Some(event) = self.ws_receiver.try_recv() {
            match event {
                WsEvent::Message(WsMessage::Text(text)) => {
                    let envelope = match serde_json::from_str::<TextMsg>(&text) {
                        Ok(e) => e,
                        Err(e) => {
                            log::warn!("Bad TextMsg envelope: {e}. Raw: {text}");
                            continue;
                        },
                    };
                    let Some(parsed) = ParsedMsg::from_envelope(envelope.clone()) else {
                        continue;
                    };

                    // Push into rolling history if it's an account update.
                    if let ParsedMsg::Account { account, ts } = &parsed {
                        self.push_account_sample(account.clone(), ts.timestamp());
                    }

                    self.latest_per_channel.insert(envelope.channel, parsed);
                },
                WsEvent::Opened => log::info!("WebSocket opened"),
                WsEvent::Closed => log::info!("WebSocket closed"),
                WsEvent::Error(e) => log::error!("WebSocket error: {e}"),
                WsEvent::Message(_) => { /* binary messages ignored */ },
            }
        }
    }

    /// Push one sample into the bounded rolling history.
    fn push_account_sample(&mut self, account: Account, ts: i64) {
        if self.account_history.len() >= MAX_HISTORY {
            self.account_history.remove(0);
            self.timestamps.remove(0);
        }
        self.account_history.push(account);
        self.timestamps.push(ts);
    }

    fn draw_send_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Message to send:");
            let resp = ui.text_edit_singleline(&mut self.text_to_send);
            if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                let payload = std::mem::take(&mut self.text_to_send);
                self.ws_sender.send(WsMessage::Text(payload));
            }
        });
    }

    fn draw_channels(&mut self, ui: &mut egui::Ui) {
        let max_width = ui.available_width();
        let legend = Legend::default()
            .text_style(TextStyle::Monospace)
            .position(Corner::LeftBottom)
            .background_alpha(0.5);

        // Sort channels for stable ordering.
        let mut names: Vec<&String> = self.latest_per_channel.keys().collect();
        names.sort();

        // Draw structured channels; collect raw messages for a debug log.
        let mut raw_snapshot: Vec<(String, String)> = Vec::new();

        for name in names {
            let Some(msg) = self.latest_per_channel.get(name) else {
                continue;
            };
            match msg {
                ParsedMsg::Account { account, .. } => {
                    self.draw_account_panel(ui, account, &legend, max_width);
                },
                ParsedMsg::Portfolio { tokens } => {
                    draw_portfolio_panel(ui, tokens, &legend, max_width);
                },
                ParsedMsg::Raw { channel, text } => {
                    raw_snapshot.push((channel.clone(), text.clone()));
                },
            }
        }

        // Debug log: everything we didn't have a structured renderer for.
        for (channel, text) in &raw_snapshot {
            CollapsingHeader::new(format!("{channel} events")).show(ui, |ui| {
                ScrollArea::vertical().show(ui, |ui| {
                    let mut text = text.clone();
                    ui.horizontal(|ui| {
                        ui.add(TextEdit::multiline(&mut text));
                    });
                });
            });
        }
    }

    fn draw_account_panel(
        &self,
        ui: &mut egui::Ui,
        account: &Account,
        legend: &Legend,
        max_width: f32,
    ) {
        let balance = BalanceChart::new(&self.account_history, &self.timestamps);
        let change = ChangeChart::new(&self.account_history, &self.timestamps);
        let earnings = EarningsChart::new(&self.account_history, &self.timestamps);
        let plot_width = max_width / 3.0 - 20.0;

        ui.horizontal_wrapped(|ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.heading(RichText::new("Balances:").color(Color32::DARK_GRAY));
                    ui.add_space(3.0);
                    ui.label(format!("Current: {:.2}", account.balance.current));
                    ui.label(" | ");
                    ui.add_space(3.0);
                    ui.label(format!("Available: {:.2}", account.balance.available));
                });
                Plot::new("balance")
                    .legend(legend.clone())
                    .view_aspect(100.0)
                    .width(plot_width)
                    .height(200.0)
                    .show(ui, |plot| {
                        plot.line(Line::new(
                            "Current balance",
                            PlotPoints::new(balance.current),
                        ));
                        plot.line(Line::new("Token balance", PlotPoints::new(balance.tokens)));
                        plot.line(Line::new(
                            "Open orders balance",
                            PlotPoints::new(balance.open_orders),
                        ));
                        plot.line(Line::new(
                            "Available balance",
                            PlotPoints::new(balance.available),
                        ));
                    });
            });
            ui.add_space(10.0);

            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.heading(RichText::new("Change:").color(Color32::DARK_GRAY));
                    ui.add_space(3.0);
                    ui.heading(
                        RichText::new(format!("% {:.2}", account.change))
                            .color(change_color(account.change)),
                    );
                });
                Plot::new("change")
                    .legend(legend.clone())
                    .view_aspect(100.0)
                    .width(plot_width)
                    .height(200.0)
                    .show(ui, |plot| {
                        plot.line(
                            Line::new("Change", PlotPoints::new(change.change))
                                .color(change_color(account.change)),
                        );
                    });
            });
            ui.add_space(10.0);

            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.heading(RichText::new("Earnings:").color(Color32::DARK_GRAY));
                    ui.add_space(3.0);
                    ui.heading(
                        RichText::new(format!("{:.2}", account.earnings))
                            .color(change_color(account.earnings)),
                    );
                });
                Plot::new("earnings")
                    .legend(legend.clone())
                    .view_aspect(100.0)
                    .width(plot_width)
                    .height(200.0)
                    .show(ui, |plot| {
                        plot.line(
                            Line::new("Earnings", PlotPoints::new(earnings.earnings))
                                .color(change_color(account.change)),
                        );
                        plot.line(
                            Line::new("Fees", PlotPoints::new(earnings.fees))
                                .color(Color32::LIGHT_BLUE),
                        );
                    });
            });
        });
    }
}

fn draw_portfolio_panel(ui: &mut egui::Ui, tokens: &[Token], legend: &Legend, max_width: f32) {
    let plot_width = max_width / 3.0 - 20.0;

    for token_chunk in tokens.chunks(3) {
        ui.horizontal(|ui| {
            for token in token_chunk {
                draw_token_card(ui, token, legend, plot_width);
                ui.add_space(10.0);
            }
            ui.add_space(10.0);
        });
    }
}

fn draw_token_card(ui: &mut egui::Ui, token: &Token, legend: &Legend, plot_width: f32) {
    let box_plot = CandlestickBoxPlot::new(&token.candlesticks, token.buy_ts, token.buy_price);

    ui.vertical(|ui| {
        draw_token_header(ui, token);

        Frame::dark_canvas(ui.style()).show(ui, |ui| {
            Plot::new(token.instid.clone())
                .allow_drag(true)
                .allow_scroll(true)
                .legend(legend.clone())
                .allow_zoom(true)
                .show_y(false)
                .width(plot_width)
                .height(200.0)
                .show(ui, |plot_ui| {
                    plot_ui.box_plot(BoxPlot::new(&token.instid, box_plot.boxes));
                });
        });

        ui.horizontal_wrapped(|ui| {
            ui.label(format!("Price: {:.5}", token.price));
            ui.label(format!("Available Bal.: {:.4}", token.balance.available));
            ui.label(format!("Current Bal.: {:.4}", token.balance.current));
            ui.label(format!("SD.: {:.4}", token.std_deviation));
        });
    });
}

fn draw_token_header(ui: &mut egui::Ui, token: &Token) {
    ui.horizontal(|ui| {
        ui.heading(RichText::new(format!(
            "{} [{}]",
            token.instid, token.status
        )));
        ui.add_space(3.0);
        ui.heading(RichText::new("| Change:"));
        ui.add_space(0.2);
        ui.heading(
            RichText::new(format!("% {:.2}", token.change))
                .color(change_color(token.change as f64)),
        );
        ui.add_space(3.0);

        if token.config.timeout == token.timeout {
            ui.add_space(10.0);
            ui.heading(RichText::new("Live").color(Color32::LIGHT_BLUE).strong());
        } else if let Some(reason) = &token.exit_reason {
            ui.heading(RichText::new(reason).color(Color32::DARK_BLUE));
        } else {
            ui.heading(RichText::new("| Timeout:"));
            ui.add_space(0.2);
            ui.heading(
                RichText::new(format!("{:.2}", token.timeout.num_seconds()))
                    .color(time_color(token.timeout.num_seconds())),
            );
        }
    });
}

fn change_color(v: f64) -> Color32 {
    match v {
        _ if v > 0.0 => Color32::GREEN,
        _ if v < 0.0 => Color32::RED,
        _ => Color32::DARK_GRAY,
    }
}

fn time_color(v: i64) -> Color32 {
    if v > 10 {
        Color32::LIGHT_BLUE
    } else {
        Color32::YELLOW
    }
}
