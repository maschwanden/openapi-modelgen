// This file is @generated — do not edit manually.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GreetingLanguage {
    #[serde(rename = "en")]
    En,
    #[serde(rename = "de")]
    De,
    #[serde(rename = "fr")]
    Fr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Greeting {
    pub id: i64,
    pub message: String,
    pub language: GreetingLanguage,
    pub created_at: DateTime<Utc>,
    pub expires_on: Option<NaiveDate>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateGreetingRequest {
    pub message: String,
    #[serde(default = "crate::default::default_create_greeting_request_language")]
    pub language: GreetingLanguage,
    pub expires_on: Option<NaiveDate>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListGreetingsQuery {
    #[serde(default = "crate::default::default_list_greetings_query_language")]
    pub language: String,
    #[serde(default = "crate::default::default_list_greetings_query_limit")]
    pub limit: i32,
}
