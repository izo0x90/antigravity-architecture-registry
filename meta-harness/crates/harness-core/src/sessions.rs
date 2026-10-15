//! Live sessions by thread. Opens them through the owning instance's adapter,
//! forwards their events to the hub, and folds catalog changes into the instance.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use futures::StreamExt;
use harness_contracts::{
    AdapterEvent, ClientEvent, InstanceId, McpServer, ModelSelection, NativeId, RuntimePolicy, SessionConfig,
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

pub struct SessionManager {
    registry: Arc<InstanceRegistry>,
    hub: Arc<EventHub>,
    sessions: RwLock<HashMap<ThreadId, Arc<dyn SessionRuntime>>>,
}

impl SessionManager {
    pub fn new(registry: Arc<InstanceRegistry>, hub: Arc<EventHub>) -> Self {
        Self { registry, hub, sessions: RwLock::default() }
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
        tokio::spawn(async move {
            while let Some(event) = events.next().await {
                if let AdapterEvent::CatalogUpdated { update } = &event {
                    instance.apply_catalog(update);
                }
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
