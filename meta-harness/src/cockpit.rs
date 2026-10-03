//! The cockpit session: user-only actions (approvals, accepting review
//! answers, deleting plans, driving the work queue) need the cookie the
//! cockpit gets when it loads. It's per launch, HttpOnly and SameSite=Strict,
//! so other sites can't use it and agents never see it through their tools.

use std::sync::Arc;

use axum::Json;
use axum::extract::{FromRef, FromRequestParts};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

const COOKIE_NAME: &str = "nah_cockpit";

#[derive(Clone)]
pub struct CockpitAuth {
    token: Arc<str>,
}

impl CockpitAuth {
    pub fn new(token: &str) -> Self {
        Self { token: token.into() }
    }

    /// A fresh token for this launch.
    pub fn random() -> Self {
        Self::new(&uuid::Uuid::new_v4().simple().to_string())
    }

    pub fn set_cookie(&self) -> String {
        format!("{COOKIE_NAME}={}; HttpOnly; SameSite=Strict; Path=/", self.token)
    }

    fn admits(&self, cookies: &str) -> bool {
        cookies.split(';').filter_map(|c| c.trim().split_once('=')).any(|(name, value)| name == COOKIE_NAME && value == &*self.token)
    }
}

/// Adds the cockpit cookie to what the cockpit loads (its page and config).
pub async fn issue(auth: axum::extract::State<CockpitAuth>, request: Request<axum::body::Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    if let Ok(value) = HeaderValue::from_str(&auth.set_cookie()) {
        response.headers_mut().append(SET_COOKIE, value);
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
