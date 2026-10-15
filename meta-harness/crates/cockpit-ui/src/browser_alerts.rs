//! The browser's own notifications, for what newly waits on the user while
//! the cockpit's tab is hidden. They're raised as the server's events
//! arrive (a hidden tab draws no frames); clicking one brings the tab back
//! and opens the item. Only in a browser, and only once the user turns
//! them on (the browser asks them first).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use harness_contracts::{ClientEvent, Level, NotificationId, NotificationState};

#[derive(Default, Clone)]
pub struct BrowserAlerts {
    on: Arc<AtomicBool>,
    /// The item a click asked for, until the cockpit opens it.
    clicked: Arc<Mutex<Option<NotificationId>>>,
}

impl BrowserAlerts {
    pub fn on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// Turned on (the browser asks the user to allow them, once) or off.
    pub fn set(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
        #[cfg(target_arch = "wasm32")]
        if on && web_sys::Notification::permission() == web_sys::NotificationPermission::Default {
            let _ = web_sys::Notification::request_permission();
        }
    }

    /// The item a notification's click asked for.
    pub fn take_clicked(&self) -> Option<NotificationId> {
        self.clicked.lock().ok()?.take()
    }

    /// For each server event as it arrives: a new waiting item, while the
    /// tab is hidden, is raised.
    pub fn arrival(&self) -> impl Fn(&str) + Send + Sync + 'static {
        let (on, clicked) = (self.on.clone(), self.clicked.clone());
        move |text: &str| {
            if !on.load(Ordering::Relaxed) || !text.contains("notification_updated") || !hidden() {
                return;
            }
            let Ok(ClientEvent::NotificationUpdated { notification }) = serde_json::from_str::<ClientEvent>(text) else { return };
            if notification.level == Level::Waiting && notification.state == NotificationState::Open {
                raise(&notification, &clicked);
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn hidden() -> bool {
    web_sys::window().and_then(|w| w.document()).is_some_and(|d| d.visibility_state() == web_sys::VisibilityState::Hidden)
}

#[cfg(not(target_arch = "wasm32"))]
fn hidden() -> bool {
    false
}

#[cfg(target_arch = "wasm32")]
fn raise(notification: &harness_contracts::Notification, clicked: &Arc<Mutex<Option<NotificationId>>>) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    if web_sys::Notification::permission() != web_sys::NotificationPermission::Granted {
        return;
    }
    let options = web_sys::NotificationOptions::new();
    options.set_tag(notification.id.as_str());
    if let Some(detail) = &notification.detail {
        options.set_body(detail);
    }
    let Ok(shown) = web_sys::Notification::new_with_options(&notification.title, &options) else { return };
    let (id, clicked, it) = (notification.id.clone(), clicked.clone(), shown.clone());
    let on_click = Closure::<dyn FnMut()>::new(move || {
        if let Some(window) = web_sys::window() {
            let _ = window.focus();
        }
        if let Ok(mut c) = clicked.lock() {
            *c = Some(id.clone());
        }
        it.close();
    });
    shown.set_onclick(Some(on_click.as_ref().unchecked_ref()));
    on_click.forget();
}

#[cfg(not(target_arch = "wasm32"))]
fn raise(_: &harness_contracts::Notification, _: &Arc<Mutex<Option<NotificationId>>>) {}
