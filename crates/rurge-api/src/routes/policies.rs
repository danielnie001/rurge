//! `GET /v1/policies` and `GET /v1/rules`.

use crate::App;
use axum::Json;
use axum::extract::State;
use serde::Serialize;

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
