mod api;
mod error;
mod extract;
mod state;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};

use crate::api::NotesApi;
use crate::state::AppState;

#[tokio::main]
async fn main() {
    let state = AppState::seeded();

    // `notes_openapi::server::router` wires every STRICT operation
    // (list_notes / create_note / get_note) into an axum Router. `deleteNote`
    // is `manual`, so we merge our own handler, plus a non-spec `/healthz`.
    //
    // The generated `router` returns `Router<AppState>` WITHOUT applying state,
    // so hand-wired manual handlers can use the `State` extractor just like the
    // generated ones. We apply `.with_state` LAST, after merging everything.
    let app = notes_openapi::server::router::<NotesApi>()
        .route("/notes/{id}", delete(delete_note))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();
    println!("listening on http://127.0.0.1:3000");
    axum::serve(listener, app).await.unwrap();
}

/// Manual handler for `deleteNote` (opted out of the trait via
/// `server-style: manual`). Because `router` no longer applies state, this
/// hand-wired handler uses the `State` extractor like any generated one.
async fn delete_note(State(state): State<AppState>, Path(id): Path<String>) -> StatusCode {
    let mut notes = state.notes.lock().unwrap();
    let before = notes.len();
    notes.retain(|n| n.id != id);
    if notes.len() < before {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}
