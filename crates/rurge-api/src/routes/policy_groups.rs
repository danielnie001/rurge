//! `GET /v1/policies/detail`, `GET /v1/policy_groups` and
//! `GET/POST /v1/policy_groups/select` (M1 design 6.6). The first two have no
//! response sample in the manual: their shapes are provisional.

use crate::App;
use crate::error::{ApiError, ApiResult, json_body, query_params};
use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

#[derive(Deserialize)]
pub struct DetailQuery {
    pub policy_name: String,
}

pub async fn detail(
    State(app): State<App>,
    query: Result<Query<DetailQuery>, QueryRejection>,
) -> ApiResult<Json<Value>> {
    let query = query_params(query)?;
    let detail = app
        .engine
        .policy_detail(&query.policy_name)
        .ok_or_else(|| ApiError::not_found(format!("unknown policy `{}`", query.policy_name)))?;
    let mut body = Map::new();
    body.insert(query.policy_name, Value::String(detail));
    Ok(Json(Value::Object(body)))
}

#[derive(Serialize)]
struct MemberJson {
    name: String,
    #[serde(rename = "typeDescription")]
    type_description: String,
    #[serde(rename = "isGroup")]
    is_group: bool,
    /// Always `true`: rurge has no way to disable a member.
    enabled: bool,
    #[serde(rename = "lineHash")]
    line_hash: String,
}

pub async fn groups(State(app): State<App>) -> Json<Value> {
    let mut body = Map::new();
    for group in app.engine.groups_view() {
        let members: Vec<MemberJson> = group
            .members
            .into_iter()
            .map(|m| MemberJson {
                name: m.name,
                type_description: m.type_description,
                is_group: m.is_group,
                enabled: true,
                line_hash: m.line_hash,
            })
            .collect();
        body.insert(group.name, json!(members));
    }
    Json(Value::Object(body))
}

#[derive(Deserialize)]
pub struct SelectionQuery {
    pub group_name: String,
}

pub async fn selection(
    State(app): State<App>,
    query: Result<Query<SelectionQuery>, QueryRejection>,
) -> ApiResult<Json<Value>> {
    let query = query_params(query)?;
    let policy = app
        .engine
        .group_selection(&query.group_name)
        .map_err(|e| ApiError::not_found(e.to_string()))?;
    // A validly configured group without members (e.g. `subnet`, which keeps
    // its targets in `conditions` / `default`) yields "" here, not an error.
    Ok(Json(json!({ "policy": policy })))
}

#[derive(Deserialize)]
pub struct Select {
    pub group_name: String,
    pub policy: String,
}

pub async fn select(
    State(app): State<App>,
    body: Result<Json<Select>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = json_body(body)?;
    app.engine
        .select_group(&body.group_name, &body.policy)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    tracing::info!(group = %body.group_name, policy = %body.policy, "policy group selection changed via http-api");
    Ok(Json(json!({})))
}
