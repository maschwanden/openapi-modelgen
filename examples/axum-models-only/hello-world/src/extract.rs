use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Query, Request};
use axum::http::request::Parts;

use hello_world_openapi::{Validation, ValidationError};
use serde::de::DeserializeOwned;

use crate::error::AppError;

pub struct JsonV<T>(pub T);

impl<T, S> FromRequest<S> for JsonV<T>
where
    T: DeserializeOwned + Validation,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) =
            Json::<T>::from_request(req, state)
                .await
                .map_err(|r: JsonRejection| AppError::BadRequest {
                    message: r.to_string(),
                })?;

        value
            .validate()
            .map_err(|e: ValidationError| AppError::ValidationFailed { details: e.details })?;

        Ok(JsonV(value))
    }
}

pub struct QueryV<T>(pub T);

impl<T, S> FromRequestParts<S> for QueryV<T>
where
    T: DeserializeOwned + Validation,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Query(value) =
            Query::<T>::from_request_parts(parts, state)
                .await
                .map_err(|r: QueryRejection| AppError::BadRequest {
                    message: r.to_string(),
                })?;

        value
            .validate()
            .map_err(|e: ValidationError| AppError::ValidationFailed { details: e.details })?;

        Ok(QueryV(value))
    }
}
