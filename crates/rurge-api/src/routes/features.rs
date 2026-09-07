//! `GET/POST /v1/features/{name}` (M4 design §5.2): every feature reads as
//! off in phase 1; only `system_proxy` can be switched, and only once M4b
//! implements `Control::set_system_proxy`.

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

pub async fn get_feature(Path(name): Path<String>) -> ApiResult<Json<Value>> {
    known(&name)?;
    Ok(Json(json!({ "enabled": false })))
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
    match app.control.set_system_proxy(body.enabled).await {
        Ok(()) => Ok(Json(json!({}))),
        Err(e) if e.contains("not implemented") => Err(ApiError::not_implemented(e)),
        Err(e) => Err(ApiError::internal(e)),
    }
}
