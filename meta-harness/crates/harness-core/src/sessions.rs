//! Live sessions by thread. Opens them through the owning instance's adapter,
//! forwards their events to the hub (a message of the user's with what the
//! user typed for it, when that was kept), and folds catalog changes into
//! the instance.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use futures::StreamExt;
use harness_contracts::{
    AdapterEvent, ClientEvent, InstanceId, ItemBody, McpServer, Message, ModelSelection, NativeId, RuntimePolicy, SessionConfig,
    ThreadId, HANDED_ON,
};

use crate::{EventHub, HarnessError, InstanceRegistry, Result, SessionRuntime};

#[derive(Debug, Clone)]
pub struct OpenThread {
    pub instance_id: InstanceId,
    pub policy: RuntimePolicy,
    pub model: Option<ModelSelection>,
    pub resume: Option<NativeId>,
    /// The tool servers this thread's session gets.
    pub mcp_servers: Vec<McpServer>,
}

/// What the user typed for a message as an agent was handed it, when it
/// was kept.
pub type Typed = Arc<dyn Fn(&str) -> Option<Message> + Send + Sync>;

pub struct SessionManager {
    registry: Arc<InstanceRegistry>,
    hub: Arc<EventHub>,
    sessions: RwLock<HashMap<ThreadId, Arc<dyn SessionRuntime>>>,
    /// Where what the user typed is read (each event reads it as it is).
    typed: Arc<OnceLock<Typed>>,
}

/// `event`, a message of the user's sent with what it links given what
/// was typed for it (`typed`), when that was kept.
fn with_typed(mut event: AdapterEvent, typed: Option<&Typed>) -> AdapterEvent {
    if let (AdapterEvent::ItemUpdated { item }, Some(typed)) = (&mut event, typed)
        && let ItemBody::UserMessage { text, typed: unknown @ None } = &mut item.body
        && text.contains(HANDED_ON)
    {
        *unknown = typed(text);
    }
    event
}

impl SessionManager {
    pub fn new(registry: Arc<InstanceRegistry>, hub: Arc<EventHub>) -> Self {
        Self { registry, hub, sessions: RwLock::default(), typed: Arc::default() }
    }

    /// Where what the user typed for a message is read from now on.
    pub fn read_typed_with(&self, typed: Typed) {
        let _ = self.typed.set(typed);
    }

    pub async fn open(&self, request: OpenThread) -> Result<ThreadId> {
        let instance = self.registry.get(&request.instance_id)?;
        let thread_id = ThreadId::new(uuid::Uuid::new_v4().to_string());
        let config = SessionConfig {
            thread_id: thread_id.clone(),
            policy: request.policy,
            model: request.model,
            resume: request.resume,
            mcp_servers: request.mcp_servers,
        };
        let session: Arc<dyn SessionRuntime> = instance.adapter().open_session(config).await?.into();

        let mut events = session.events();
        let hub = self.hub.clone();
        let closed_id = thread_id.clone();
        let typed = self.typed.clone();
        tokio::spawn(async move {
            while let Some(event) = events.next().await {
                if let AdapterEvent::CatalogUpdated { update } = &event {
                    instance.apply_catalog(update);
                }
                hub.publish(ClientEvent::Session { event: with_typed(event, typed.get()) });
            }
            hub.publish(ClientEvent::ThreadClosed { thread_id: closed_id });
        });

        self.write().insert(thread_id.clone(), session);
        Ok(thread_id)
    }

    pub fn get(&self, thread_id: &ThreadId) -> Result<Arc<dyn SessionRuntime>> {
        self.sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(thread_id)
            .cloned()
            .ok_or_else(|| HarnessError::NotFound { what: format!("thread {thread_id}") })
    }

    pub async fn close(&self, thread_id: &ThreadId) -> Result<()> {
        let session = self
            .write()
            .remove(thread_id)
            .ok_or_else(|| HarnessError::NotFound { what: format!("thread {thread_id}") })?;
        session.close().await?;
        self.hub.publish(ClientEvent::ThreadClosed { thread_id: thread_id.clone() });
        Ok(())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<ThreadId, Arc<dyn SessionRuntime>>> {
        self.sessions.write().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_contracts::{ItemId, ItemStatus, TurnId, TurnItem};

    fn said(text: &str) -> AdapterEvent {
        AdapterEvent::ItemUpdated { item: TurnItem { id: ItemId::new("i"), thread_id: ThreadId::new("t"), turn_id: TurnId::new("u"), native_id: None, parent_id: None, ordinal: 0, status: ItemStatus::Completed, title: None, body: ItemBody::UserMessage { text: text.into(), typed: None } } }
    }

    fn typed_of(event: &AdapterEvent) -> Option<&Message> {
        match event {
            AdapterEvent::ItemUpdated { item } => match &item.body {
                ItemBody::UserMessage { typed, .. } => typed.as_ref(),
                _ => None,
            },
            _ => None,
        }
    }

    /// A message sent with what it links gets what was typed for it, when
    /// that was kept; one without isn't looked up.
    #[test]
    fn a_message_sent_with_its_links_gets_what_was_typed() {
        let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = asked.clone();
        let typed: Typed = Arc::new(move |sent: &str| {
            seen.lock().unwrap().push(sent.to_owned());
            sent.starts_with("look").then(|| Message::text("look"))
        });
        let sent = format!("look{HANDED_ON}\n\n#x\n- a thing");
        assert_eq!(typed_of(&with_typed(said(&sent), Some(&typed))), Some(&Message::text("look")));
        assert_eq!(typed_of(&with_typed(said("plain"), Some(&typed))), None);
        assert_eq!(*asked.lock().unwrap(), [sent], "a plain message isn't looked up");
    }
}
