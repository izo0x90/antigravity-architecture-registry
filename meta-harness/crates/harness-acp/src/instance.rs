//! One configured ACP agent: its live snapshot (status, sign-in, catalog,
//! skills, tool servers), logins, saved sessions, and the adapter for its
//! sessions. What differs by agent is its profile's.

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use harness_contracts::{
    AuthState, AuthStatus, CatalogUpdate, HarnessProfile, InstanceId, Launch, ProviderSnapshot, ProviderStatus,
    SignIn,
};
use harness_core::skills::discover_skills;
use harness_core::tool_config::list_tool_servers;
use harness_core::{AuthController, HarnessError, ProviderAdapter, ProviderInstance, Result, SessionCatalog};
use tokio::sync::watch;

use crate::adapter::AcpAdapter;
use crate::auth::AcpAuthController;
use crate::catalog::AcpSessionCatalog;
use crate::control::probe;
use crate::launch::{AcpLauncher, AcpTransport};
use crate::profile::Profile;

/// What an instance is made of.
pub struct InstanceParts {
    pub id: InstanceId,
    pub harness: HarnessProfile,
    pub profile: Arc<Profile>,
    /// How its agent is started, or why it can't be.
    pub launcher: std::result::Result<(Arc<dyn AcpLauncher>, Option<Launch>), String>,
}

/// The launcher of an agent that can't be started: it says why.
struct Unavailable(String);

#[async_trait]
impl AcpLauncher for Unavailable {
    async fn launch(&self, _config: &harness_contracts::SessionConfig) -> Result<AcpTransport> {
        Err(HarnessError::Provider { reason: self.0.clone() })
    }
}

pub struct AcpInstance {
    id: InstanceId,
    profile: Arc<Profile>,
    /// Why its agent can't be started, if it can't.
    unavailable: Option<String>,
    launcher: Arc<dyn AcpLauncher>,
    adapter: Arc<AcpAdapter>,
    auth: Arc<AcpAuthController>,
    sessions: Arc<AcpSessionCatalog>,
    snapshot: watch::Sender<ProviderSnapshot>,
}

impl AcpInstance {
    pub fn create(parts: InstanceParts) -> Arc<Self> {
        let InstanceParts { id, harness, profile, launcher } = parts;
        let (launcher, launch, unavailable): (Arc<dyn AcpLauncher>, _, _) = match launcher {
            Ok((launcher, launch)) => (launcher, launch, None),
            Err(why) => (Arc::new(Unavailable(why.clone())), None, Some(why)),
        };
        let adapter = Arc::new(AcpAdapter::new(id.clone(), profile.clone(), launcher.clone()));
        let (snapshot, _) = watch::channel(ProviderSnapshot {
            instance_id: id.clone(),
            driver: harness.driver.clone(),
            display_name: harness.display_name.clone(),
            status: ProviderStatus::Starting,
            auth: AuthState { status: AuthStatus::Unknown, account: None, methods: Vec::new() },
            capabilities: adapter.capabilities(),
            models: Vec::new(),
            commands: Vec::new(),
            skills: Vec::new(),
            runtime_modes: profile.runtime_modes(),
            supports_plan_mode: profile.supports_plan_mode(),
            tool_servers: Vec::new(),
            problems: Vec::new(),
        });
        Arc::new_cyclic(|me: &Weak<Self>| {
            let me = me.clone();
            // After a login or logout: say so, then check again (for who).
            let on_auth = Arc::new(move |signed_in: Option<bool>| {
                let Some(instance) = me.upgrade() else { return };
                if let Some(signed_in) = signed_in {
                    instance.snapshot.send_modify(|s| s.auth.status = if signed_in { AuthStatus::SignedIn } else { AuthStatus::SignedOut });
                }
                tokio::spawn(async move {
                    if let Err(err) = instance.refresh().await {
                        tracing::warn!(target: "harness_acp", "check after login failed: {err}");
                    }
                });
            });
            Self {
                id,
                auth: Arc::new(AcpAuthController::new(launcher.clone(), profile.clone(), launch, on_auth)),
                sessions: Arc::new(AcpSessionCatalog::new(launcher.clone(), profile.clone())),
                profile,
                unavailable,
                launcher,
                adapter,
                snapshot,
            }
        })
    }

    /// Its skills and tool servers, read from its configuration.
    async fn read_configuration(&self) {
        let profile = self.profile.clone();
        let read = tokio::task::spawn_blocking(move || {
            let settings = &profile.settings;
            let mut problems = Vec::new();
            let skills = match &settings.skills {
                Some(skills) => discover_skills(&profile.places.skill_roots(skills), skills.rules()).unwrap_or_else(|e| {
                    problems.push(e.to_string());
                    Vec::new()
                }),
                None => Vec::new(),
            };
            let (tool_servers, unread) = list_tool_servers(&profile.places.tool_files(&settings.tool_config));
            problems.extend(unread);
            (skills, tool_servers, problems)
        });
        match read.await {
            Ok((skills, tool_servers, problems)) => self.snapshot.send_modify(|s| {
                s.skills = skills;
                s.tool_servers = tool_servers;
                s.problems = problems;
            }),
            Err(err) => tracing::warn!(target: "harness_acp", "reading the agent's configuration failed: {err}"),
        }
    }
}

fn apply_sign_in(auth: &mut AuthState, sign_in: &SignIn) {
    auth.status = sign_in.status;
    auth.account = sign_in.account.clone();
}

#[async_trait]
impl ProviderInstance for AcpInstance {
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
        self.read_configuration().await;
        match probe(&self.launcher, &self.profile).await {
            Ok(info) => {
                self.auth.set_logins(info.logins);
                self.snapshot.send_modify(|s| {
                    s.status = ProviderStatus::Ready;
                    s.auth.methods = info.auth_methods;
                    if let Some(sign_in) = &info.sign_in {
                        apply_sign_in(&mut s.auth, sign_in);
                    }
                    info.capabilities.apply(&mut s.capabilities);
                });
            }
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
            match &update.sign_in {
                Some(sign_in) => apply_sign_in(&mut s.auth, sign_in),
                // A session that reports a catalog is signed in (when the
                // agent doesn't say who as).
                None if s.auth.status != AuthStatus::SignedIn && (update.commands.is_some() || update.models.is_some()) => s.auth.status = AuthStatus::SignedIn,
                None => {}
            }
        });
    }
}
