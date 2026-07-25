use std::sync::{Arc, Mutex};

use notes_openapi::Note;

/// Shared application state. This is the `type State` the generated `Api`
/// trait is generic over — modelgen never dictates it.
#[derive(Clone)]
pub struct AppState {
    pub notes: Arc<Mutex<Vec<Note>>>,
}

impl AppState {
    pub fn seeded() -> Self {
        AppState {
            notes: Arc::new(Mutex::new(vec![
                Note {
                    id: "1".into(),
                    text: "Buy milk".into(),
                    tag: Some("errand".into()),
                },
                Note {
                    id: "2".into(),
                    text: "Read a book".into(),
                    tag: None,
                },
            ])),
        }
    }
}
