use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use url::Url;

use crate::error::HarnessError;

pub const AUTH_TIMEOUT: Duration = Duration::from_secs(300);
pub const FORWARDING_FAILED_MESSAGE: &str =
    "Could not deliver the sign-in response. Start sign-in again.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAuthState {
    pub instance_id: String,
    pub phase: String,
    pub flow_id: Option<String>,
    pub authorization_url: Option<String>,
    pub expires_at: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityAuthorizationUrl {
    pub authorization_url: String,
    pub state: String,
    pub redirect_port: u16,
    pub redirect_path: String,
}

pub fn parse_antigravity_authorization_url(
    url_str: &str,
) -> Result<AntigravityAuthorizationUrl, HarnessError> {
    let parsed = Url::parse(url_str).map_err(|e| HarnessError::Validation(e.to_string()))?;
    let mut state = None;
    let mut redirect_uri_str = None;
    for (k, v) in parsed.query_pairs() {
        if k == "state" {
            state = Some(v.to_string());
        } else if k == "redirect_uri" {
            redirect_uri_str = Some(v.to_string());
        }
    }
    let state = state
        .ok_or_else(|| HarnessError::Validation("Missing state in auth url".to_string()))?;
    let redirect_uri_str = redirect_uri_str.ok_or_else(|| {
        HarnessError::Validation("Missing redirect_uri in auth url".to_string())
    })?;
    let redirect_uri =
        Url::parse(&redirect_uri_str).map_err(|e| HarnessError::Validation(e.to_string()))?;
    let redirect_port = redirect_uri.port().unwrap_or(80);
    let redirect_path = redirect_uri.path().to_string();

    Ok(AntigravityAuthorizationUrl {
        authorization_url: url_str.to_string(),
        state,
        redirect_port,
        redirect_path,
    })
}

pub async fn forward_antigravity_callback(callback_url: &Url) -> Result<(), HarnessError> {
    let port = callback_url.port().unwrap_or(80);
    let addr = format!("127.0.0.1:{}", port);
    let mut stream = tokio::net::TcpStream::connect(&addr).await.map_err(|_| {
        HarnessError::ProviderSetup {
            operation: "complete".to_string(),
            detail: FORWARDING_FAILED_MESSAGE.to_string(),
        }
    })?;

    let path_and_query = match callback_url.query() {
        Some(q) => format!("{}?{}", callback_url.path(), q),
        None => callback_url.path().to_string(),
    };

    let http_req = format!(
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        path_and_query, port
    );
    use tokio::io::AsyncWriteExt;
    stream.write_all(http_req.as_bytes()).await.map_err(|_| {
        HarnessError::ProviderSetup {
            operation: "complete".to_string(),
            detail: FORWARDING_FAILED_MESSAGE.to_string(),
        }
    })?;
    let _ = stream.flush().await;
    Ok(())
}

pub fn validate_antigravity_callback_url(
    pending: &AntigravityAuthorizationUrl,
    callback_url_str: &str,
) -> Result<Url, HarnessError> {
    let callback_url = Url::parse(callback_url_str).map_err(|_| HarnessError::ProviderSetup {
        operation: "complete".to_string(),
        detail: "Invalid callback URL".to_string(),
    })?;

    if callback_url.port().unwrap_or(80) != pending.redirect_port {
        return Err(HarnessError::ProviderSetup {
            operation: "complete".to_string(),
            detail: "Callback port mismatch".to_string(),
        });
    }

    if callback_url.path() != pending.redirect_path {
        return Err(HarnessError::ProviderSetup {
            operation: "complete".to_string(),
            detail: "Callback path mismatch".to_string(),
        });
    }

    let state_matches: Vec<_> = callback_url
        .query_pairs()
        .filter(|(k, _)| k == "state")
        .map(|(_, v)| v.to_string())
        .collect();

    if state_matches.len() != 1 || state_matches[0] != pending.state {
        return Err(HarnessError::ProviderSetup {
            operation: "complete".to_string(),
            detail: "Callback state mismatch".to_string(),
        });
    }

    Ok(callback_url)
}

