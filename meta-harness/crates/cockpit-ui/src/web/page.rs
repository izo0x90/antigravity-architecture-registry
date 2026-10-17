//! The page's door: scripts in it (and agents, through the cockpit's
//! socket) call the cockpit as `window.nah`.

/// `window.nah`: `call({op, ...})` and the same calls by name, each a promise.
pub fn install_page_door(door: crate::remote::Door) -> Result<(), wasm_bindgen::JsValue> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;

    let call = Closure::<dyn Fn(JsValue) -> js_sys::Promise>::new(move |call: JsValue| {
        let door = door.clone();
        js_sys::Promise::new(&mut |resolve, reject| {
            let text = js_sys::JSON::stringify(&call).ok().and_then(|s| s.as_string()).unwrap_or_default();
            door.knock(
                &text,
                Box::new(move |answer| match answer {
                    Ok(value) => {
                        let value = js_sys::JSON::parse(&value.to_string()).unwrap_or(JsValue::NULL);
                        let _ = resolve.call1(&JsValue::NULL, &value);
                    }
                    Err(error) => {
                        let _ = reject.call1(&JsValue::NULL, &JsValue::from_str(&error));
                    }
                }),
            );
        })
    });
    let nah = js_sys::Object::new();
    js_sys::Reflect::set(&nah, &"call".into(), call.as_ref().unchecked_ref())?;
    call.forget();
    let sugar = js_sys::Function::new_with_args(
        "nah",
        "nah.read = () => nah.call({op: 'read'});
         nah.actions = () => nah.call({op: 'actions'});
         nah.run = (id, value) => nah.call({op: 'run', id, value});
         nah.click = (what) => nah.call(typeof what === 'string' ? {op: 'click', text: what} : {op: 'click', ...what});
         nah.type = (text) => nah.call({op: 'type', text});
         nah.keys = (keys) => nah.call({op: 'keys', keys});",
    );
    sugar.call1(&JsValue::NULL, &nah)?;
    let window = web_sys::window().ok_or("no window")?;
    js_sys::Reflect::set(&window, &"nah".into(), &nah)?;
    Ok(())
}
