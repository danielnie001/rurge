//! `GET /v1/traffic` (Surge's shape: `in` is bytes downloaded, `out` uploaded).

use crate::App;
use crate::routes::requests::listener_name;
use axum::Json;
use axum::extract::State;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize)]
pub struct Bytes {
    #[serde(rename = "in")]
    pub down: u64,
    pub out: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Total {
    #[serde(rename = "in")]
    pub down: u64,
    pub out: u64,
    pub in_current_speed: u64,
    pub out_current_speed: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficJson {
    pub start_time: f64,
    pub total: Total,
    pub connector: BTreeMap<String, Bytes>,
    pub listener: BTreeMap<&'static str, Bytes>,
}

pub async fn traffic(State(app): State<App>) -> Json<TrafficJson> {
    let stats = app.engine.traffic();
    // `total` counts in-flight sessions too, so it stays consistent with the
    // speeds (which the sampler derives from the same `snapshot_bytes`); the
    // per-connector and per-listener maps are finished sessions only.
    let (total_up, total_down) = app.engine.request_log().snapshot_bytes(stats);
    let (rate_up, rate_down) = stats.rate();
    let connector = stats
        .by_policy()
        .into_iter()
        .map(|(name, up, down)| (name, Bytes { down, out: up }))
        .collect();
    let listener = stats
        .by_listener()
        .into_iter()
        .map(|(kind, up, down)| (listener_name(kind), Bytes { down, out: up }))
        .collect::<BTreeMap<_, _>>();
    Json(TrafficJson {
        start_time: app.started_secs,
        total: Total {
            down: total_down,
            out: total_up,
            in_current_speed: rate_down,
            out_current_speed: rate_up,
        },
        connector,
        listener,
    })
}
