// This file is @generated — do not edit manually.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub text: String,
    pub tag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewNote {
    pub text: String,
    pub tag: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListNotesQuery {
    #[serde(default = "crate::default::default_list_notes_query_limit")]
    pub limit: i32,
    pub tag: Option<String>,
}
