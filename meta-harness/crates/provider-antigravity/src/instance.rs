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

pub struct AntigravityInstance {
    id: InstanceId,
    executable: PathBuf,
    launcher: Arc<dyn AcpLauncher>,
    adapter: Arc<AcpAdapter>,
    auth: Arc<AcpAuthController>,
    sessions: Arc<AcpSessionCatalog>,
    snapshot: watch::Sender<ProviderSnapshot>,
}

impl AntigravityInstance {
    pub(crate) fn create(id: InstanceId, config: AntigravityConfig) -> Arc<Self> {
        let executable = config.executable.clone().or_else(crate::driver::find_acp_server).unwrap_or_default();
        let launcher: Arc<dyn AcpLauncher> =
            Arc::new(ProcessLauncher { program: executable.clone(), args: config.args, env: config.env });
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
                executable,
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
        if !self.executable.is_file() {
            self.snapshot.send_modify(|s| {
                s.status = ProviderStatus::Unavailable {
                    reason: format!("Antigravity ACP server not found ({})", self.executable.display()),
                };
            });
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
