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
#[serde(tag = "channel")]
pub enum DeliveryChannel {
    #[serde(rename = "email")]
    EmailDelivery(EmailDelivery),
    #[serde(rename = "sms")]
    SmsDelivery(SmsDelivery),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Attachment {
    ImageAttachment(ImageAttachment),
    LinkAttachment(LinkAttachment),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Greeting {
    pub id: i64,
    pub message: String,
    pub language: GreetingLanguage,
    pub created_at: DateTime<Utc>,
    pub expires_on: Option<NaiveDate>,
    pub tags: Option<Vec<String>>,
    pub attachment: Option<Attachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateGreetingRequest {
    pub message: String,
    #[serde(default = "crate::default::default_create_greeting_request_language")]
    pub language: GreetingLanguage,
    pub expires_on: Option<NaiveDate>,
    pub tags: Option<Vec<String>>,
    pub delivery: Option<DeliveryChannel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailDelivery {
    pub address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmsDelivery {
    pub phone_number: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAttachment {
    pub url: String,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkAttachment {
    pub href: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListGreetingsQuery {
    pub language: Option<String>,
    #[serde(default = "crate::default::default_list_greetings_query_limit")]
    pub limit: i32,
}
