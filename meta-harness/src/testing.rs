use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use futures::future::BoxFuture;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex, Notify};

use crate::adapter::{AcpRuntime, AntigravityAdapter};
use crate::error::HarnessError;
use crate::protocol::*;

pub struct NativePrompt {
    pub index: usize,
    pub content: String,
    result_tx: Arc<Mutex<Option<oneshot::Sender<PromptResponse>>>>,
}

impl NativePrompt {
    pub async fn resolve(self, response: PromptResponse) {
        if let Some(tx) = self.result_tx.lock().await.take() {
            let _ = tx.send(response);
        }
    }
}

pub struct PermissionHandle {
    rx: oneshot::Receiver<PermissionOutcome>,
    resolved: Option<PermissionOutcome>,
}

impl PermissionHandle {
    pub fn is_resolved(&mut self) -> bool {
        if self.resolved.is_some() {
            return true;
        }
        match self.rx.try_recv() {
            Ok(outcome) => {
                self.resolved = Some(outcome);
                true
            }
            _ => false,
        }
    }

    pub async fn await_outcome(mut self) -> Result<PermissionOutcome, HarnessError> {
        if let Some(outcome) = self.resolved.take() {
            return Ok(outcome);
        }
        self.rx.await.map_err(|_| HarnessError::Cancelled)
    }
}

pub struct MockControls {
    pub fail_model: bool,
    pub fail_auth: bool,
    pub auth_invalidations: usize,
    pub closed: usize,
}

pub struct HarnessOptions {
    pub enabled: bool,
    pub hold_cancel: bool,
    pub hold_close: bool,
    pub hold_dispatch: bool,
}

impl Default for HarnessOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            hold_cancel: false,
            hold_close: false,
            hold_dispatch: false,
        }
    }
}

pub type ActivePromptSlot = Arc<Mutex<Option<oneshot::Sender<PromptResponse>>>>;
pub type ActivePromptRecord = Arc<Mutex<Option<(usize, ActivePromptSlot)>>>;
pub type PermissionHandler =
    Arc<dyn Fn(PermissionRequest) -> BoxFuture<'static, PermissionOutcome> + Send + Sync>;

pub struct MockAcpRuntime {
    pub calls: Arc<Mutex<Vec<String>>>,
    pub events_tx: broadcast::Sender<NativeEvent>,
    pub prompts_tx: mpsc::Sender<NativePrompt>,
    pub active_prompt: ActivePromptRecord,
    pub prompt_index: AtomicUsize,
    pub controls: Arc<Mutex<MockControls>>,
    pub permission_handler: Arc<Mutex<Option<PermissionHandler>>>,
    pub hold_cancel: AtomicBool,
    cancel_release: Arc<Notify>,
    current_model: Arc<Mutex<String>>,
    cancellations_tx: mpsc::Sender<usize>,
    pub command_updates: Arc<Mutex<Vec<Vec<String>>>>,
}

#[async_trait::async_trait]
impl AcpRuntime for MockAcpRuntime {
    async fn start(&self) -> Result<String, HarnessError> {
        let controls = self.controls.lock().await;
        if controls.fail_auth {
            return Err(HarnessError::Validation("Sign-in required".to_string()));
        }
        *self.current_model.lock().await = "gemini-test-low".to_string();
        self.calls.lock().await.push("start".to_string());
        let commands = vec!["plan".to_string(), "logout".to_string()];
        self.command_updates.lock().await.push(commands.clone());
        let _ = self.events_tx.send(NativeEvent::AvailableCommandsUpdated {
            commands,
        });
        Ok("b75db7e9-cd99-40e5-aa63-ac2b4674a6a9".to_string())
    }

    async fn set_model(&self, model: &str) -> Result<(), HarnessError> {
        self.calls.lock().await.push(format!("model:{}", model));
        let mut controls = self.controls.lock().await;
        if controls.fail_model {
            controls.fail_model = false;
            return Err(HarnessError::Validation("Native model selection failed.".to_string()));
        }
        *self.current_model.lock().await = model.to_string();
        Ok(())
    }

    async fn set_mode(&self, mode: &str) -> Result<(), HarnessError> {
        self.calls.lock().await.push(format!("mode:{}", mode));
        Ok(())
    }

    async fn prompt(&self, text: &str) -> Result<PromptResponse, HarnessError> {
        let idx = self.prompt_index.fetch_add(1, Ordering::SeqCst) + 1;
        self.calls.lock().await.push(format!("prompt:{}", idx));
        let (tx, rx) = oneshot::channel();
        let result_cell = Arc::new(Mutex::new(Some(tx)));

        *self.active_prompt.lock().await = Some((idx, result_cell.clone()));

        let native_prompt = NativePrompt {
            index: idx,
            content: text.to_string(),
            result_tx: result_cell,
        };

        let _ = self.prompts_tx.send(native_prompt).await;

        let res = rx.await;
        *self.active_prompt.lock().await = None;
        res.map_err(|_| HarnessError::Cancelled)
    }

    async fn cancel(&self) -> Result<(), HarnessError> {
        let active = self.active_prompt.lock().await.take();
        if let Some((idx, cell)) = active {
            self.calls.lock().await.push(format!("cancel:{}", idx));
            let _ = self.cancellations_tx.send(idx).await;
            if self.hold_cancel.load(Ordering::SeqCst) {
                self.cancel_release.notified().await;
            }
            if let Some(tx) = cell.lock().await.take() {
                let _ = tx.send(PromptResponse {
                    stop_reason: "cancelled".to_string(),
                });
            }
            self.drain_events().await?;
            self.calls.lock().await.push(format!("drained:{}", idx));
        }
        Ok(())
    }

