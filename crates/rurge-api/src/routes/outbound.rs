//! `GET/POST /v1/outbound` and `/v1/outbound/global` (M4 design §5.2).

use crate::App;
use crate::error::{ApiError, ApiResult, json_body};
use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use rurge_engine::Mode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Serialize)]
pub struct ModeJson {
    pub mode: &'static str,
}

#[derive(Deserialize)]
pub struct SetMode {
    pub mode: String,
}

pub async fn get_mode(State(app): State<App>) -> Json<ModeJson> {
    Json(ModeJson {
        mode: app.engine.mode().as_str(),
    })
}

pub async fn set_mode(
    State(app): State<App>,
    body: Result<Json<SetMode>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = json_body(body)?;
    let mode = Mode::parse(&body.mode).ok_or_else(|| {
        ApiError::bad_request(format!(
            "unknown mode `{}`: expected direct, proxy or rule",
            body.mode
        ))
    })?;
    if mode == Mode::Proxy && app.engine.global_policy().is_none() {
        return Err(ApiError::bad_request(
            "proxy mode needs a global policy; set it with POST /v1/outbound/global first",
        ));
    }
    app.engine.set_mode(mode).await;
    tracing::info!(mode = mode.as_str(), "outbound mode changed via http-api");
    Ok(Json(json!({})))
}

#[derive(Serialize)]
pub struct GlobalJson {
    pub policy: Option<String>,
}

#[derive(Deserialize)]
pub struct SetGlobal {
    pub policy: String,
}

pub async fn get_global(State(app): State<App>) -> Json<GlobalJson> {
    Json(GlobalJson {
        policy: app.engine.global_policy(),
    })
}

pub async fn set_global(
    State(app): State<App>,
    body: Result<Json<SetGlobal>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = json_body(body)?;
    if body.policy.trim().is_empty() && app.engine.mode() == Mode::Proxy {
        return Err(ApiError::bad_request(
            "cannot clear the global policy while the outbound mode is proxy; switch the mode first",
        ));
    }
    app.engine
        .set_global_policy(&body.policy)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    tracing::info!(policy = %body.policy, "global policy changed via http-api");
    Ok(Json(json!({})))
}
