//! The cockpit in a browser: its start-up (the canvas, the keys the browser
//! would take, the page's door, logging to the console) and what the
//! browser does for it (alerts as the browser's notifications). The only
//! code that knows it runs in a browser; the app gets what differs by
//! platform from here, handed in.

mod alerts;
mod keys;
mod page;

use crate::app::CockpitApp;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct WebHandle {
    runner: eframe::WebRunner,
}

impl Default for WebHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl WebHandle {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self { runner: eframe::WebRunner::new() }
    }

    #[wasm_bindgen]
    pub async fn start(&self, canvas_id: &str) -> Result<(), JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window found"))?;
        let document = window.document().ok_or_else(|| JsValue::from_str("No document found"))?;
        let canvas = document
            .get_element_by_id(canvas_id)
            .ok_or_else(|| JsValue::from_str(&format!("Canvas with id '{}' not found", canvas_id)))?
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .map_err(|_| JsValue::from_str("Element is not a canvas"))?;

        // Said in the browser's console.
        let _ = eframe::WebLogger::init(log::LevelFilter::Info);
        keys::keep_browser_keys(&window)?;
        let location = window.location();
        let origin = location.origin().unwrap_or_else(|_| "http://localhost:8080".to_string());

        let web_options = eframe::WebOptions::default();
        self.runner
            .start(
                canvas,
                web_options,
                Box::new(move |cc| {
                    let mut app = CockpitApp::new(&origin, std::sync::Arc::new(alerts::Web));
                    app.start_fonts(&cc.egui_ctx);
                    if let Err(e) = page::install_page_door(app.page.wake(&cc.egui_ctx)) {
                        log::warn!("window.nah isn't there: {e:?}");
                    }
                    Ok(Box::new(app))
                }),
            )
            .await
    }
}
