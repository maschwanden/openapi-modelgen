use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;

use crate::error::AppError;
use crate::state::AppState;

/// The authenticated principal. Because `POST /notes` declares `security` in
/// the spec, the generated trait gives `create_note` an `auth: Self::Auth`
/// parameter — and THIS extractor is the guard that produces it. A request
/// without a valid `Authorization: Bearer <token>` header is rejected with 401
/// before `create_note` ever runs.
pub struct AuthUser {
    #[allow(dead_code)]
    pub token: String,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .filter(|t| !t.is_empty());

        match token {
            Some(t) => Ok(AuthUser {
                token: t.to_string(),
            }),
            None => Err(AppError::Unauthorized),
        }
    }
}
