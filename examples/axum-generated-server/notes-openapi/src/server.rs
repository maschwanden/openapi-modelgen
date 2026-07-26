// This file is @generated — do not edit manually.

//! Server scaffolding generated from the OpenAPI paths.
//!
//! Implement [`Api`] for your application type — the compiler will list
//! any operation you have not handled yet. With the `server-axum` feature,
//! [`router`] wires every generated operation into an `axum::Router`.
//!
//! Operations configured as `manual` are NOT part of this trait — wire them
//! yourself and merge them into `router(..)`:
//!   - `DELETE /notes/{id}`

/// One method per OpenAPI operation. No default bodies: a missing route
/// is a `not all trait items implemented` compile error.
///
/// `State`, `Error` and `Ctx` (a general request extractor) are yours to
/// choose. Secured operations additionally receive `Auth`.
pub trait Api {
    type State: Clone + Send + Sync + 'static;
    type Error: From<crate::ValidationError> + Send;
    type Ctx: Send;
    type Auth: Send;
    /// `GET /notes`
    fn list_notes(state: Self::State, ctx: Self::Ctx, query: crate::ListNotesQuery) -> impl ::core::future::Future<Output = Result<Vec<crate::Note>, Self::Error>> + Send;
    /// `POST /notes`
    fn create_note(state: Self::State, ctx: Self::Ctx, auth: Self::Auth, body: crate::NewNote) -> impl ::core::future::Future<Output = Result<crate::Note, Self::Error>> + Send;
    /// `GET /notes/{id}`
    fn get_note(state: Self::State, ctx: Self::Ctx, id: String) -> impl ::core::future::Future<Output = Result<crate::Note, Self::Error>> + Send;
}

#[cfg(feature = "server-axum")]
mod axum_impl {
    use super::Api;
    use crate::Validation;
    use axum::{
        Router,
        http::StatusCode,
        extract::{FromRequestParts, State, Path, Json},
        response::IntoResponse,
        routing::{get},
    };
    use axum_extra::extract::Query;

    /// Wire every generated route into an `axum::Router`. Add any manual
    /// routes, then apply your state LAST:
    /// `router::<MyApi>().route("/healthz", ..).with_state(state)`.
    pub fn router<A>() -> Router<A::State>
    where
        A: Api + 'static,
        A::Error: IntoResponse,
        A::Ctx: FromRequestParts<A::State> + 'static,
        A::Auth: FromRequestParts<A::State> + 'static,
    {
        Router::new()
            .route("/notes", get(handle_list_notes::<A>).post(handle_create_note::<A>))
            .route("/notes/{id}", get(handle_get_note::<A>))
    }

    async fn handle_list_notes<A>(
        State(state): State<A::State>,
        ctx: A::Ctx,
        Query(query): Query<crate::ListNotesQuery>,
    ) -> Result<(StatusCode, Json<Vec<crate::Note>>), A::Error>
    where
        A: Api + 'static,
        A::Error: IntoResponse,
        A::Ctx: FromRequestParts<A::State> + 'static,
    {
        query.validate()?;
        let __resp = A::list_notes(state, ctx, query).await?;
        Ok((StatusCode::OK, Json(__resp)))
    }
    async fn handle_create_note<A>(
        State(state): State<A::State>,
        ctx: A::Ctx,
        auth: A::Auth,
        Json(body): Json<crate::NewNote>,
    ) -> Result<(StatusCode, Json<crate::Note>), A::Error>
    where
        A: Api + 'static,
        A::Error: IntoResponse,
        A::Ctx: FromRequestParts<A::State> + 'static,
        A::Auth: FromRequestParts<A::State> + 'static,
    {
        body.validate()?;
        let __resp = A::create_note(state, ctx, auth, body).await?;
        Ok((StatusCode::CREATED, Json(__resp)))
    }
    async fn handle_get_note<A>(
        State(state): State<A::State>,
        ctx: A::Ctx,
        Path(id): Path<String>,
    ) -> Result<(StatusCode, Json<crate::Note>), A::Error>
    where
        A: Api + 'static,
        A::Error: IntoResponse,
        A::Ctx: FromRequestParts<A::State> + 'static,
    {
        let __resp = A::get_note(state, ctx, id).await?;
        Ok((StatusCode::OK, Json(__resp)))
    }
}

#[cfg(feature = "server-axum")]
pub use axum_impl::router;
