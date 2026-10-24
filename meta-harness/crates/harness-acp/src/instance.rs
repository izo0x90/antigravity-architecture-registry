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
use crate::auth::{AcpAuthController, AuthEvent};
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
        let (snapshot, _) = watch::channel(ProviderSnapshot {
            capabilities: profile.capabilities(),
            runtime_modes: profile.runtime_modes(),
            supports_plan_mode: profile.supports_plan_mode(),
            home: profile.home(),
            ..ProviderSnapshot::new(id.clone(), harness.driver.clone(), harness.display_name.clone())
        });
        Arc::new_cyclic(|me: &Weak<Self>| {
            let me = me.clone();
            // A sign-in under way, or over: say so; signed in (or not known),
            // check again (for who).
            let on_auth = Arc::new(move |event: AuthEvent| {
                let Some(instance) = me.upgrade() else { return };
                let signed_in = match event {
                    // Ids only grow: what's said of an older sign-in than the
                    // one shown, or one that has ended, is passed over.
                    AuthEvent::Waiting(waiting) => {
                        instance.snapshot.send_if_modified(|s| {
                            let older = s.auth.waiting.as_ref().is_some_and(|w| w.id > waiting.id) || s.auth.ended.as_ref().is_some_and(|e| e.id >= waiting.id);
                            if !older {
                                s.auth.waiting = Some(Box::new(waiting));
                                s.auth.ended = None;
                            }
                            !older
                        });
                        return;
                    }
                    AuthEvent::Ended(ended) => {
                        instance.snapshot.send_if_modified(|s| {
                            let older = s.auth.waiting.as_ref().is_some_and(|w| w.id > ended.id) || s.auth.ended.as_ref().is_some_and(|e| e.id > ended.id);
                            if !older {
                                s.auth.waiting = None;
                                s.auth.ended = Some(Box::new(ended));
                            }
                            !older
                        });
                        return;
                    }
                    AuthEvent::Status(signed_in) => signed_in,
                };
                if let Some(signed_in) = signed_in {
                    instance.snapshot.send_modify(|s| s.auth.status = if signed_in { AuthStatus::SignedIn } else { AuthStatus::SignedOut });
                }
                if signed_in == Some(false) {
                    return;
                }
                tokio::spawn(async move {
                    if let Err(err) = instance.refresh().await {
                        tracing::warn!(target: "harness_acp", "check after login failed: {err}");
                    }
                });
            });
            Self {
                adapter: Arc::new(AcpAdapter::new(id.clone(), profile.clone(), launcher.clone(), on_auth.clone())),
                id,
                auth: Arc::new(AcpAuthController::new(launcher.clone(), profile.clone(), launch, on_auth)),
                sessions: Arc::new(AcpSessionCatalog::new(launcher.clone(), profile.clone())),
                profile,
                unavailable,
                launcher,
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
                // One that doesn't say, nothing known yet: signed in when its
                // login is kept where it keeps it. What it does (a session
                // refused, a sign-in) says more, and isn't overridden.
                let kept = match (&info.sign_in, &self.profile.settings.login.kept) {
                    (None, Some(kept)) if self.snapshot.borrow().auth.status == AuthStatus::Unknown => tokio::fs::try_exists(self.profile.places.path(kept)).await.unwrap_or(false),
                    _ => false,
                };
                self.snapshot.send_modify(|s| {
                    s.status = ProviderStatus::Ready;
                    s.auth.methods = info.auth_methods;
                    if let Some(sign_in) = &info.sign_in {
                        apply_sign_in(&mut s.auth, sign_in);
                    } else if kept && s.auth.status == AuthStatus::Unknown {
                        s.auth.status = AuthStatus::SignedIn;
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
