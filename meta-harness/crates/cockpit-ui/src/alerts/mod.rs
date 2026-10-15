//! System alerts: what newly waits on the user, raised outside the cockpit
//! (a browser notification, an OS one) while the cockpit is out of sight.
//! One of the ways notifications reach the user: the cockpit's own counters
//! and quick look read them in place, and the server's adapters take them
//! off the device. They're raised as home's events arrive (a hidden tab
//! draws no frames), through whichever backend the build has; clicking one
//! brings the cockpit back and opens the item.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use harness_contracts::{ClientEvent, HomeEvent, Level, Notification, NotificationId, NotificationState};

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod web;

/// Whether the platform lets alerts be shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Permission {
    Granted,
    /// Not asked yet: asking needs the user's click (the browser's rule).
    Ask,
    /// The user (or the platform) said no; changed outside the cockpit.
    Denied,
    /// This build has no way to show them, and why.
    Unavailable(&'static str),
}

/// How alerts are, as the indicator shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlertState {
    Off,
    On,
    /// On, but the platform hasn't been asked yet: a click asks.
    NeedsPermission,
    /// On, but the platform refuses them.
    Blocked,
    Unavailable(&'static str),
}

/// What clicking an alert opens: its server, and it.
type Clicked = Arc<Mutex<Option<(String, NotificationId)>>>;

/// A way to raise alerts: the browser's, the OS's.
pub trait Backend: Send + Sync {
    fn permission(&self) -> Permission;
    /// Asks the platform to allow them (call it from the user's click).
    fn ask(&self);
    /// The cockpit is out of sight (a hidden tab, a window in the background).
    fn out_of_sight(&self) -> bool;
    /// Raises one; its click is put in `clicked`.
    fn raise(&self, server: &str, notification: &Notification, clicked: Clicked);
}

#[derive(Clone)]
pub struct Alerts {
    on: Arc<AtomicBool>,
    clicked: Clicked,
    backend: Arc<dyn Backend>,
}

impl Default for Alerts {
    /// The build's own backend.
    fn default() -> Self {
        #[cfg(target_arch = "wasm32")]
        let backend: Arc<dyn Backend> = Arc::new(web::Web);
        #[cfg(not(target_arch = "wasm32"))]
        let backend: Arc<dyn Backend> = Arc::new(native::Native);
        Self::with(backend)
    }
}

impl Alerts {
    pub fn with(backend: Arc<dyn Backend>) -> Self {
        Self { on: Arc::default(), clicked: Arc::default(), backend }
    }

    pub fn on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    pub fn state(&self) -> AlertState {
        match (self.on(), self.backend.permission()) {
            (_, Permission::Unavailable(why)) => AlertState::Unavailable(why),
            (false, _) => AlertState::Off,
            (true, Permission::Granted) => AlertState::On,
            (true, Permission::Ask) => AlertState::NeedsPermission,
            (true, Permission::Denied) => AlertState::Blocked,
        }
    }

    /// As kept: on or off, asking nothing (no click to ask with).
    pub fn restore(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
    }

    /// The user's click: off turns them on (asking the platform when it
    /// hasn't been), on turns them off; still needing permission, asks.
    pub fn toggle(&self) {
        match self.state() {
            AlertState::Off => {
                self.on.store(true, Ordering::Relaxed);
                if self.backend.permission() == Permission::Ask {
                    self.backend.ask();
                }
            }
            AlertState::NeedsPermission => self.backend.ask(),
            AlertState::On | AlertState::Blocked => self.on.store(false, Ordering::Relaxed),
            AlertState::Unavailable(_) => {}
        }
    }

    /// The item an alert's click asked for.
    pub fn take_clicked(&self) -> Option<(String, NotificationId)> {
        self.clicked.lock().ok()?.take()
    }

    /// For each of home's events as it arrives: a new waiting item, while
    /// the cockpit is out of sight, is raised.
    pub fn arrival(&self) -> impl Fn(&str) + Send + Sync + 'static {
        let (on, clicked, backend) = (self.on.clone(), self.clicked.clone(), self.backend.clone());
        move |text: &str| {
            if !on.load(Ordering::Relaxed) || !text.contains("notification_updated") || backend.permission() != Permission::Granted || !backend.out_of_sight() {
                return;
            }
            let Ok(HomeEvent::Event { server, event }) = serde_json::from_str::<HomeEvent>(text) else { return };
            if let ClientEvent::NotificationUpdated { notification } = *event
                && notification.level == Level::Waiting
                && notification.state == NotificationState::Open
            {
                backend.raise(&server, &notification, clicked.clone());
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A backend that records what it raised, with its permission and sight as set.
    #[derive(Default)]
    pub(crate) struct Recorded {
        pub permission: Mutex<Option<Permission>>,
        pub hidden: AtomicBool,
        pub asked: AtomicBool,
        pub raised: Mutex<Vec<String>>,
    }

    impl Backend for Recorded {
        fn permission(&self) -> Permission {
            self.permission.lock().unwrap().clone().unwrap_or(Permission::Ask)
        }
        fn ask(&self) {
            self.asked.store(true, Ordering::Relaxed);
        }
        fn out_of_sight(&self) -> bool {
            self.hidden.load(Ordering::Relaxed)
        }
        fn raise(&self, server: &str, n: &Notification, clicked: Clicked) {
            self.raised.lock().unwrap().push(format!("{server}/{}", n.id));
            *clicked.lock().unwrap() = Some((server.into(), n.id.clone()));
        }
    }

    fn waiting(state: &str) -> String {
        format!(r#"{{"type":"event","server":"cloud","event":{{"type":"notification_updated","notification":{{"id":"req:1","level":"waiting","kind":"approval","slot":null,"thread":null,"request":null,"title":"Run it","detail":null,"atMs":1,"state":"{state}"}}}}}}"#)
    }

    #[test]
    fn a_click_turns_them_on_asking_once_and_only_whats_new_and_waiting_is_raised_out_of_sight() {
        let backend = Arc::new(Recorded::default());
        let alerts = Alerts::with(backend.clone());
        let arrive = alerts.arrival();
        assert_eq!(alerts.state(), AlertState::Off);
        alerts.toggle();
        assert!(backend.asked.load(Ordering::Relaxed), "turned on: the platform is asked");
        assert_eq!(alerts.state(), AlertState::NeedsPermission, "until it allows them");
        arrive(&waiting("open"));
        assert!(backend.raised.lock().unwrap().is_empty(), "not allowed: nothing raised");
        *backend.permission.lock().unwrap() = Some(Permission::Granted);
        assert_eq!(alerts.state(), AlertState::On);
        arrive(&waiting("open"));
        assert!(backend.raised.lock().unwrap().is_empty(), "in sight: the cockpit shows it itself");
        backend.hidden.store(true, Ordering::Relaxed);
        arrive(&waiting("done"));
        arrive(&waiting("open"));
        assert_eq!(*backend.raised.lock().unwrap(), ["cloud/req:1"], "only what newly waits");
        assert_eq!(alerts.take_clicked(), Some(("cloud".into(), NotificationId::new("req:1"))), "its click opens it");
        *backend.permission.lock().unwrap() = Some(Permission::Denied);
        assert_eq!(alerts.state(), AlertState::Blocked, "refused: said, not silent");
        alerts.toggle();
        assert_eq!(alerts.state(), AlertState::Off);
        alerts.restore(true);
        assert_eq!(alerts.state(), AlertState::Blocked, "restored as kept, nothing asked");
    }
}
