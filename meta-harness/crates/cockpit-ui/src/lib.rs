pub mod app;
pub mod chat;
pub mod code;
pub mod feedback;
pub mod graph;
pub mod graph_draw;
pub mod graph_layout;
pub mod keymap;
pub mod net;
pub mod samples;
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
                    let mut fonts = egui::FontDefinitions::default();
                    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
                    if let Some(font_keys) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                        font_keys.insert(1, "phosphor".into());
                    }
                    cc.egui_ctx.set_fonts(fonts);
                    Ok(Box::new(CockpitApp::new(&origin)))
                }),
            )
            .await
    }
}
