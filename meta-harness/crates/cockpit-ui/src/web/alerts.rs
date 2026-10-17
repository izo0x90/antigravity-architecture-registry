//! The browser's notifications (the page asks for permission on the
//! user's click); out of sight is a hidden tab.

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{Notification as WebNotification, NotificationOptions, NotificationPermission, VisibilityState};

use crate::alerts::{Backend, Clicked, Permission};

pub(super) struct Web;

impl Backend for Web {
    fn permission(&self) -> Permission {
        match WebNotification::permission() {
            NotificationPermission::Granted => Permission::Granted,
            NotificationPermission::Denied => Permission::Denied,
            _ => Permission::Ask,
        }
    }

    fn ask(&self) {
        let _ = WebNotification::request_permission();
    }

    fn out_of_sight(&self) -> bool {
        web_sys::window().and_then(|w| w.document()).is_some_and(|d| d.visibility_state() == VisibilityState::Hidden)
    }

    fn raise(&self, server: &str, notification: &harness_contracts::Notification, clicked: Clicked) {
        let options = NotificationOptions::new();
        options.set_tag(&format!("{server}/{}", notification.id));
        if let Some(detail) = &notification.detail {
            options.set_body(detail);
        }
        let Ok(shown) = WebNotification::new_with_options(&notification.title, &options) else { return };
        let (item, it) = ((server.to_owned(), notification.id.clone()), shown.clone());
        let on_click = Closure::<dyn FnMut()>::new(move || {
            if let Some(window) = web_sys::window() {
                let _ = window.focus();
            }
            if let Ok(mut c) = clicked.lock() {
                *c = Some(item.clone());
            }
            it.close();
        });
        shown.set_onclick(Some(on_click.as_ref().unchecked_ref()));
        on_click.forget();
    }
}
