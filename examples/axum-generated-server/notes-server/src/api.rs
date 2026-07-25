use notes_openapi::server::Api;
use notes_openapi::{ListNotesQuery, NewNote, Note};

use crate::error::AppError;
use crate::extract::AuthUser;
use crate::state::AppState;

/// Marker type implementing the generated `Api` trait. Implementing every
/// STRICT operation is compiler-enforced: add an operation to `openapi.yaml`,
/// regenerate, and this `impl` fails with
/// `error[E0046]: not all trait items implemented` until you handle it.
///
/// `deleteNote` is NOT here — it is `server-style: manual` in
/// `openapi-modelgen.yaml`, so we wire it by hand in `main.rs`.
pub struct NotesApi;

impl Api for NotesApi {
    type State = AppState;
    type Error = AppError;
    type Ctx = (); // no general extractor needed
    type Auth = AuthUser; // guards the secured `create_note`

    // `query` is already validated by the generated handler before we run.
    // The array response (`type: array, items: $ref Note`) is typed as `Vec<Note>`.
    async fn list_notes(
        state: AppState,
        _ctx: (),
        query: ListNotesQuery,
    ) -> Result<Vec<Note>, AppError> {
        let notes = state.notes.lock().unwrap();
        Ok(notes
            .iter()
            .filter(|n| match &query.tag {
                Some(tag) => n.tag.as_deref() == Some(tag.as_str()),
                None => true,
            })
            .take(query.limit as usize)
            .cloned()
            .collect())
    }

    // Reached only with a valid bearer token (via `AuthUser`) and a validated body.
    async fn create_note(
        state: AppState,
        _ctx: (),
        _auth: AuthUser,
        body: NewNote,
    ) -> Result<Note, AppError> {
        let mut notes = state.notes.lock().unwrap();
        let note = Note {
            id: (notes.len() + 1).to_string(),
            text: body.text,
            tag: body.tag,
        };
        notes.push(note.clone());
        Ok(note)
    }

    async fn get_note(state: AppState, _ctx: (), id: String) -> Result<Note, AppError> {
        state
            .notes
            .lock()
            .unwrap()
            .iter()
            .find(|n| n.id == id)
            .cloned()
            .ok_or(AppError::NotFound)
    }
}
