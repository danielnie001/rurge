//! `POST /v1/log/level`.

use crate::App;
use crate::error::{ApiError, ApiResult, json_body};
use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use rurge_engine::LogLevel;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
pub struct LevelBody {
    pub level: String,
}

pub async fn set_level(
    State(app): State<App>,
    body: Result<Json<LevelBody>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = json_body(body)?;
    let level = LogLevel::parse(&body.level).ok_or_else(|| {
        ApiError::bad_request(format!(
            "unknown log level `{}`: expected verbose, debug, info, notify, warning or error",
            body.level
        ))
    })?;
    app.control
        .set_log_level(level)
        .map_err(ApiError::internal)?;
    tracing::info!(level = level.as_str(), "log level changed via http-api");
    Ok(Json(json!({})))
}
