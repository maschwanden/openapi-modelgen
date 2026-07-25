use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

pub enum AppError {
    BadRequest { message: String },
    ValidationFailed { details: Vec<String> },
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct Body {
            error: String,
            details: Vec<String>,
        }

        let (status, body) = match self {
            AppError::BadRequest { message } => (
                StatusCode::BAD_REQUEST,
                Body {
                    error: message,
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
