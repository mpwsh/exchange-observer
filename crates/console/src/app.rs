//! Console UI: connects to the scheduler's websocket and renders a live
//! dashboard of account state + per-token trading progress.
//!
//! Layout, top to bottom:
//!
//! 1. **Account header strip** — one dense monospace row (balance,
//!    earnings, %chg, timestamp) with a connection pill and a settings
//!    cog on the right.
//! 2. **Open positions** — responsive card grid: 2 cards per row when the
//!    window is wide enough, 1 per row when it's narrow. Each card has
//!    a clickable arrow to expand its candle chart inline.
//! 3. **Recent trades** — placeholder strip, wired up later when the
//!    scheduler grows a `reports` channel.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use egui::{
    Color32, Context, Frame, Key, Margin, RichText, ScrollArea, Sense, Stroke, TextStyle,
    TopBottomPanel, CentralPanel,
};
use egui_plot::{BoxPlot, Corner, Legend, Plot};
use ewebsock::{Options, WsEvent, WsMessage, WsReceiver, WsSender};
use serde::Deserialize;

use self::{
    charts::CandlestickBoxPlot,
    models::{Account, Report, Token},
};

mod charts;
mod models;

/// Responsive breakpoint: 2-column grid above this width, single column below.
const GRID_2COL_MIN_WIDTH: f32 = 900.0;

/// Default WS URL used only when nothing has been persisted from a prior run.
const DEFAULT_URL: &str = "ws://127.0.0.1:9002";

// ---------------------------------------------------------------------------
// Palette — terminal-aesthetic. Muted, monochrome background, one accent
// per state.
// ---------------------------------------------------------------------------

const STRIP_BG: Color32 = Color32::from_rgb(20, 22, 26);
const STRIP_BORDER: Color32 = Color32::from_rgb(48, 52, 60);
const CARD_BG: Color32 = Color32::from_rgb(16, 18, 22);
const DIM: Color32 = Color32::from_rgb(140, 145, 155);
const FG: Color32 = Color32::from_rgb(210, 214, 220);
const UP: Color32 = Color32::from_rgb(120, 220, 130);
const DOWN: Color32 = Color32::from_rgb(230, 100, 100);
const INFO: Color32 = Color32::from_rgb(120, 200, 230);
const WARN: Color32 = Color32::from_rgb(230, 200, 100);

fn strip_frame() -> Frame {
    Frame::NONE
        .fill(STRIP_BG)
        .inner_margin(Margin::symmetric(10, 6))
        .stroke(Stroke::new(1.0, STRIP_BORDER))
}

fn card_frame() -> Frame {
    Frame::NONE
        .fill(CARD_BG)
        .inner_margin(Margin::symmetric(10, 8))
        .stroke(Stroke::new(1.0, STRIP_BORDER))
}

fn change_color(v: f64) -> Color32 {
    match v {
        _ if v > 0.0 => UP,
        _ if v < 0.0 => DOWN,
        _ => DIM,
    }
}

fn time_color(secs: i64) -> Color32 {
    if secs > 30 {
        INFO
    } else if secs > 10 {
        WARN
    } else {
        DOWN
    }
}

fn mono(text: impl Into<String>, color: Color32) -> RichText {
    RichText::new(text).monospace().color(color)
}

/// Format unix seconds as `HH:MM`. Empty on out-of-range values so the tick
/// just doesn't render instead of showing gibberish during autoscale probing.
/// Seconds are redundant on a minute-candle chart and only crowd the axis —
/// `HH:MM:SS` labels are wide enough that egui_plot's tick-density heuristic
/// thins them out until only one survives.
fn fmt_ts_axis(mark: egui_plot::GridMark, _range: &std::ops::RangeInclusive<f64>) -> String {
    DateTime::<Utc>::from_timestamp(mark.value as i64, 0)
        .map(|dt| dt.format("%H:%M").to_string())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// URL persistence — one string, platform-appropriate storage.
// ---------------------------------------------------------------------------
// eframe's `persistence` feature could handle this but it drags in a lot of
// serde-based machinery for what is literally one string. Do it by hand.

#[cfg(not(target_arch = "wasm32"))]
mod persist {
    use std::path::PathBuf;

    fn config_path() -> Option<PathBuf> {
        // Prefer $XDG_CONFIG_HOME, fall back to $HOME/.config.
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("exchange-observer-console").join("url"))
    }

    pub fn load_url() -> Option<String> {
        let path = config_path()?;
        std::fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
    }

    pub fn save_url(url: &str) {
        let Some(path) = config_path() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, url);
    }
}

