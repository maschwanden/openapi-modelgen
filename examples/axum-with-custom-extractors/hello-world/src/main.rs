mod error;
mod extract;

use std::sync::Mutex;

use axum::{Json, Router, http::StatusCode, routing::get};
use chrono::Utc;
use hello_world_openapi::{CreateGreetingRequest, Greeting, GreetingLanguage, ListGreetingsQuery};

use crate::{
    error::AppError,
    extract::{JsonV, QueryV},
};

static NEXT_ID: Mutex<i64> = Mutex::new(3);

#[tokio::main]
async fn main() {
    let app = Router::new().route("/greetings", get(list_greetings).post(create_greeting));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();
    println!("listening on http://127.0.0.1:3000");
    axum::serve(listener, app).await.unwrap();
}

/// GET /greetings?language=de&limit=10
///
/// `QueryV` deserializes the query string and validates constraints (e.g. limit
/// between 1 and 100) before the handler runs. Invalid requests never reach
/// this function.
async fn list_greetings(
    QueryV(params): QueryV<ListGreetingsQuery>,
) -> Result<Json<Vec<Greeting>>, AppError> {
    let limit = params.limit.unwrap_or(100) as usize;

    let greetings: Vec<Greeting> = seed_greetings()
        .into_iter()
        .filter(|g| {
            params
                .language
                .as_ref()
                .is_none_or(|lang| lang == &format!("{:?}", g.language).to_lowercase())
        })
        .take(limit)
        .collect();

    Ok(Json(greetings))
}

/// POST /greetings  { "message": "...", "language": "en", ... }
///
/// `JsonV` deserializes the JSON body and validates constraints (e.g. message
/// length between 1 and 280, tags max 10 unique items) before the handler runs.
async fn create_greeting(
    JsonV(body): JsonV<CreateGreetingRequest>,
) -> Result<(StatusCode, Json<Greeting>), AppError> {
    let mut next_id = NEXT_ID.lock().unwrap();
    let id = *next_id;
    *next_id += 1;

    let greeting = Greeting {
        id,
        message: body.message,
        language: body.language,
        created_at: Utc::now(),
        expires_on: body.expires_on,
        tags: body.tags,
    };

    Ok((StatusCode::CREATED, Json(greeting)))
}

fn seed_greetings() -> Vec<Greeting> {
    vec![
        Greeting {
            id: 0,
            message: "Hello, world!".into(),
            language: GreetingLanguage::En,
            created_at: Utc::now(),
            expires_on: None,
            tags: Some(vec!["welcome".into()]),
        },
        Greeting {
            id: 1,
            message: "Hallo, Welt!".into(),
            language: GreetingLanguage::De,
            created_at: Utc::now(),
            expires_on: None,
            tags: Some(vec!["welcome".into()]),
        },
        Greeting {
            id: 2,
            message: "Bonjour, le monde!".into(),
            language: GreetingLanguage::Fr,
            created_at: Utc::now(),
            expires_on: None,
            tags: None,
        },
    ]
}
