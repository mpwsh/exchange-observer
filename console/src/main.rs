#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), deny(warnings))] // Forbid warnings in release builds
#![warn(clippy::all, rust_2018_idioms)]

// When compiling natively:
#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() -> eframe::Result<()> {
    env_logger::init(); // Log to stderr (if you run with `RUST_LOG=debug`).

    let mut app = console::Console::default();
    app.url = "ws://127.0.0.1:9002".to_string();
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "exchange-observer",
        native_options,
        Box::new(|_cc| Box::new(app)),
    )
}
