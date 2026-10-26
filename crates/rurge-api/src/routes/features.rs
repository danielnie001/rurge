//! `GET/POST /v1/features/{name}` (M4 design §5.2): `system_proxy` and
//! `enhanced_mode` are live; the others read as off and cannot be switched.

use crate::App;
use crate::error::{ApiError, ApiResult, json_body};
use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use rurge_engine::EnhancedStatus;
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

/// The enhanced mode as the API gives it (M3-D22, P12's key names): whether it
/// is on, what its TUN has dropped for each reason, and the panics of the TCP
/// stacks. All zero while it is off.
fn enhanced_json(status: EnhancedStatus) -> Value {
    let c = status.counters;
    json!({
        "enabled": status.enabled,
        "dropped": {
            "malformed": c.malformed,
            "fragment": c.fragment,
            "martian": c.martian,
            "unsupported": c.unsupported,
            "queueFull": c.queue_full,
            "unknownFake": c.unknown_fake,
            "busy": c.busy,
            "beyondWindow": c.beyond_window,
            "rstLimited": c.rst_limited,
            "icmpLimited": c.icmp_limited,
        },
        "stackPanics": c.stack_panics,
    })
}

pub async fn get_feature(
    State(app): State<App>,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    known(&name)?;
    let body = match name.as_str() {
        "system_proxy" => json!({ "enabled": app.control.system_proxy_enabled() }),
        "enhanced_mode" => enhanced_json(app.control.enhanced_mode()),
        _ => json!({ "enabled": false }),
    };
    Ok(Json(body))
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
    match name.as_str() {
        "system_proxy" => {
            app.control
                .set_system_proxy(body.enabled)
                .await
                .map_err(ApiError::internal)?;
            tracing::info!(enabled = body.enabled, "system proxy switched via http-api");
        }
        "enhanced_mode" => {
            app.control
                .set_enhanced_mode(body.enabled)
                .await
                .map_err(ApiError::internal)?;
            tracing::info!(
                enabled = body.enabled,
                "enhanced mode switched via http-api"
            );
        }
        _ => {
            return Err(ApiError::not_implemented(format!(
                "feature `{name}` is not available in this phase"
            )));
        }
    }
    Ok(Json(json!({})))
}
