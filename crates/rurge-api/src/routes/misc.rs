//! Empty phase-1 collections, `POST /v1/stop`, and the 404 fallback.

use crate::App;
use crate::error::ApiError;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::{Value, json};

pub async fn modules() -> Json<Value> {
    Json(json!({ "enabled": [], "available": [] }))
}

pub async fn scripting() -> Json<Value> {
    Json(json!({ "scripts": [] }))
}

pub async fn events() -> Json<Value> {
    Json(json!({ "events": [] }))
}

/// Answers `{}` first, then asks the daemon to stop: the stop cancels the
/// API's shutdown token, and a response still being written would race it.
pub async fn stop(State(app): State<App>) -> Json<Value> {
    tracing::info!("stop requested via http-api");
    let control = app.control.clone();
    tokio::spawn(async move { control.stop().await });
    Json(json!({}))
}

pub async fn not_found() -> ApiError {
    ApiError::not_found("no such endpoint")
}

/// A registered path reached with the wrong method: axum's own 405 has an
/// empty body, which would break "every error is `{"error":…}`".
pub async fn method_not_allowed() -> ApiError {
    ApiError::new(StatusCode::METHOD_NOT_ALLOWED, "method not allowed")
}