pub fn safe_auth_failure(err_msg: &str, uses_browser: bool) -> String {
    if err_msg.contains("SUBSCRIPTION_REQUIRED") {
        return "Google requires an eligible Antigravity subscription for this account.".to_string();
    }
    let lower = err_msg.to_lowercase();
    if lower.contains("access_denied")
        || lower.contains("denied access")
        || lower.contains("cancelled")
    {
        return "Google sign-in was not approved. Start sign-in again.".to_string();
    }
    if err_msg.contains("session/new") && err_msg.contains("-32603") {
        return "Antigravity authenticated, but could not initialize a session or load models."
            .to_string();
    }
    if !uses_browser && err_msg.contains("-32602") {
        return "Antigravity rejected the configured credentials. Check the provider settings."
            .to_string();
    }
    if uses_browser {
        "Google sign-in failed. Start sign-in again.".to_string()
    } else {
        "Antigravity could not authenticate with the configured credentials.".to_string()
    }
}

pub fn is_logout_prompt(text: &str, has_attachments: bool) -> bool {
    !has_attachments && text.trim() == "/logout"
}

pub fn visible_snapshot(
    state: &ProviderAuthState,
    flow_owner: Option<&str>,
    client_session_id: &str,
) -> ProviderAuthState {
    if flow_owner.is_none() || flow_owner == Some(client_session_id) {
        return state.clone();
    }
    let busy = matches!(state.phase.as_str(), "starting" | "waiting" | "verifying");
    ProviderAuthState {
        instance_id: state.instance_id.clone(),
        phase: state.phase.clone(),
        flow_id: None,
        authorization_url: None,
        expires_at: None,
        message: if busy {
            Some("Sign-in is in progress in another client.".to_string())
        } else {
            state.message.clone()
        },
    }
}

pub trait AntigravityAuthRuntime: Send + Sync {
    fn initialize(&mut self) -> Result<serde_json::Value, HarnessError>;
    fn start(&mut self) -> Result<serde_json::Value, HarnessError>;
    fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, HarnessError>;
}

struct AuthFlowInternal {
    id: String,
    owner_session_id: String,
    deadline: Instant,
    state: ProviderAuthState,
    pending: Option<AntigravityAuthorizationUrl>,
    callback_sent: bool,
}

struct InnerAuth {
    instance_id: String,
    uses_browser: bool,
    operation: String,
    active_flow: Option<AuthFlowInternal>,
    current_state: ProviderAuthState,
    active_processes: HashSet<u64>,
    next_process_id: u64,
}

#[derive(Clone)]
pub struct AntigravityAuth {
    inner: Arc<Mutex<InnerAuth>>,
    tx: broadcast::Sender<(Option<String>, ProviderAuthState)>,
}

