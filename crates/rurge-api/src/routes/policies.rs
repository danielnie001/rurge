//! `GET /v1/policies`, `POST /v1/policies/test` and `GET /v1/rules`.

use crate::App;
use crate::error::{ApiError, ApiResult, json_body};
use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use rurge_engine::TestResult;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::time::UNIX_EPOCH;
use url::Url;

#[derive(Serialize)]
pub struct PoliciesJson {
    pub proxies: Vec<String>,
    #[serde(rename = "policy-groups")]
    pub policy_groups: Vec<String>,
}

pub async fn policies(State(app): State<App>) -> Json<PoliciesJson> {
    let view = app.engine.policies_view();
    Json(PoliciesJson {
        proxies: view.proxies,
        policy_groups: view.groups,
    })
}

#[derive(Deserialize)]
pub struct TestPolicies {
    pub policy_names: Vec<String>,
    /// Test every policy here instead of at its own test URL.
    #[serde(default)]
    pub url: Option<String>,
}

/// `POST /v1/policies/test` (phase 2 M3 design 6.6): `{"<name>": Result…}`.
/// The manual gives no response sample: the shape is provisional.
pub async fn test(
    State(app): State<App>,
    body: Result<Json<TestPolicies>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = json_body(body)?;
    let url = match body.url.as_deref() {
        None | Some("") => None,
        Some(text) => {
            Some(Url::parse(text).map_err(|_| ApiError::bad_request("`url` is not a URL"))?)
        }
    };
    let results = app
        .engine
        .test_policies(&body.policy_names, url)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let mut out = Map::new();
    for (name, result) in results {
        let result = match result {
            Some(result) => result_json(&result),
            None => json!({ "error": "not testable" }),
        };
        out.insert(name, result);
    }
    Ok(Json(Value::Object(out)))
}

/// A test result as the API shows it: `delay` in milliseconds, or the
/// `error` the test failed with; `time`, when it ended, in Unix seconds.
pub(crate) fn result_json(result: &TestResult) -> Value {
    let time = result
        .when
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    match &result.outcome {
        Ok(delay) => json!({ "delay": delay.as_millis() as u64, "time": time }),
        Err(error) => json!({ "error": error, "time": time }),
    }
}

#[derive(Serialize)]
pub struct RuleJson {
    pub index: usize,
    pub rule: String,
    pub hits: u64,
}

#[derive(Serialize)]
pub struct RulesJson {
    pub rules: Vec<RuleJson>,
}

pub async fn rules(State(app): State<App>) -> Json<RulesJson> {
    let rules = app
        .engine
        .rules_view()
        .into_iter()
        .map(|r| RuleJson {
            index: r.index,
            rule: r.rule,
            hits: r.hits,
        })
        .collect();
    Json(RulesJson { rules })
}