#[cfg(target_arch = "wasm32")]
mod persist {
    const KEY: &str = "exchange-observer-console.url";

    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }

    pub fn load_url() -> Option<String> {
        storage()?.get_item(KEY).ok().flatten()
    }

    pub fn save_url(url: &str) {
        if let Some(s) = storage() {
            let _ = s.set_item(KEY, url);
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level app
// ---------------------------------------------------------------------------

pub struct Console {
    /// Current URL — either the persisted one, the default, or whatever the
    /// user typed in the settings modal most recently. Only saved to disk
    /// once a connection *succeeds*, so bad URLs don't get remembered.
    url: String,
    /// Frontend (present iff we have a live WS connection).
    frontend: Option<FrontEnd>,
    /// If Some, an error to show in a modal — from a failed connect attempt.
    connect_error: Option<String>,
    /// Whether the settings modal is open.
    settings_open: bool,
    /// URL being edited inside the settings modal. Only committed to `url`
    /// (and reconnected) on Save.
    settings_url_draft: String,
    /// Whether autoconnect has run yet this session.
    autoconnect_done: bool,
}

impl Default for Console {
    fn default() -> Self {
        // Load persisted URL if we have one; otherwise use the default.
        let url = persist::load_url()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_URL.to_owned());
        Self {
            settings_url_draft: url.clone(),
            url,
            frontend: None,
            connect_error: None,
            settings_open: false,
            autoconnect_done: false,
        }
    }
}

impl eframe::App for Console {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // One autoconnect attempt on first frame. Failure surfaces as a
        // modal; no automatic retries — the user hits reconnect when ready.
        if !self.autoconnect_done {
            self.autoconnect_done = true;
            self.connect(ctx);
        }

        self.draw_account_strip(ctx);
        self.draw_recent_trades_strip(ctx);

        if let Some(frontend) = &mut self.frontend {
            frontend.draw_positions(ctx);
        } else {
            // No connection yet: show a friendly empty state in the center.
            CentralPanel::default().frame(strip_frame()).show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.label(mono("not connected", DIM));
                    ui.add_space(4.0);
                    ui.label(mono("open settings (⚙) to change URL and reconnect", DIM));
                });
            });
        }

        // Modals last so they render on top of everything else.
        self.draw_error_modal(ctx);
        self.draw_settings_modal(ctx);
    }
}