    async fn drain_events(&self) -> Result<(), HarnessError> {
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        Ok(())
    }

    fn subscribe_events(&self) -> broadcast::Receiver<NativeEvent> {
        self.events_tx.subscribe()
    }

    fn register_permission_handler(&self, handler: Arc<dyn Fn(PermissionRequest) -> BoxFuture<'static, PermissionOutcome> + Send + Sync>) {
        let slot = self.permission_handler.clone();
        tokio::spawn(async move {
            *slot.lock().await = Some(handler);
        });
    }
}

pub struct AdapterTestHarness {
    pub adapter: AntigravityAdapter,
    pub calls: Arc<Mutex<Vec<String>>>,
    pub seen: Arc<Mutex<Vec<ProviderRuntimeEvent>>>,
    pub controls: Arc<Mutex<MockControls>>,
    pub cancel_release: Arc<Notify>,
    pub launches: Arc<Mutex<Vec<StartSessionInput>>>,
    pub command_updates: Arc<Mutex<Vec<Vec<String>>>>,
    runtime: Arc<MockAcpRuntime>,
    prompts_rx: Arc<Mutex<mpsc::Receiver<NativePrompt>>>,
    canonical_events_rx: Arc<Mutex<mpsc::Receiver<ProviderRuntimeEvent>>>,
    cancellations_rx: Arc<Mutex<mpsc::Receiver<usize>>>,
}

impl AdapterTestHarness {
    pub async fn new() -> Self {
        Self::new_with_options(HarnessOptions::default()).await
    }

    pub async fn new_with_options(options: HarnessOptions) -> Self {
        let (events_tx, _) = broadcast::channel(1024);
        let (prompts_tx, prompts_rx) = mpsc::channel(100);
        let (cancellations_tx, cancellations_rx) = mpsc::channel(100);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let controls = Arc::new(Mutex::new(MockControls {
            fail_model: false,
            fail_auth: false,
            auth_invalidations: 0,
            closed: 0,
        }));
        let cancel_release = Arc::new(Notify::new());
        let launches = Arc::new(Mutex::new(Vec::new()));
        let command_updates = Arc::new(Mutex::new(Vec::new()));

        let runtime = Arc::new(MockAcpRuntime {
            calls: calls.clone(),
            events_tx,
            prompts_tx,
            active_prompt: Arc::new(Mutex::new(None)),
            prompt_index: AtomicUsize::new(0),
            controls: controls.clone(),
            permission_handler: Arc::new(Mutex::new(None)),
            hold_cancel: AtomicBool::new(options.hold_cancel),
            cancel_release: cancel_release.clone(),
            current_model: Arc::new(Mutex::new("gemini-test-low".to_string())),
            cancellations_tx,
            command_updates: command_updates.clone(),
        });

        let rt_for_factory = runtime.clone();
        let launches_clone = launches.clone();
        let adapter = AntigravityAdapter::with_runtime_factory(Arc::new(move |input| {
            let mut l = launches_clone.try_lock().expect("lock launches");
            l.push(input.clone());
            Ok(rt_for_factory.clone() as Arc<dyn AcpRuntime>)
        }));

        let (canonical_tx, canonical_rx) = mpsc::channel(1024);
        let seen = Arc::new(Mutex::new(Vec::new()));

        let mut sub = adapter.subscribe();
        let seen_clone = seen.clone();
        tokio::spawn(async move {
            while let Ok(event) = sub.recv().await {
                seen_clone.lock().await.push(event.clone());
                let _ = canonical_tx.send(event).await;
            }
        });

        Self {
            adapter,
            calls,
            seen,
            controls,
            cancel_release,
            launches,
            command_updates,
            runtime,
            prompts_rx: Arc::new(Mutex::new(prompts_rx)),
            canonical_events_rx: Arc::new(Mutex::new(canonical_rx)),
            cancellations_rx: Arc::new(Mutex::new(cancellations_rx)),
        }
    }

    pub async fn next_cancellation(&self) -> usize {
        self.cancellations_rx.lock().await.recv().await.expect("cancellation")
    }

    pub async fn emit_native(&self, event: NativeEvent) {
        let _ = self.runtime.events_tx.send(event);
    }

    pub async fn invoke_permission(&self, request: PermissionRequest) -> PermissionHandle {
        let (tx, rx) = oneshot::channel();
        // Wait until permission handler is registered
        let handler = loop {
            if let Some(h) = self.runtime.permission_handler.lock().await.clone() {
                break h;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
        };

        tokio::spawn(async move {
            let outcome = handler(request).await;
            let _ = tx.send(outcome);
        });

        PermissionHandle { rx, resolved: None }
    }

    pub async fn next_prompt(&self) -> NativePrompt {
        self.prompts_rx.lock().await.recv().await.expect("next prompt")
    }

    pub async fn wait_for_event<F>(&self, predicate: F) -> ProviderRuntimeEvent
    where
        F: Fn(&ProviderRuntimeEvent) -> bool,
    {
        let mut rx = self.canonical_events_rx.lock().await;
        loop {
            let event = rx.recv().await.expect("valid canonical event");
            if predicate(&event) {
                return event;
            }
        }
    }

    pub async fn has_active_prompt(&self) -> bool {
        self.runtime.active_prompt.lock().await.is_some()
    }

    pub fn release_cancel(&self) {
        self.cancel_release.notify_waiters();
    }

    pub async fn drain_events(&self) {
        tokio::time::sleep(tokio::time::Duration::from_millis(25)).await;
    }
}
