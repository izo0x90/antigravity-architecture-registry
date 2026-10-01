use std::sync::{Arc, Mutex};
use ewebsock::{WsEvent, WsMessage, WsReceiver, WsSender};
use serde::{Deserialize, Serialize};

pub use harness_protocol::{
    ContentDeltaPayload, DriverSnapshot, ModelSnapshot, ProviderRuntimeEvent, SessionState,
    StartSessionInput, SendTurnInput, SendTurnResult, StreamKind, TaskPayload, TurnCompletedPayload,
    TurnStartedPayload, TurnState,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStartResponse {
    pub thread_id: String,
    pub status: String,
}

pub struct NetClient {
    base_url: String,
    ws_url: String,
    ws_sender: Option<WsSender>,
    ws_receiver: Option<WsReceiver>,
    pub incoming_events: Arc<Mutex<Vec<ProviderRuntimeEvent>>>,
    pub models: Arc<Mutex<Vec<ModelSnapshot>>>,
    pub is_connected: Arc<Mutex<bool>>,
    pub last_error: Arc<Mutex<Option<String>>>,
}

impl NetClient {
    pub fn new(host_origin: &str) -> Self {
        let base_url = host_origin.trim_end_matches('/').to_string();
        let ws_url = if base_url.starts_with("https://") {
            base_url.replacen("https://", "wss://", 1) + "/ws/events"
        } else {
            base_url.replacen("http://", "ws://", 1) + "/ws/events"
        };

        Self {
            base_url,
            ws_url,
            ws_sender: None,
            ws_receiver: None,
            incoming_events: Arc::new(Mutex::new(Vec::new())),
            models: Arc::new(Mutex::new(Vec::new())),
            is_connected: Arc::new(Mutex::new(false)),
            last_error: Arc::new(Mutex::new(None)),
        }
    }

    pub fn connect_ws(&mut self, ctx: egui::Context) {
        let options = ewebsock::Options::default();
        let wakeup = move || {
            ctx.request_repaint();
        };

        match ewebsock::connect_with_wakeup(&self.ws_url, options, wakeup) {
            Ok((sender, receiver)) => {
                self.ws_sender = Some(sender);
                self.ws_receiver = Some(receiver);
                if let Ok(mut c) = self.is_connected.lock() {
                    *c = true;
                }
            }
            Err(e) => {
                if let Ok(mut err) = self.last_error.lock() {
                    *err = Some(format!("WebSocket connect error: {}", e));
                }
            }
        }
    }

    pub fn poll_ws(&mut self) {
        if let Some(receiver) = &self.ws_receiver {
            while let Some(event) = receiver.try_recv() {
                match event {
                    WsEvent::Message(WsMessage::Text(text)) => {
                        match serde_json::from_str::<ProviderRuntimeEvent>(&text) {
                            Ok(parsed) => {
                                if let Ok(mut events) = self.incoming_events.lock() {
                                    events.push(parsed);
                                }
                            }
                            Err(err) => {
                                #[cfg(target_arch = "wasm32")]
                                web_sys::console::warn_1(&format!("Failed to parse ProviderRuntimeEvent: {} | text: {}", err, text).into());
                                #[cfg(not(target_arch = "wasm32"))]
                                eprintln!("Failed to parse ProviderRuntimeEvent: {} | text: {}", err, text);
                            }
                        }
                    }
                    WsEvent::Opened => {
                        if let Ok(mut c) = self.is_connected.lock() {
                            *c = true;
                        }
                    }
                    WsEvent::Closed => {
                        if let Ok(mut c) = self.is_connected.lock() {
                            *c = false;
                        }
                    }
                    WsEvent::Error(err) => {
                        if let Ok(mut e) = self.last_error.lock() {
                            *e = Some(format!("WS Error: {}", err));
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn fetch_models(&self, ctx: egui::Context) {
        let url = format!("{}/api/driver/snapshot", self.base_url);
        let request = ehttp::Request::get(&url);
        let models_arc = self.models.clone();
        let last_error_arc = self.last_error.clone();

        ehttp::fetch(request, move |result| {
            match result {
                Ok(response) => {
                    if response.ok {
                        if let Ok(snapshot) = serde_json::from_slice::<DriverSnapshot>(&response.bytes) {
                            if let Ok(mut list) = models_arc.lock() {
                                *list = snapshot.models;
                            }
                        }
                    } else {
                        if let Ok(mut err) = last_error_arc.lock() {
                            *err = Some(format!("Failed to load models: HTTP {}", response.status));
                        }
                    }
                }
                Err(e) => {
                    if let Ok(mut err) = last_error_arc.lock() {
                        *err = Some(format!("Failed to fetch models: {}", e));
                    }
                }
            }
            ctx.request_repaint();
        });
    }

    pub fn start_session<F>(&self, thread_id: Option<String>, ctx: egui::Context, on_done: F)
    where
        F: FnOnce(Result<String, String>) + Send + 'static,
    {
        let tid = thread_id.unwrap_or_else(|| {
            let now = js_sys::Date::now() as u64;
            format!("sess-{:x}", now & 0xffffff)
        });
        let url = format!("{}/api/sessions/start", self.base_url);
        let payload = serde_json::json!({
            "thread_id": tid,
            "cwd": ".",
            "runtime_mode": "auto-accept-edits"
        });
        let body = serde_json::to_vec(&payload).unwrap_or_default();
        let mut request = ehttp::Request::post(&url, body);
        request.headers.insert("Content-Type", "application/json");

        let ret_tid = tid.clone();
        ehttp::fetch(request, move |result| {
            match result {
                Ok(response) => {
                    if response.ok {
                        on_done(Ok(ret_tid));
                    } else {
                        let err_text = String::from_utf8_lossy(&response.bytes);
                        on_done(Err(format!("HTTP Error {}: {}", response.status, err_text)));
                    }
                }
                Err(e) => on_done(Err(format!("Network error: {}", e))),
            }
            ctx.request_repaint();
        });
    }

    pub fn send_turn<F>(&self, thread_id: &str, prompt: &str, model: &str, effort: &str, on_done: F)
    where
        F: FnOnce(Result<(), String>) + Send + 'static,
    {
        let url = format!("{}/api/sessions/{}/turn", self.base_url, thread_id);
        let payload = serde_json::json!({
            "prompt": prompt,
            "model": model,
            "effort": effort,
        });
        let body = serde_json::to_vec(&payload).unwrap_or_default();
        let mut request = ehttp::Request::post(&url, body);
        request.headers.insert("Content-Type", "application/json");

        ehttp::fetch(request, move |result| {
            match result {
                Ok(resp) => {
                    if resp.ok {
                        on_done(Ok(()));
                    } else {
                        on_done(Err(format!("Turn failed: HTTP {}", resp.status)));
                    }
                }
                Err(e) => on_done(Err(format!("Turn network error: {}", e))),
            }
        });
    }

    pub fn steer_session(&self, thread_id: &str, model: &str, mode: &str) {
        let url = format!("{}/api/sessions/{}/steer", self.base_url, thread_id);
        let payload = serde_json::json!({
            "model": model,
            "mode": mode,
        });
        let body = serde_json::to_vec(&payload).unwrap_or_default();
        let mut request = ehttp::Request::post(&url, body);
        request.headers.insert("Content-Type", "application/json");

        ehttp::fetch(request, |_| {});
    }

    pub fn respond_request(&self, thread_id: &str, request_id: &str, decision: &str) {
        let url = format!("{}/api/sessions/{}/requests/{}/respond", self.base_url, thread_id, request_id);
        let payload = serde_json::json!({
            "decision": decision,
        });
        let body = serde_json::to_vec(&payload).unwrap_or_default();
        let mut request = ehttp::Request::post(&url, body);
        request.headers.insert("Content-Type", "application/json");

        ehttp::fetch(request, |_| {});
    }

    pub fn fetch_file<F>(&self, path: &str, ctx: egui::Context, on_done: F)
    where
        F: FnOnce(Result<String, String>) + Send + 'static,
    {
        let encoded_path = urlencoding::encode(path);
        let url = format!("{}/api/fs/read?path={}", self.base_url, encoded_path);
        let request = ehttp::Request::get(&url);

        ehttp::fetch(request, move |result| {
            match result {
                Ok(resp) => {
                    if resp.ok {
                        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&resp.bytes) {
                            if let Some(content) = json.get("content").and_then(|c| c.as_str()) {
                                on_done(Ok(content.to_string()));
                            } else {
                                on_done(Err("Invalid response format".into()));
                            }
                        } else {
                            on_done(Err("Failed to parse JSON".into()));
                        }
                    } else {
                        on_done(Err(format!("File read HTTP error {}", resp.status)));
                    }
                }
                Err(e) => on_done(Err(format!("Network error: {}", e))),
            }
            ctx.request_repaint();
        });
    }

    pub fn submit_feedback<F>(&self, feedback_json: serde_json::Value, ctx: egui::Context, on_done: F)
    where
        F: FnOnce(Result<String, String>) + Send + 'static,
    {
        let url = format!("{}/api/dev/feedback", self.base_url);
        let body = serde_json::to_vec(&feedback_json).unwrap_or_default();
        let mut request = ehttp::Request::post(&url, body);
        request.headers.insert("Content-Type", "application/json");

        ehttp::fetch(request, move |result| {
            match result {
                Ok(resp) => {
                    if resp.ok {
                        on_done(Ok("Feedback saved successfully".into()));
                    } else {
                        on_done(Err(format!("HTTP Error {}", resp.status)));
                    }
                }
                Err(e) => on_done(Err(format!("Network error: {}", e))),
            }
            ctx.request_repaint();
        });
    }

    pub fn fetch_feedback<F>(&self, ctx: egui::Context, on_done: F)
    where
        F: FnOnce(Result<Vec<serde_json::Value>, String>) + Send + 'static,
    {
        let url = format!("{}/api/dev/feedback", self.base_url);
        let request = ehttp::Request::get(&url);

        ehttp::fetch(request, move |result| {
            match result {
                Ok(resp) => {
                    if resp.ok {
                        match serde_json::from_slice::<Vec<serde_json::Value>>(&resp.bytes) {
                            Ok(items) => on_done(Ok(items)),
                            Err(e) => on_done(Err(format!("JSON Parse error: {}", e))),
                        }
                    } else {
                        on_done(Err(format!("HTTP Error {}", resp.status)));
                    }
                }
                Err(e) => on_done(Err(format!("Network error: {}", e))),
            }
            ctx.request_repaint();
        });
    }

    pub fn update_feedback<F>(&self, feedback_json: serde_json::Value, ctx: egui::Context, on_done: F)
    where
        F: FnOnce(Result<String, String>) + Send + 'static,
    {
        let url = format!("{}/api/dev/feedback", self.base_url);
        let body = serde_json::to_vec(&feedback_json).unwrap_or_default();
        let mut request = ehttp::Request::post(&url, body);
        request.method = "PUT".to_string();
        request.headers.insert("Content-Type", "application/json");

        ehttp::fetch(request, move |result| {
            match result {
                Ok(resp) => {
                    if resp.ok {
                        on_done(Ok("Feedback updated successfully".into()));
                    } else {
                        on_done(Err(format!("HTTP Error {}", resp.status)));
                    }
                }
                Err(e) => on_done(Err(format!("Network error: {}", e))),
            }
            ctx.request_repaint();
        });
    }
}
