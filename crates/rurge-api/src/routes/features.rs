//! `GET/POST /v1/features/{name}` (M4 design §5.2): only `system_proxy` is
//! live in phase 1; the others read as off and cannot be switched.

use crate::App;
use crate::error::{ApiError, ApiResult, json_body};
use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use serde::Deserialize;
use serde_json::{Value, json};

const FEATURES: [&str; 6] = [
    "system_proxy",
    "enhanced_mode",
    "mitm",
    "capture",
    "rewrite",
    "scripting",
];

fn known(name: &str) -> ApiResult<()> {
    if FEATURES.contains(&name) {
        Ok(())
    } else {
        Err(ApiError::not_found(format!("unknown feature `{name}`")))
    }
}

pub async fn get_feature(
    State(app): State<App>,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    known(&name)?;
    let enabled = name == "system_proxy" && app.control.system_proxy_enabled();
    Ok(Json(json!({ "enabled": enabled })))
}

#[derive(Deserialize)]
pub struct SetFeature {
    pub enabled: bool,
}

pub async fn set_feature(
    State(app): State<App>,
    Path(name): Path<String>,
    body: Result<Json<SetFeature>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    known(&name)?;
    let body = json_body(body)?;
    if name != "system_proxy" {
        return Err(ApiError::not_implemented(format!(
            "feature `{name}` is not available in this phase"
        )));
    }
    app.control
        .set_system_proxy(body.enabled)
        .await
        .map_err(ApiError::internal)?;
    tracing::info!(enabled = body.enabled, "system proxy switched via http-api");
    Ok(Json(json!({})))
}
