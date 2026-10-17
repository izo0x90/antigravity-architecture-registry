//! The cockpit session: user-only actions (approvals, accepting review
//! answers, deleting plans, driving the work queue, answering an agent's
//! permission requests) need the cookie the cockpit gets from its launch
//! link, `/cockpit?nah=<token>`. The token is the user's (from
//! `NAH_COCKPIT_TOKEN`, or a file in their config directory, outside any
//! repository an agent works in) and is never handed out by the API. The
//! cookie is HttpOnly and SameSite=Strict, so other sites can't use it.
//!
//! Limits: an agent running shell commands as the same OS user could read
//! the token file; keeping agents out of it is the harness sandbox's job.

use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{FromRef, FromRequestParts, Query, State};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use nah_access::{Access, Capability, Principal};
use serde::Deserialize;

const COOKIE_NAME: &str = "nah_cockpit";

#[derive(Clone)]
pub struct CockpitAuth {
    token: Arc<str>,
}

impl CockpitAuth {
    pub fn new(token: &str) -> Self {
        Self { token: token.into() }
    }

    /// The user's token: `NAH_COCKPIT_TOKEN`, else the one stored in
    /// `<config dir>/nah/cockpit-token` (created on first launch, owner-only).
    pub fn for_user() -> std::io::Result<Self> {
        if let Ok(token) = std::env::var("NAH_COCKPIT_TOKEN")
            && !token.trim().is_empty()
        {
            return Ok(Self::new(token.trim()));
        }
        let file = token_file()?;
        match std::fs::read_to_string(&file) {
            Ok(token) if !token.trim().is_empty() => Ok(Self::new(token.trim())),
            _ => {
                let token = uuid::Uuid::new_v4().simple().to_string();
                write_private(&file, &token)?;
                Ok(Self::new(&token))
            }
        }
    }

    /// Where the user opens the cockpit.
    /// The secret itself (what `meta-harness token` prints).
    pub fn secret(&self) -> &str {
        &self.token
    }

    pub fn launch_path(&self) -> String {
        format!("/cockpit?nah={}", self.token)
    }

    fn set_cookie(&self) -> String {
        format!("{COOKIE_NAME}={}; HttpOnly; SameSite=Strict; Path=/", self.token)
    }

    /// A home server, relaying the user: `Authorization: Bearer <secret>`
    /// (the secret is never taken from a URL, but for the launch link).
    fn bearer(&self, parts: &Parts) -> bool {
        parts.headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).is_some_and(|secret| secret.trim() == &*self.token)
    }

    fn admits(&self, cookies: &str) -> bool {
        cookies.split(';').filter_map(|c| c.trim().split_once('=')).any(|(name, value)| name == COOKIE_NAME && value == &*self.token)
    }
}

fn token_file() -> std::io::Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .ok_or_else(|| std::io::Error::other("no config directory (set NAH_COCKPIT_TOKEN)"))?;
    Ok(base.join("nah").join("cockpit-token"))
}

fn write_private(file: &Path, token: &str) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct Launch {
    nah: String,
}

/// The launch link: a matching token sets the cookie and opens the cockpit.
/// It answers with a page (not a redirect) so the cookie also survives
/// proxies that follow redirects themselves.
pub async fn launch(State(auth): State<CockpitAuth>, Query(q): Query<Launch>) -> Response {
    if q.nah != *auth.token {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({ "error": "this isn't the cockpit's launch link" }))).into_response();
    }
    let page = "<!doctype html><meta charset=\"utf-8\"><meta http-equiv=\"refresh\" content=\"0; url=/\"><title>NAH</title><script>location.replace('/')</script><a href=\"/\">Open the cockpit</a>";
    let mut response = (StatusCode::OK, axum::response::Html(page)).into_response();
    if let Ok(cookie) = HeaderValue::from_str(&auth.set_cookie()) {
        response.headers_mut().insert(SET_COOKIE, cookie);
    }
    response
}

