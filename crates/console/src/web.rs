//! WASM entry point.

use eframe::wasm_bindgen::{self, prelude::*};

/// Called once from JavaScript to boot the app.
#[wasm_bindgen]
pub async fn start(canvas_id: &str) -> Result<(), wasm_bindgen::JsValue> {
    // Route `log` messages to the browser console.
    eframe::WebLogger::init(log::LevelFilter::Debug).ok();

    // Resolve the canvas element the app should render into.
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(|| wasm_bindgen::JsValue::from_str("no document"))?;
    let canvas = document
        .get_element_by_id(canvas_id)
        .and_then(|el| el.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        .ok_or_else(|| {
            wasm_bindgen::JsValue::from_str(&format!("canvas #{canvas_id} not found"))
        })?;

    let app = crate::Console::default();
    eframe::WebRunner::new()
        .start(
            canvas,
            eframe::WebOptions::default(),
            Box::new(|_cc| Ok(Box::new(app))),
        )
        .await
}
