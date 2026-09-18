//! `GET /v1/profiles/current`, `POST /v1/profiles/reload|check` (M4 D3).

use crate::App;
use crate::error::{ApiError, ApiResult, query_params};
use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::IntoResponse;
use rurge_config::Severity;
use rurge_config::config::load;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize)]
pub struct CurrentQuery {
    pub sensitive: Option<u8>,
}

pub async fn current(
    State(app): State<App>,
    q: Result<Query<CurrentQuery>, QueryRejection>,
) -> ApiResult<impl IntoResponse> {
    let q = query_params(q)?;
    let sensitive = q.sensitive.unwrap_or(0) != 0;
    let text = app
        .engine
        .config_text(sensitive)
        .await
        .map_err(|e| ApiError::internal(format!("cannot read the profile: {e}")))?;
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReloadJson {
    pub ok: bool,
    pub errors: usize,
    pub warnings: usize,
    pub listeners_rebound: bool,
}

pub async fn reload(State(app): State<App>) -> Json<ReloadJson> {
    let report = app.control.reload().await;
    Json(ReloadJson {
        ok: report.ok,
        errors: report.errors,
        warnings: report.warnings,
        listeners_rebound: report.listeners_rebound,
    })
}

#[derive(Serialize)]
pub struct CheckJson {
    pub ok: bool,
    pub errors: usize,
    pub warnings: usize,
    pub diagnostics: Vec<Value>,
}

/// Re-validates the profile on disk with the daemon's load options; never
/// touches the running config.
pub async fn check(State(app): State<App>) -> ApiResult<Json<CheckJson>> {
    let path = app.engine.profile_path();
    let opts = app.load_options.clone();
    let loaded = tokio::task::spawn_blocking(move || load(&path, &opts))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map_err(|e| ApiError::internal(format!("cannot load the profile: {e}")))?;
    let diagnostics = loaded.diagnostics.sorted();
    let errors = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let warnings = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .count();
    Ok(Json(CheckJson {
        ok: errors == 0,
        errors,
        warnings,
        diagnostics: diagnostics
            .iter()
            .map(|d| serde_json::to_value(d).unwrap_or(Value::Null))
            .collect(),
    }))
}