/// Who a request comes from: the cookie the launch link set is the user;
/// anything else is no one known. (Agents don't use these routes: they use
/// the MCP endpoint, where their session's key says who they are.)
pub fn principal_of(parts: &Parts, auth: &CockpitAuth) -> Principal {
    if auth.bearer(parts) {
        return Principal::Home;
    }
    let user = parts.headers.get_all(COOKIE).iter().filter_map(|v| v.to_str().ok()).any(|c| auth.admits(c));
    if user { Principal::User } else { Principal::Anonymous }
}

/// A capability a route needs, as a type: `Needs<can::SessionsControl>`.
pub trait Cap: Send + Sync + 'static {
    const NEEDS: Capability;
}

/// The capabilities as types, one per `Capability`.
pub mod can {
    use super::{Cap, Capability};

    macro_rules! markers {
        ($($name:ident),* $(,)?) => {
            $(
                pub struct $name;
                impl Cap for $name {
                    const NEEDS: Capability = Capability::$name;
                }
            )*
        };
    }

    markers!(ViewRead, ViewDrive, PlanEdit, CodeCheck, SlotsObserve, SlotsManage, SlotsMessage, SlotsReport, UserNotify, RequestsAnswer, PlansApprove, SessionsControl, ChangesStage, ShellDrive, TagsEdit);
}

/// Extractor: the request's caller has the capability `C`, else 403. Put it
/// first among a handler's arguments, so nothing else is read before it.
pub struct Needs<C: Cap>(PhantomData<C>);

impl<S, C: Cap> FromRequestParts<S> for Needs<C>
where
    CockpitAuth: FromRef<S>,
    Arc<Access>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let who = principal_of(parts, &CockpitAuth::from_ref(state));
        if Arc::<Access>::from_ref(state).allows(&who, C::NEEDS) {
            Ok(Needs(PhantomData))
        } else {
            Err((StatusCode::FORBIDDEN, Json(serde_json::json!({ "error": "only the user, in the cockpit, can do this" }))).into_response())
        }
    }
}

/// One-time tickets for the sockets: the cockpit asks for one with its
/// session (`POST /api/cockpit/ticket`), then opens a socket with it.
/// Proxies may drop cookies from a socket's request, so the session is
/// checked on the ticket's.
#[derive(Default)]
pub struct Tickets {
    issued: std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
}

/// How long a ticket stays good.
const TICKET_FOR: std::time::Duration = std::time::Duration::from_secs(60);

impl Tickets {
    /// A ticket to connect with (once, within a minute).
    pub fn issue(&self) -> String {
        let ticket = uuid::Uuid::new_v4().simple().to_string();
        let mut issued = self.issued.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        issued.retain(|_, at| at.elapsed() < TICKET_FOR);
        issued.insert(ticket.clone(), std::time::Instant::now());
        ticket
    }

    /// Uses a ticket up: whether it was good.
    pub fn admit(&self, ticket: &str) -> bool {
        self.issued.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(ticket).is_some_and(|at| at.elapsed() < TICKET_FOR)
    }
}

#[derive(Deserialize)]
struct TicketQuery {
    #[serde(default)]
    ticket: String,
}

/// Extractor for socket upgrades: a good ticket in `?ticket=` (the
/// cockpit), or a home server's secret in its `Authorization`, else 403.
pub struct Ticketed;

impl<S> FromRequestParts<S> for Ticketed
where
    Arc<Tickets>: FromRef<S>,
    CockpitAuth: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let q = Query::<TicketQuery>::try_from_uri(&parts.uri).map(|Query(q)| q.ticket).unwrap_or_default();
        if CockpitAuth::from_ref(state).bearer(parts) || Arc::<Tickets>::from_ref(state).admit(&q) {
            Ok(Ticketed)
        } else {
            Err((StatusCode::FORBIDDEN, Json(serde_json::json!({ "error": "only the cockpit connects here (with a ticket from /api/cockpit/ticket)" }))).into_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_is_good_once() {
        let tickets = Tickets::default();
        let ticket = tickets.issue();
        assert!(tickets.admit(&ticket));
        assert!(!tickets.admit(&ticket), "used up");
        assert!(!tickets.admit("made-up"));
    }
}
