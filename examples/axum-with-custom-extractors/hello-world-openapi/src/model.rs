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
    pub language: GreetingLanguage,
    pub expires_on: Option<NaiveDate>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListGreetingsQuery {
    pub language: Option<String>,
    pub limit: Option<i32>,
}
