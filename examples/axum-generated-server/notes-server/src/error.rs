use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use notes_openapi::ValidationError;
use serde::Serialize;

/// The application's own error type. The generated `Api` trait only requires
/// `From<ValidationError>`; the axum adapter additionally requires `IntoResponse`.
pub enum AppError {
    NotFound,
    Unauthorized,
    ValidationFailed { details: Vec<String> },
}

// Lets the generated handlers turn a failed `.validate()?` into our error.
impl From<ValidationError> for AppError {
    fn from(e: ValidationError) -> Self {
        AppError::ValidationFailed { details: e.details }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct Body {
            error: String,
            details: Vec<String>,
        }

        let (status, body) = match self {
            AppError::NotFound => (
                StatusCode::NOT_FOUND,
                Body {
                    error: "not found".into(),
                    details: vec![],
                },
            ),
            AppError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                Body {
                    error: "missing or invalid bearer token".into(),
                    details: vec![],
                },
            ),
            AppError::ValidationFailed { details } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Body {
                    error: "validation failed".into(),
                    details,
                },
            ),
        };

        (status, Json(body)).into_response()
    }
}
