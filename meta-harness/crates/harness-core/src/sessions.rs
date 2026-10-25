//! Live sessions by thread. Opens them through the owning instance's adapter,
//! forwards their events to the hub (a message of the user's with what the
//! user typed for it, when that was kept), and folds catalog changes into
//! the instance.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use futures::StreamExt;
use harness_contracts::{
    AdapterEvent, AsTyped, ClientEvent, InstanceId, ItemBody, McpServer, ModelSelection, NativeId, RuntimePolicy, SessionConfig,
    ThreadId,
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
pub type Typed = Arc<dyn Fn(&str) -> Option<AsTyped> + Send + Sync>;

pub struct SessionManager {
    registry: Arc<InstanceRegistry>,
    hub: Arc<EventHub>,
    sessions: RwLock<HashMap<ThreadId, Arc<dyn SessionRuntime>>>,
    /// Where what the user typed is read (each event reads it as it is).
    typed: Arc<OnceLock<Typed>>,
}

/// The text sent of `event`'s message of the user's, when what was typed
/// for it isn't with it (a replayed history's, or one typed as sent): what
/// was typed, if it was kept, is looked up by it.
fn typed_wanted(event: &AdapterEvent) -> Option<String> {
    match event {
        AdapterEvent::ItemUpdated { item } => match &item.body {
            ItemBody::UserMessage { text, typed: None } => Some(text.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// `event`'s message of the user's with what was typed for it.
fn with_typed(mut event: AdapterEvent, found: Option<AsTyped>) -> AdapterEvent {
    if let AdapterEvent::ItemUpdated { item } = &mut event
        && let ItemBody::UserMessage { typed, .. } = &mut item.body
    {
        *typed = found;
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
                // (What was typed, read off the runtime's threads.)
                let event = match typed_wanted(&event).zip(typed.get().cloned()) {
                    Some((sent, read)) => {
                        let found = tokio::task::spawn_blocking(move || read(&sent)).await.ok().flatten();
                        with_typed(event, found)
                    }
                    None => event,
                };
                hub.publish(ClientEvent::Session { event });
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

    fn typed_of(event: &AdapterEvent) -> Option<&AsTyped> {
        match event {
            AdapterEvent::ItemUpdated { item } => match &item.body {
                ItemBody::UserMessage { typed, .. } => typed.as_ref(),
                _ => None,
            },
            _ => None,
        }
    }

    /// A replayed message is looked up by its text (one with what was
    /// typed already isn't); it gets what's found.
    #[test]
    fn a_replayed_message_gets_what_was_typed() {
        let sent = "look\n\n- a thing";
        let typed = AsTyped { message: harness_contracts::Message::text("look"), handed_on: Some("- a thing".into()) };
        assert_eq!(typed_wanted(&said(sent)), Some(sent.to_owned()));
        let mut known = said(sent);
        if let AdapterEvent::ItemUpdated { item } = &mut known {
            item.body = ItemBody::UserMessage { text: sent.into(), typed: Some(typed.clone()) };
        }
        assert_eq!(typed_wanted(&known), None, "live: what was typed came with it");
        assert_eq!(typed_of(&with_typed(said(sent), Some(typed.clone()))), Some(&typed));
    }
}