impl AntigravityAuth {
    pub fn new(instance_id: String, uses_browser: bool) -> Self {
        let current_state = ProviderAuthState {
            instance_id: instance_id.clone(),
            phase: "idle".to_string(),
            flow_id: None,
            authorization_url: None,
            expires_at: None,
            message: None,
        };

        let inner = Arc::new(Mutex::new(InnerAuth {
            instance_id,
            uses_browser,
            operation: "idle".to_string(),
            active_flow: None,
            current_state,
            active_processes: HashSet::new(),
            next_process_id: 1,
        }));

        let (tx, _) = broadcast::channel(128);

        Self { inner, tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<(Option<String>, ProviderAuthState)> {
        self.tx.subscribe()
    }

    pub fn get_state(&self, owner_session_id: &str) -> ProviderAuthState {
        let lock = self.inner.lock().unwrap();
        let owner = lock
            .active_flow
            .as_ref()
            .map(|f| f.owner_session_id.as_str());
        visible_snapshot(&lock.current_state, owner, owner_session_id)
    }

    pub fn start<F>(
        &self,
        owner_session_id: &str,
        stop_sessions: F,
    ) -> Result<ProviderAuthState, HarnessError>
    where
        F: FnOnce() -> Result<(), HarnessError>,
    {
        let state = {
            let mut lock = self.inner.lock().unwrap();
            if let Some(ref flow) = lock.active_flow
                && flow.owner_session_id == owner_session_id
                && lock.operation == "auth"
            {
                return Ok(visible_snapshot(
                    &flow.state,
                    Some(owner_session_id),
                    owner_session_id,
                ));
            }

            if lock.operation != "idle" {
                return Err(HarnessError::ProviderSetup {
                    operation: "start".to_string(),
                    detail: "Antigravity setup is already in progress.".to_string(),
                });
            }

            let flow_id = uuid::Uuid::new_v4().to_string();
            let now = Instant::now();
            let deadline = now + AUTH_TIMEOUT;

            let state = ProviderAuthState {
                instance_id: lock.instance_id.clone(),
                phase: "starting".to_string(),
                flow_id: Some(flow_id.clone()),
                authorization_url: None,
                expires_at: Some("300s".to_string()),
                message: Some(if lock.uses_browser {
                    "Starting Google sign-in.".to_string()
                } else {
                    "Checking credentials.".to_string()
                }),
            };

            let flow = AuthFlowInternal {
                id: flow_id,
                owner_session_id: owner_session_id.to_string(),
                deadline,
                state: state.clone(),
                pending: None,
                callback_sent: false,
            };

            lock.active_flow = Some(flow);
            lock.operation = "auth".to_string();
            lock.current_state = state.clone();

            let _ = self
                .tx
                .send((Some(owner_session_id.to_string()), state.clone()));
            state
        };

        let _ = stop_sessions();

        Ok(visible_snapshot(
            &state,
            Some(owner_session_id),
            owner_session_id,
        ))
    }

    pub fn receive_authorization_url(&self, url_str: &str) -> Result<(), HarnessError> {
        let auth_url = parse_antigravity_authorization_url(url_str)?;
        let mut lock = self.inner.lock().unwrap();

        if lock.operation != "auth" {
            return Ok(());
        }

        let instance_id = lock.instance_id.clone();
        let Some(ref mut flow) = lock.active_flow else {
            return Ok(());
        };

        if let Some(ref pending) = flow.pending {
            if pending.authorization_url == auth_url.authorization_url {
                return Ok(());
            }

            let owner = flow.owner_session_id.clone();
            let failed_state = ProviderAuthState {
                instance_id,
                phase: "failed".to_string(),
                flow_id: None,
                authorization_url: None,
                expires_at: None,
                message: Some(
                    "Antigravity started more than one Google sign-in request.".to_string(),
                ),
            };
            lock.current_state = failed_state.clone();
            lock.operation = "idle".to_string();
            lock.active_flow = None;
            let _ = self.tx.send((Some(owner), failed_state));
            return Err(HarnessError::Unsupported(
                "Antigravity started more than one Google sign-in request.".to_string(),
            ));
        }

        let owner = flow.owner_session_id.clone();
        let flow_id = flow.id.clone();
        let expires_at = flow.state.expires_at.clone();

        let waiting_state = ProviderAuthState {
            instance_id,
            phase: "waiting".to_string(),
            flow_id: Some(flow_id),
            authorization_url: Some(auth_url.authorization_url.clone()),
            expires_at,
            message: Some(
                "Open the Google sign-in link. If you are remote, paste the redirect URL here."
                    .to_string(),
            ),
        };

        flow.pending = Some(auth_url);
        flow.state = waiting_state.clone();
        lock.current_state = waiting_state.clone();
        let _ = self.tx.send((Some(owner), waiting_state));

        Ok(())
    }

    pub fn complete(
        &self,
        owner_session_id: &str,
        flow_id: &str,
        callback_url: &str,
    ) -> Result<ProviderAuthState, HarnessError> {
        let mut lock = self.inner.lock().unwrap();

        let (pending, expires_at) = {
            let flow = match lock.active_flow.as_ref() {
                Some(f)
                    if f.id == flow_id
                        && f.owner_session_id == owner_session_id
                        && Instant::now() < f.deadline =>
                {
                    f
                }
                _ => {
                    return Err(HarnessError::ProviderSetup {
                        operation: "complete".to_string(),
                        detail: "This sign-in is no longer active in this client.".to_string(),
                    });
                }
            };

            let pending = match flow.pending.as_ref() {
                Some(p) if !flow.callback_sent => p.clone(),
                _ => {
                    return Err(HarnessError::ProviderSetup {
                        operation: "complete".to_string(),
                        detail: if flow.callback_sent {
                            "The sign-in response was already sent. Wait for Google to finish."
                                .to_string()
                        } else {
                            "Wait for the Google sign-in link before you send a redirect URL."
                                .to_string()
                        },
                    });
                }
            };

            (pending, flow.state.expires_at.clone())
        };

        validate_antigravity_callback_url(&pending, callback_url)?;

        let instance_id = lock.instance_id.clone();
        let verifying = ProviderAuthState {
            instance_id,
            phase: "verifying".to_string(),
            flow_id: Some(flow_id.to_string()),
            authorization_url: None,
            expires_at,
            message: Some("Waiting for Google to finish sign-in.".to_string()),
        };

        if let Some(ref mut flow) = lock.active_flow {
            flow.callback_sent = true;
            flow.state = verifying.clone();
        }
        lock.current_state = verifying.clone();
        let _ = self
            .tx
            .send((Some(owner_session_id.to_string()), verifying.clone()));

        Ok(visible_snapshot(
            &verifying,
            Some(owner_session_id),
            owner_session_id,
        ))
    }

    pub fn fail_delivery(&self, flow_id: &str) {
        let mut lock = self.inner.lock().unwrap();
        if let Some(ref flow) = lock.active_flow
            && flow.id == flow_id
        {
            let owner = flow.owner_session_id.clone();
            let instance_id = lock.instance_id.clone();
            let failed = ProviderAuthState {
                instance_id,
                phase: "failed".to_string(),
                flow_id: None,
                authorization_url: None,
                expires_at: None,
                message: Some(FORWARDING_FAILED_MESSAGE.to_string()),
            };
            lock.current_state = failed.clone();
            lock.operation = "idle".to_string();
            lock.active_flow = None;
            let _ = self.tx.send((Some(owner), failed));
        }
    }

    pub fn finish_authentication(&self, flow_id: &str, result: Result<(), HarnessError>) {
        let mut lock = self.inner.lock().unwrap();
        if let Some(ref flow) = lock.active_flow
            && flow.id == flow_id
        {
            let owner = flow.owner_session_id.clone();
            let instance_id = lock.instance_id.clone();
            let uses_browser = lock.uses_browser;
            let final_state = match result {
                Ok(()) => ProviderAuthState {
                    instance_id,
                    phase: "succeeded".to_string(),
                    flow_id: None,
                    authorization_url: None,
                    expires_at: None,
                    message: Some(if uses_browser {
                        "Signed in with Google.".to_string()
                    } else {
                        "Connected to Antigravity.".to_string()
                    }),
                },
                Err(err) => ProviderAuthState {
                    instance_id,
                    phase: "failed".to_string(),
                    flow_id: None,
                    authorization_url: None,
                    expires_at: None,
                    message: Some(safe_auth_failure(&err.to_string(), uses_browser)),
                },
            };
            lock.current_state = final_state.clone();
            lock.operation = "idle".to_string();
            lock.active_flow = None;
            let _ = self.tx.send((Some(owner), final_state));
        }
    }

    pub fn cancel(
        &self,
        owner_session_id: &str,
        flow_id: &str,
    ) -> Result<ProviderAuthState, HarnessError> {
        let mut lock = self.inner.lock().unwrap();
        let owner = match lock.active_flow.as_ref() {
            Some(f) if f.id == flow_id && f.owner_session_id == owner_session_id => {
                f.owner_session_id.clone()
            }
            _ => {
                return Err(HarnessError::ProviderSetup {
                    operation: "cancel".to_string(),
                    detail: "This sign-in is no longer active in this client.".to_string(),
                });
            }
        };

        let cancelled = ProviderAuthState {
            instance_id: lock.instance_id.clone(),
            phase: "cancelled".to_string(),
            flow_id: None,
            authorization_url: None,
            expires_at: None,
            message: Some("Google sign-in was cancelled.".to_string()),
        };

        lock.current_state = cancelled.clone();
        lock.operation = "idle".to_string();
        lock.active_flow = None;
        let _ = self.tx.send((Some(owner), cancelled.clone()));

        Ok(cancelled)
    }

    pub fn expire_flow(&self, flow_id: &str) {
        let mut lock = self.inner.lock().unwrap();
        if let Some(ref flow) = lock.active_flow
            && flow.id == flow_id
        {
            let owner = flow.owner_session_id.clone();
            let instance_id = lock.instance_id.clone();
            let failed = ProviderAuthState {
                instance_id,
                phase: "failed".to_string(),
                flow_id: None,
                authorization_url: None,
                expires_at: None,
                message: Some("Google sign-in expired. Start sign-in again.".to_string()),
            };
            lock.current_state = failed.clone();
            lock.operation = "idle".to_string();
            lock.active_flow = None;
            let _ = self.tx.send((Some(owner), failed));
        }
    }

    pub fn logout<S, R>(
        &self,
        stop_sessions: S,
        runtime_factory: R,
    ) -> Result<ProviderAuthState, HarnessError>
    where
        S: FnOnce() -> Result<(), HarnessError>,
        R: FnOnce() -> Result<Box<dyn AntigravityAuthRuntime>, HarnessError>,
    {
        {
            let mut lock = self.inner.lock().unwrap();
            if lock.operation != "idle" && lock.operation != "auth" {
                return Err(HarnessError::ProviderSetup {
                    operation: "logout".to_string(),
                    detail: "Antigravity setup is already stopping.".to_string(),
                });
            }
            lock.operation = "logout".to_string();
            if let Some(ref flow) = lock.active_flow {
                let owner = flow.owner_session_id.clone();
                let cancelled = ProviderAuthState {
                    instance_id: lock.instance_id.clone(),
                    phase: "cancelled".to_string(),
                    flow_id: None,
                    authorization_url: None,
                    expires_at: None,
                    message: Some("Google sign-in was cancelled by sign-out.".to_string()),
                };
                let _ = self.tx.send((Some(owner), cancelled));
            }
            lock.active_flow = None;
        }

        let stop_res = stop_sessions();
        if let Err(err) = stop_res {
            let mut lock = self.inner.lock().unwrap();
            lock.operation = "idle".to_string();
            return Err(err);
        }

        let mut runtime = match runtime_factory() {
            Ok(rt) => rt,
            Err(err) => {
                let mut lock = self.inner.lock().unwrap();
                lock.operation = "idle".to_string();
                return Err(err);
            }
        };

        let init_val = match runtime.initialize() {
            Ok(v) => v,
            Err(err) => {
                let mut lock = self.inner.lock().unwrap();
                lock.operation = "idle".to_string();
                return Err(err);
            }
        };

        let supports_logout = init_val
            .get("agentCapabilities")
            .and_then(|c| c.get("auth"))
            .and_then(|a| a.get("logout"))
            .is_some();

        if !supports_logout {
            let mut lock = self.inner.lock().unwrap();
            lock.operation = "idle".to_string();
            return Err(HarnessError::ProviderSetup {
                operation: "logout".to_string(),
                detail:
                    "This Antigravity version does not support sign-out. Update the provider."
                        .to_string(),
            });
        }

        let _ = runtime.request("logout", serde_json::json!({}))?;

        let instance_id = self.inner.lock().unwrap().instance_id.clone();
        let final_idle = ProviderAuthState {
            instance_id,
            phase: "idle".to_string(),
            flow_id: None,
            authorization_url: None,
            expires_at: None,
            message: Some("Signed out of Google.".to_string()),
        };

        {
            let mut lock = self.inner.lock().unwrap();
            lock.operation = "idle".to_string();
            lock.current_state = final_idle.clone();
            let _ = self.tx.send((None, final_idle.clone()));
        }

        Ok(final_idle)
    }

    pub fn with_process<T, S, F>(&self, stop: S, task: F) -> Result<T, HarnessError>
    where
        S: FnOnce(),
        F: FnOnce() -> Result<T, HarnessError>,
    {
        let pid = {
            let mut lock = self.inner.lock().unwrap();
            if lock.operation != "idle" {
                return Err(HarnessError::ProviderSetup {
                    operation: "startProcess".to_string(),
                    detail:
                        "Antigravity sign-in or sign-out is in progress. Try again after it finishes."
                            .to_string(),
                });
            }
            let pid = lock.next_process_id;
            lock.next_process_id += 1;
            lock.active_processes.insert(pid);
            pid
        };

        let res = task();

        {
            let mut lock = self.inner.lock().unwrap();
            lock.active_processes.remove(&pid);
        }

        stop();
        res
    }
}
