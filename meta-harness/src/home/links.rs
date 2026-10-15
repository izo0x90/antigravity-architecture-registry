//! Home's links: one task per server, following its event stream (with
//! its secret) into an event hub of its own. Each (re)connect starts that
//! hub over from the server's replay, so nothing stale survives. A server
//! that stops answering is tried again, waiting longer each time, and said
//! once as a notification until it's back.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use harness_contracts::{ClientEvent, LinkState, ServerView};
use harness_core::EventHub;
use nah_state::ServerEntry;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

/// Reconnects: the first wait, and the most it doubles up to.
const BACKOFF: (Duration, Duration) = (Duration::from_millis(500), Duration::from_secs(10));

struct Link {
    entry: ServerEntry,
    hub: Arc<EventHub>,
    state: Arc<Mutex<LinkState>>,
    task: Option<tokio::task::AbortHandle>,
}

pub struct Links {
    links: Mutex<BTreeMap<String, Link>>,
    notify: Arc<nah_notify::NotifyHub>,
    /// Bumped whenever a server or its link changes.
    changed: tokio::sync::watch::Sender<u64>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Links {
    /// Links to each server, following it now.
    pub fn start(entries: Vec<ServerEntry>, notify: Arc<nah_notify::NotifyHub>) -> Arc<Self> {
        let links = Arc::new(Self { links: Mutex::default(), notify, changed: tokio::sync::watch::channel(0).0 });
        for entry in entries {
            links.add(entry);
        }
        links
    }

    fn bump(&self) {
        self.changed.send_modify(|n| *n += 1);
    }

    /// Something changed (servers or links) since `seen`.
    pub fn watch(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// A server added: followed from now on.
    pub fn add(self: &Arc<Self>, entry: ServerEntry) {
        let name = entry.name.clone();
        lock(&self.links).insert(name.clone(), Link { entry, hub: Arc::new(EventHub::default()), state: Arc::new(Mutex::new(LinkState::Connecting)), task: None });
        self.connect(&name);
    }

    /// A server removed: no longer followed.
    pub fn remove(&self, name: &str) {
        if let Some(link) = lock(&self.links).remove(name)
            && let Some(task) = link.task
        {
            task.abort();
        }
        if let Err(e) = self.notify.server_back(name) {
            tracing::warn!("{name}: its lost notification couldn't be closed: {e}");
        }
        self.bump();
    }

    /// Follows the server again (a running link is left as it is). False
    /// for a server there isn't.
    pub fn connect(self: &Arc<Self>, name: &str) -> bool {
        let mut links = lock(&self.links);
        let Some(link) = links.get_mut(name) else { return false };
        if link.task.as_ref().is_some_and(|t| !t.is_finished()) {
            return true;
        }
        *lock(&link.state) = LinkState::Connecting;
        let (entry, hub, state, me) = (link.entry.clone(), link.hub.clone(), link.state.clone(), Arc::downgrade(self));
        link.task = Some(tokio::spawn(follow(entry, hub, state, me)).abort_handle());
        drop(links);
        self.bump();
        true
    }

    /// Stops following it; the server runs on, and stays listed. False for
    /// a server there isn't.
    pub fn disconnect(&self, name: &str) -> bool {
        let mut links = lock(&self.links);
        let Some(link) = links.get_mut(name) else { return false };
        if let Some(task) = link.task.take() {
            task.abort();
        }
        *lock(&link.state) = LinkState::Disconnected;
        drop(links);
        if let Err(e) = self.notify.server_back(name) {
            tracing::warn!("{name}: its lost notification couldn't be closed: {e}");
        }
        self.bump();
        true
    }

    /// Each server's event hub.
    pub fn hubs(&self) -> Vec<(String, Arc<EventHub>)> {
        lock(&self.links).iter().map(|(name, link)| (name.clone(), link.hub.clone())).collect()
    }

    /// The servers and their links (no secrets).
    pub fn list(&self) -> Vec<ServerView> {
        lock(&self.links).values().map(|l| ServerView { name: l.entry.name.clone(), url: l.entry.url.clone(), link: lock(&l.state).clone(), project: true }).collect()
    }

    /// Where a server is, and its secret.
    pub fn entry(&self, name: &str) -> Option<ServerEntry> {
        lock(&self.links).get(name).map(|l| l.entry.clone())
    }

    fn set(&self, name: &str, state: &Arc<Mutex<LinkState>>, to: LinkState) {
        let changed = {
            let mut now = lock(state);
            let changed = *now != to;
            *now = to.clone();
            changed
        };
        let said = match &to {
            LinkState::Connected => self.notify.server_back(name),
            LinkState::Lost { reason } => self.notify.server_lost(name, reason),
            _ => Ok(()),
        };
        if let Err(e) = said {
            tracing::warn!("{name}: {e}");
        }
        if changed {
            self.bump();
        }
    }
}

/// Follows one server until stopped: connects, takes its replay and live
/// events into `hub`, and after a drop tries again.
async fn follow(entry: ServerEntry, hub: Arc<EventHub>, state: Arc<Mutex<LinkState>>, links: std::sync::Weak<Links>) {
    let mut wait = BACKOFF.0;
    loop {
        let reason = match stream(&entry, &hub, &state, &links).await {
            Ok(()) => "it closed the connection".to_owned(),
            Err(e) => e,
        };
        let Some(me) = links.upgrade() else { return };
        me.set(&entry.name, &state, LinkState::Lost { reason });
        drop(me);
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(BACKOFF.1);
        if let Some(me) = links.upgrade() {
            me.set(&entry.name, &state, LinkState::Connecting);
        }
    }
}

/// One connection: until it ends. Its notifications are said to be from
/// it, and handed to home's adapters (its rules ran there already).
async fn stream(entry: &ServerEntry, hub: &Arc<EventHub>, state: &Arc<Mutex<LinkState>>, links: &std::sync::Weak<Links>) -> Result<(), String> {
    let url = format!("{}/ws/events", entry.url.replacen("http://", "ws://", 1));
    let mut request = url.as_str().into_client_request().map_err(|e| format!("{url}: {e}"))?;
    let bearer = format!("Bearer {}", entry.secret).parse().map_err(|_| "its secret isn't a header value".to_owned())?;
    request.headers_mut().insert("Authorization", bearer);
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.map_err(|e| e.to_string())?;
    hub.reset();
    if let Some(me) = links.upgrade() {
        me.set(&entry.name, state, LinkState::Connected);
    }
    while let Some(message) = socket.next().await {
        let text = match message.map_err(|e| e.to_string())? {
            Message::Text(text) => text,
            Message::Close(_) => return Ok(()),
            _ => continue,
        };
        let mut event: ClientEvent = serde_json::from_str(&text).map_err(|e| format!("an event home can't read ({e})"))?;
        match &mut event {
            // It fell behind on its side: its whole state follows.
            ClientEvent::Resync => {
                hub.reset();
                continue;
            }
            ClientEvent::NotificationUpdated { notification } => {
                notification.server = Some(entry.name.clone());
                if let Some(me) = links.upgrade() {
                    me.notify.relay(notification);
                }
            }
            _ => {}
        }
        hub.publish(event);
    }
    Ok(())
}