impl Console {
    /// Top strip: account header + connection pill + settings cog. Merged
    /// account and control chrome into one strip — no separate connect bar,
    /// no menu bar, no error strip; errors go to a modal.
    fn draw_account_strip(&mut self, ctx: &Context) {
        TopBottomPanel::top("account_strip")
            .frame(strip_frame())
            .show(ctx, |ui| {
                let account_and_ts = self.frontend.as_ref().and_then(|f| {
                    if let Some(ParsedMsg::Account { account, ts }) =
                        f.latest_per_channel.get("account")
                    {
                        Some((account.clone(), *ts))
                    } else {
                        None
                    }
                });

                ui.horizontal(|ui| {
                    if let Some((account, _)) = &account_and_ts {
                        let field = |ui: &mut egui::Ui, label: &str, value: RichText| {
                            ui.label(mono(format!("{label:>10} "), DIM));
                            ui.label(value);
                            ui.add_space(14.0);
                        };
                        field(
                            ui,
                            "Balance",
                            mono(format!("${:>10.2}", account.balance.current), FG),
                        );
                        field(
                            ui,
                            "Available",
                            mono(format!("${:>10.2}", account.balance.available), FG),
                        );
                        field(
                            ui,
                            "Spendable",
                            mono(format!("${:>8.2}", account.balance.spendable), FG),
                        );
                        field(
                            ui,
                            "Earnings",
                            mono(
                                format!("${:>8.2}", account.earnings),
                                change_color(account.earnings),
                            ),
                        );
                        field(
                            ui,
                            "Fees",
                            mono(format!("${:>6.2}", account.fee_spend), DIM),
                        );
                        field(
                            ui,
                            "Change",
                            mono(
                                format!("{:>+6.2}%", account.change),
                                change_color(account.change),
                            ),
                        );
                    } else {
                        ui.label(mono("waiting for account…", DIM));
                    }

                    // Right side: timestamp, connection pill, settings cog.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Settings cog (rightmost).
                        let cog =
                            egui::Button::new(RichText::new("⚙").size(16.0).color(DIM))
                                .frame(false);
                        if ui.add(cog).on_hover_text("Settings").clicked() {
                            self.settings_url_draft = self.url.clone();
                            self.settings_open = true;
                        }
                        ui.add_space(6.0);

                        // Connection pill.
                        if self.frontend.is_some() {
                            ui.label(mono("● connected", UP));
                        } else {
                            ui.label(mono("○ disconnected", DIM));
                        }
                        ui.add_space(12.0);

                        // Timestamp (or fallback dashes).
                        let ts_str = account_and_ts
                            .as_ref()
                            .map(|(_, ts)| ts.format("%H:%M:%S UTC").to_string())
                            .unwrap_or_else(|| "--:--:-- UTC".into());
                        ui.label(mono(ts_str, DIM));
                    });
                });
            });
    }

    /// Recent trades — live feed of closed positions as the scheduler
    /// pushes them through the `report` channel. Ring-buffered per session;
    /// Scylla remains the durable history.
    fn draw_recent_trades_strip(&self, ctx: &Context) {
        TopBottomPanel::bottom("recent_trades_strip")
            .resizable(true)
            .default_height(180.0)
            .frame(strip_frame())
            .show(ctx, |ui| {
                let reports: &[(DateTime<Utc>, Report)] = self
                    .frontend
                    .as_ref()
                    .map(|f| f.recent_reports.as_slice())
                    .unwrap_or(&[]);

                ui.horizontal(|ui| {
                    ui.label(mono("RECENT TRADES", DIM));
                    ui.add_space(10.0);
                    if reports.is_empty() {
                        ui.label(mono("(no closes yet this session)", DIM));
                    } else {
                        ui.label(mono(format!("({} shown)", reports.len()), DIM));
                    }
                });
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    let h = |ui: &mut egui::Ui, w: f32, s: &str| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(w, 16.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| ui.label(mono(s, DIM)),
                        );
                    };
                    h(ui, 80.0, "TIME");
                    h(ui, 20.0, "");
                    h(ui, 100.0, "SYMBOL");
                    h(ui, 120.0, "REASON");
                    h(ui, 90.0, "EARNINGS");
                    h(ui, 60.0, "FEES");
                    h(ui, 70.0, "CHANGE");
                    h(ui, 70.0, "HIGHEST");
                    h(ui, 70.0, "LOWEST");
                });
                ui.separator();

                if reports.is_empty() {
                    return;
                }

                ScrollArea::vertical()
                    .id_salt("recent_trades_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (ts, r) in reports.iter() {
                            let cell = |ui: &mut egui::Ui, w: f32, text: RichText| {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(w, 16.0),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| ui.label(text),
                                );
                            };

                            // Outcome glyph: cashout = ★, floor_reached win = ✓,
                            // stoploss / big loss = ✗, else · (mostly zero-ish
                            // timeouts). Same taxonomy as watch_trades.sh so
                            // the visual language is consistent across tools.
                            let mark = match r.reason.as_str() {
                                "cashout" => "★",
                                "stoploss" => "✗",
                                "floor_reached" if r.earnings > 0.0 => "✓",
                                _ if r.earnings > 0.05 => "✓",
                                _ if r.earnings < -0.10 => "✗",
                                _ => "·",
                            };
                            let mark_color = match mark {
                                "★" => UP,
                                "✓" => UP,
                                "✗" => DOWN,
                                _ => DIM,
                            };

                            ui.horizontal(|ui| {
                                cell(ui, 80.0, mono(ts.format("%H:%M:%S").to_string(), DIM));
                                cell(ui, 20.0, mono(mark, mark_color));
                                cell(
                                    ui,
                                    100.0,
                                    mono(short_instid(&r.instid), FG),
                                );
                                cell(ui, 120.0, mono(&r.reason, DIM));
                                cell(
                                    ui,
                                    90.0,
                                    mono(
                                        format!("${:>+7.3}", r.earnings),
                                        change_color(r.earnings),
                                    ),
                                );
                                cell(
                                    ui,
                                    60.0,
                                    mono(format!("${:>5.3}", r.fees), DIM),
                                );
                                cell(
                                    ui,
                                    70.0,
                                    mono(
                                        format!("{:>+6.2}%", r.change),
                                        change_color(f64::from(r.change)),
                                    ),
                                );
                                cell(
                                    ui,
                                    70.0,
                                    mono(format!("{:>+6.2}%", r.highest), DIM),
                                );
                                cell(
                                    ui,
                                    70.0,
                                    mono(format!("{:>+6.2}%", r.lowest), DIM),
                                );
                            });
                        }
                    });
            });
    }

    fn draw_error_modal(&mut self, ctx: &Context) {
        let Some(err) = self.connect_error.clone() else {
            return;
        };

        // Background dim: paint a semi-transparent overlay over the whole app
        // before showing the modal window on top of it.
        egui::Area::new("error_modal_bg".into())
            .fixed_pos(egui::pos2(0.0, 0.0))
            .interactable(true)
            .show(ctx, |ui| {
                let screen = ctx.screen_rect();
                ui.painter().rect_filled(
                    screen,
                    0.0,
                    Color32::from_black_alpha(160),
                );
                // Block clicks reaching content below.
                ui.allocate_rect(screen, Sense::click_and_drag());
            });

        egui::Window::new(RichText::new("Connection error").color(DOWN).strong())
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                ui.add_space(4.0);
                ui.label(mono("Could not connect to:", DIM));
                ui.label(mono(&self.url, FG));
                ui.add_space(6.0);
                ui.label(mono("Reason reported:", DIM));
                ui.label(mono(&err, FG));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        self.connect_error = None;
                    }
                });
            });
    }

    fn draw_settings_modal(&mut self, ctx: &Context) {
        if !self.settings_open {
            return;
        }

        egui::Area::new("settings_modal_bg".into())
            .fixed_pos(egui::pos2(0.0, 0.0))
            .interactable(true)
            .show(ctx, |ui| {
                let screen = ctx.screen_rect();
                ui.painter().rect_filled(
                    screen,
                    0.0,
                    Color32::from_black_alpha(160),
                );
                ui.allocate_rect(screen, Sense::click_and_drag());
            });

        let mut save_clicked = false;
        let mut cancel_clicked = false;

        egui::Window::new(RichText::new("Settings").color(FG).strong())
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(400.0);
                ui.add_space(4.0);
                ui.label(mono("Scheduler WebSocket URL", DIM));
                let resp = ui.text_edit_singleline(&mut self.settings_url_draft);
                if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    save_clicked = true;
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        save_clicked = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel_clicked = true;
                    }
                });
            });

        if save_clicked {
            self.url = self.settings_url_draft.trim().to_owned();
            self.settings_open = false;
            // Drop any existing connection and reconnect at the new URL.
            self.frontend = None;
            self.connect(ctx);
        } else if cancel_clicked {
            self.settings_open = false;
        }
    }

    /// Attempt to connect using `self.url`. On success, persist it so
    /// autoconnect can use it next time. On failure, surface the reason in
    /// the error modal.
    fn connect(&mut self, ctx: &Context) {
        let wakeup = {
            let ctx = ctx.clone();
            move || ctx.request_repaint()
        };
        match ewebsock::connect_with_wakeup(&self.url, Options::default(), wakeup) {
            Ok((sender, receiver)) => {
                self.frontend = Some(FrontEnd::new(sender, receiver));
                self.connect_error = None;
                persist::save_url(&self.url);
            },
            Err(err) => {
                log::error!("Failed to connect to {:?}: {err}", self.url);
                self.connect_error = Some(err.to_string());
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Frontend: WS pump + positions grid.
// ---------------------------------------------------------------------------

#[derive(Deserialize, Clone)]
struct TextMsg {
    channel: String,
    data: String,
    ts: DateTime<Utc>,
}

#[derive(Clone)]
enum ParsedMsg {
    Account { account: Account, ts: DateTime<Utc> },
    Portfolio { tokens: Vec<Token> },
    Report { report: Report, ts: DateTime<Utc> },
    Raw { channel: String, text: String },
}

impl ParsedMsg {
    fn from_envelope(envelope: TextMsg) -> Option<Self> {
        match envelope.channel.as_str() {
            "account" => match serde_json::from_str::<Account>(&envelope.data) {
                Ok(account) => Some(ParsedMsg::Account { account, ts: envelope.ts }),
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
            "report" => match serde_json::from_str::<Report>(&envelope.data) {
                Ok(report) => Some(ParsedMsg::Report { report, ts: envelope.ts }),
                Err(e) => {
                    log::warn!("Bad report payload: {e}");
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
    #[allow(dead_code)] // reserved for future two-way protocol.
    ws_sender: WsSender,
    ws_receiver: WsReceiver,
    latest_per_channel: HashMap<String, ParsedMsg>,
    /// Tokens whose candle chart is currently expanded inside their card.
    /// New tokens auto-populate this on first sight (see `seen`), so cards
    /// arrive expanded by default; a manual collapse survives subsequent
    /// portfolio messages because `seen` records the token permanently.
    expanded: HashSet<String>,
    /// Tokens we've observed in any portfolio message this session. Used
    /// only to distinguish "brand new — expand it" from "already known —
    /// respect whatever the user last did with it".
    seen: HashSet<String>,
    /// Rolling ring of the most-recent closed reports, newest first.
    /// Bounded so long sessions don't grow unbounded — the reports table
    /// in Scylla is the durable source of truth; this is just the log
    /// feed on the console.
    recent_reports: Vec<(DateTime<Utc>, Report)>,
}

/// How many closed trades the console keeps in the "recent trades" strip.
/// Beyond this, run `analyze_reports.sh` or query Scylla — the console
/// is a live feed, not a historical browser.
const RECENT_REPORTS_CAP: usize = 100;

impl FrontEnd {
    fn new(ws_sender: WsSender, ws_receiver: WsReceiver) -> Self {
        Self {
            ws_sender,
            ws_receiver,
            latest_per_channel: HashMap::new(),
            expanded: HashSet::new(),
            seen: HashSet::new(),
            recent_reports: Vec::new(),
        }
    }

    fn draw_positions(&mut self, ctx: &Context) {
        self.pump_incoming();

        CentralPanel::default().frame(strip_frame()).show(ctx, |ui| {
            let Some(ParsedMsg::Portfolio { tokens }) =
                self.latest_per_channel.get("portfolio").cloned()
            else {
                ui.label(mono("waiting for portfolio…", DIM));
                return;
            };

            if tokens.is_empty() {
                ui.label(mono("no open positions", DIM));
                return;
            }

            // Newly-observed tokens open expanded. Manual collapse survives
            // subsequent portfolio messages because `seen` records them.
            for token in &tokens {
                if !self.seen.contains(&token.instid) {
                    self.seen.insert(token.instid.clone());
                    self.expanded.insert(token.instid.clone());
                }
            }

            ScrollArea::vertical()
                .id_salt("positions_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Compute card width from the scroll area's inner width so
                    // the vertical scrollbar's reserved gutter doesn't push
                    // the right column off-screen (which was the earlier
                    // overflow bug — outer CentralPanel width included the
                    // area the scrollbar would sit in).
                    let avail_w = ui.available_width();
                    let cols = if avail_w >= GRID_2COL_MIN_WIDTH { 2 } else { 1 };
                    let col_gap = 8.0;
                    let card_w =
                        (avail_w - col_gap * (cols as f32 - 1.0)) / cols as f32;

                    for row_tokens in tokens.chunks(cols) {
                        ui.horizontal_top(|ui| {
                            for (i, token) in row_tokens.iter().enumerate() {
                                if i > 0 {
                                    ui.add_space(col_gap);
                                }
                                ui.allocate_ui_with_layout(
                                    egui::vec2(card_w, 0.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| self.draw_position_card(ui, token, card_w),
                                );
                            }
                        });
                        ui.add_space(col_gap);
                    }
                });
        });
    }

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
                    // Reports are a *feed*, not a "latest state" channel:
                    // push into the ring buffer instead of overwriting the
                    // per-channel latest slot. The latest_per_channel slot
                    // still records the newest report so tests / diagnostics
                    // can see it, but the UI reads from `recent_reports`.
                    if let ParsedMsg::Report { report, ts } = &parsed {
                        self.recent_reports.insert(0, (*ts, report.clone()));
                        self.recent_reports.truncate(RECENT_REPORTS_CAP);
                    }
                    self.latest_per_channel.insert(envelope.channel, parsed);
                },
                WsEvent::Opened => log::info!("WebSocket opened"),
                WsEvent::Closed => log::info!("WebSocket closed"),
                WsEvent::Error(e) => log::error!("WebSocket error: {e}"),
                WsEvent::Message(_) => { /* binary ignored */ },
            }
        }
    }

    fn draw_position_card(&mut self, ui: &mut egui::Ui, token: &Token, card_w: f32) {
        card_frame().show(ui, |ui| {
            ui.set_width(card_w - 20.0); // account for card_frame margin
            let is_expanded = self.expanded.contains(&token.instid);

            // Header row: expand button + symbol + numbers.
            ui.horizontal(|ui| {
                let arrow = if is_expanded { "▾" } else { "▸" };
                let btn = egui::Button::new(
                    RichText::new(arrow).monospace().color(FG),
                )
                .frame(false)
                .min_size(egui::vec2(20.0, 20.0));
                if ui.add(btn).clicked() {
                    if is_expanded {
                        self.expanded.remove(&token.instid);
                    } else {
                        self.expanded.insert(token.instid.clone());
                    }
                }

                ui.label(mono(
                    format!("{:<8}", short_instid(&token.instid)),
                    FG,
                ));
                ui.label(mono(format!("{:>10.5}", token.price), FG));
                ui.label(mono(
                    format!("{:>+6.2}%", token.change),
                    change_color(f64::from(token.change)),
                ));
                ui.label(mono(
                    format!("${:>+7.2}", token.earnings),
                    change_color(token.earnings),
                ));

                let secs = token.timeout.num_seconds();
                ui.label(mono(format!("{secs:>4}s"), time_color(secs)));

                let status_color = if token.status.eq_ignore_ascii_case("trading") {
                    UP
                } else {
                    INFO
                };
                ui.label(mono(&token.status, status_color));

                if let Some(reason) = &token.exit_reason {
                    ui.label(mono(reason, DIM));
                }
            });

            if is_expanded {
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    let f = |ui: &mut egui::Ui, k: &str, v: RichText| {
                        ui.label(mono(format!("{k}: "), DIM));
                        ui.label(v);
                        ui.add_space(14.0);
                    };
                    f(ui, "buy", mono(format!("{:.5}", token.buy_price), FG));
                    f(ui, "vol 20m", mono(format!("{:.0}", token.vol), FG));
                    f(ui, "vol 24h", mono(format!("{:.0}", token.vol24h), FG));
                    f(ui, "range 20m", mono(format!("{:.2}%", token.range), FG));
                    f(ui, "std dev", mono(format!("{:.4}", token.std_deviation), FG));
                });
                ui.add_space(4.0);

                let box_plot =
                    CandlestickBoxPlot::new(&token.candlesticks, token.buy_ts, token.buy_price);
                Plot::new(format!("candles_{}", token.instid))
                    .allow_drag(true)
                    .allow_scroll(false)
                    .allow_zoom(true)
                    .show_y(true)
                    .x_axis_formatter(fmt_ts_axis)
                    .legend(
                        Legend::default()
                            .text_style(TextStyle::Monospace)
                            .position(Corner::LeftBottom)
                            .background_alpha(0.5),
                    )
                    .height(160.0)
                    .show(ui, |plot_ui| {
                        plot_ui.box_plot(BoxPlot::new(&token.instid, box_plot.boxes));
                    });
            }
        });
    }
}

fn short_instid(instid: &str) -> &str {
    instid.split_once('-').map_or(instid, |(base, _)| base)
}
