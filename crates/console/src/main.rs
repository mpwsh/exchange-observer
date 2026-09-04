#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), deny(warnings))]
#![warn(clippy::all, rust_2018_idioms)]

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() -> eframe::Result<()> {
    env_logger::init();

    let app = console::Console::default();

    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 720.0])
            .with_maximized(true),
        ..Default::default()
    };

    eframe::run_native(
        "exchange-observer",
        native_options,
        Box::new(|_cc| Ok(Box::new(app))),
    )
}

// The wasm entry point lives in web.rs; main.rs is only used on native.
#[cfg(target_arch = "wasm32")]
fn main() {}
