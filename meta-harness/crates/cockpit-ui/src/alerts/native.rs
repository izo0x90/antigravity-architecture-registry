//! The native build's alerts: there's no native cockpit app yet, so there's
//! no OS notification backend; the indicator says so rather than seeming on.

use super::{Backend, Clicked, Permission};

pub struct Native;

impl Backend for Native {
    fn permission(&self) -> Permission {
        Permission::Unavailable("the native cockpit has no OS notifications yet")
    }

    fn ask(&self) {}

    fn out_of_sight(&self) -> bool {
        false
    }

    fn raise(&self, _: &str, _: &harness_contracts::Notification, _: Clicked) {}
}
