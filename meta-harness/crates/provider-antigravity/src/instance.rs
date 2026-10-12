//! One configured Antigravity: live snapshot (status, auth, catalog, skills),
//! login, saved sessions, and the ACP adapter for its sessions.

use std::path::PathBuf;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use harness_acp::{AcpAdapter, AcpAuthController, AcpLauncher, AcpSessionCatalog, ProcessLauncher, probe};
use harness_contracts::{
    AuthState, AuthStatus, CatalogUpdate, DriverKind, InstanceId, ProviderSnapshot, ProviderStatus, RuntimeMode, Skill,
};
use harness_core::{AuthController, ProviderAdapter, ProviderInstance, Result, SessionCatalog};
use tokio::sync::watch;

use crate::driver::AntigravityConfig;
use crate::flavor::AntigravityFlavor;
use crate::skills::{DiscoverSkillsInput, discover_antigravity_skills};

pub(crate) const DRIVER_KIND: &str = "antigravity";

/// The launcher of a server that can't be started: it says why.
struct Unavailable(String);

#[async_trait]
impl AcpLauncher for Unavailable {
    async fn launch(&self, _config: &harness_contracts::SessionConfig) -> Result<harness_acp::AcpTransport> {
        Err(harness_core::HarnessError::Provider { reason: self.0.clone() })
    }
}

pub struct AntigravityInstance {
    id: InstanceId,
    /// Why its server can't be started (no launch), if it can't.
    unavailable: Option<String>,
    launcher: Arc<dyn AcpLauncher>,
    adapter: Arc<AcpAdapter>,
    auth: Arc<AcpAuthController>,
    sessions: Arc<AcpSessionCatalog>,
    snapshot: watch::Sender<ProviderSnapshot>,
}

impl AntigravityInstance {
    pub(crate) fn create(id: InstanceId, config: AntigravityConfig) -> Arc<Self> {
        let unavailable = match &config.launch {
            Some(_) => None,
            None => Some(config.unavailable.clone().unwrap_or_else(|| "no launch configured for the Antigravity ACP server".to_owned())),
        };
        // No launch: anything that would start the server says why instead.
        let launcher: Arc<dyn AcpLauncher> = match (config.launch, &unavailable) {
            (Some(launch), _) => Arc::new(ProcessLauncher { program: PathBuf::from(launch.program), args: launch.args, env: launch.env }),
            (None, why) => Arc::new(Unavailable(why.clone().unwrap_or_default())),
        };
        let flavor = Arc::new(AntigravityFlavor);
        let adapter = Arc::new(AcpAdapter::new(id.clone(), flavor, launcher.clone()));
        let (snapshot, _) = watch::channel(ProviderSnapshot {
            instance_id: id.clone(),
            driver: DriverKind::new(DRIVER_KIND),
            display_name: config.display_name.unwrap_or_else(|| "Antigravity".into()),
            status: ProviderStatus::Starting,
            auth: AuthState { status: AuthStatus::Unknown, account: None, methods: Vec::new() },
            capabilities: adapter.capabilities(),
            models: Vec::new(),
            commands: Vec::new(),
            skills: Vec::new(),
            runtime_modes: vec![RuntimeMode::ApprovalRequired, RuntimeMode::AutoAcceptEdits, RuntimeMode::FullAccess],
            supports_plan_mode: false,
        });

        Arc::new_cyclic(|me: &Weak<Self>| {
            let me = me.clone();
            let on_auth = Arc::new(move |signed_in: bool| {
                if let Some(instance) = me.upgrade() {
                    instance.snapshot.send_modify(|s| {
                        s.auth.status = if signed_in { AuthStatus::SignedIn } else { AuthStatus::SignedOut };
                    });
                }
            });
            Self {
                id,
                unavailable,
                auth: Arc::new(AcpAuthController::new(launcher.clone(), on_auth)),
                sessions: Arc::new(AcpSessionCatalog::new(launcher.clone())),
                launcher,
                adapter,
                snapshot,
            }
        })
    }

    fn discover_skills(&self) -> Vec<Skill> {
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
        let cwd = std::env::current_dir().unwrap_or_default();
        match discover_antigravity_skills(&DiscoverSkillsInput { cwd, user_home: home }) {
            Ok(found) => found
                .into_iter()
                .filter(|s| s.enabled)
                .map(|s| Skill { name: s.name, description: s.description, path: Some(s.path) })
                .collect(),
            Err(err) => {
                tracing::warn!(target: "provider_antigravity", "skill discovery failed: {err}");
                Vec::new()
            }
        }
    }
}

#[async_trait]
impl ProviderInstance for AntigravityInstance {
    fn id(&self) -> &InstanceId {
        &self.id
    }

    fn snapshot(&self) -> watch::Receiver<ProviderSnapshot> {
        self.snapshot.subscribe()
    }

    async fn refresh(&self) -> Result<()> {
        if let Some(reason) = &self.unavailable {
            self.snapshot.send_modify(|s| s.status = ProviderStatus::Unavailable { reason: reason.clone() });
            return Ok(());
        }
        let skills = self.discover_skills();
        match probe(&self.launcher).await {
            Ok(info) => self.snapshot.send_modify(|s| {
                s.status = ProviderStatus::Ready;
                s.auth.methods = info.auth_methods;
                info.capabilities.apply(&mut s.capabilities);
                s.skills = skills;
            }),
            Err(err) => self.snapshot.send_modify(|s| s.status = ProviderStatus::Error { message: err.to_string() }),
        }
        Ok(())
    }

    fn auth(&self) -> Option<Arc<dyn AuthController>> {
        Some(self.auth.clone())
    }

    fn adapter(&self) -> Arc<dyn ProviderAdapter> {
        self.adapter.clone()
    }

    fn sessions(&self) -> Option<Arc<dyn SessionCatalog>> {
        Some(self.sessions.clone())
    }

    fn apply_catalog(&self, update: &CatalogUpdate) {
        self.snapshot.send_modify(|s| {
            if let Some(commands) = &update.commands {
                s.commands = commands.clone();
            }
            if let Some(models) = &update.models {
                s.models = models.clone();
            }
            // A session that reports a catalog is signed in.
            s.auth.status = AuthStatus::SignedIn;
        });
    }
}
