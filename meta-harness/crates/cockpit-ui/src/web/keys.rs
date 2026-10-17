//! The browser keeps some keys for itself.

/// Keys with ctrl the cockpit binds stay the cockpit's in a browser: the
/// browser's own use of them (history, the address bar, bookmarks) is kept
/// from them (eframe keeps only a few). Read from the keys in use, so a
/// user's keymap counts.
pub fn keep_browser_keys(window: &web_sys::Window) -> Result<(), wasm_bindgen::JsValue> {
    use wasm_bindgen::JsCast;
    let listener = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(|e: web_sys::KeyboardEvent| {
        if e.ctrl_key() && !e.shift_key() && !e.alt_key() && !e.meta_key() && crate::keymap::local().browser_keys.contains(&e.key().to_lowercase()) {
            e.prevent_default();
        }
    });
    let options = web_sys::AddEventListenerOptions::new();
    options.set_capture(true);
    window.add_event_listener_with_callback_and_add_event_listener_options("keydown", listener.as_ref().unchecked_ref(), &options)?;
    listener.forget();
    Ok(())
}
