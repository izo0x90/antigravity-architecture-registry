pub mod actions;
pub mod app;
pub mod chat;
pub mod code;
pub mod columns;
pub mod config;
pub mod feedback;
pub mod graph;
pub mod graph_config;
pub mod goto;
pub mod graph_style;
pub mod keymap;
pub mod nav;
pub mod net;
pub mod palette;
pub mod panes;
pub mod queue;
pub mod quick_look;
pub mod remote;
pub mod request_card;
pub mod review;
pub mod saved;
pub mod spec;
pub mod status;
pub mod talk;
pub mod store;
pub mod theme;
pub mod touch;
pub mod tree;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;
#[cfg(target_arch = "wasm32")]
use app::CockpitApp;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub struct WebHandle {
    runner: eframe::WebRunner,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
impl WebHandle {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            runner: eframe::WebRunner::new(),
        }
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

        let location = window.location();
        let origin = location.origin().unwrap_or_else(|_| "http://localhost:8080".to_string());

        let web_options = eframe::WebOptions::default();
        self.runner
            .start(
                canvas,
                web_options,
                Box::new(move |cc| {
                    cc.egui_ctx.set_fonts(crate::theme::font_definitions());
                    let mut app = CockpitApp::new(&origin);
                    if let Err(e) = crate::remote::install_page_door(app.remote.door(&cc.egui_ctx)) {
                        web_sys::console::warn_1(&e);
                    }
                    Ok(Box::new(app))
                }),
            )
            .await
    }
}
