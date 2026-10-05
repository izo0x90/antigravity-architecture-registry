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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{FromRef, FromRequestParts, Query, State};
use axum::http::header::{COOKIE, LOCATION, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
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
    pub fn launch_path(&self) -> String {
        format!("/cockpit?nah={}", self.token)
    }

    fn set_cookie(&self) -> String {
        format!("{COOKIE_NAME}={}; HttpOnly; SameSite=Strict; Path=/", self.token)
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
pub async fn launch(State(auth): State<CockpitAuth>, Query(q): Query<Launch>) -> Response {
    if q.nah != *auth.token {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({ "error": "this isn't the cockpit's launch link" }))).into_response();
    }
    let mut response = StatusCode::SEE_OTHER.into_response();
    if let (Ok(cookie), Ok(home)) = (HeaderValue::from_str(&auth.set_cookie()), HeaderValue::from_str("/")) {
        response.headers_mut().insert(SET_COOKIE, cookie);
        response.headers_mut().insert(LOCATION, home);
    }
    response
}

/// Extractor for handlers only the user, in the cockpit, may call.
pub struct CockpitUser;

impl<S> FromRequestParts<S> for CockpitUser
where
    CockpitAuth: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let auth = CockpitAuth::from_ref(state);
        let admitted = parts.headers.get_all(COOKIE).iter().filter_map(|v| v.to_str().ok()).any(|c| auth.admits(c));
        if admitted {
            Ok(CockpitUser)
        } else {
            Err((StatusCode::FORBIDDEN, Json(serde_json::json!({ "error": "only the user, in the cockpit, can do this" }))).into_response())
        }
    }
}
