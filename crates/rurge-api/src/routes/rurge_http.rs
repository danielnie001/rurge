//! `GET /v1/rurge/http`: what rurge's HTTP engine has done (phase 4 M1 design
//! 11). It is rurge's own endpoint, not one Surge has, so it lives under
//! `/v1/rurge/…` — the prefix phase 4's general design gives rurge's own
//! interfaces. M1a has the five ways a sniff can end; M1b adds `requests`.

use crate::App;
use axum::Json;
use axum::extract::State;
use serde::Serialize;

#[derive(Serialize)]
pub struct HttpJson {
    pub sniffed: SniffedJson,
    /// The requests that reached the rules, refused ones included.
    pub requests: u64,
}

/// The outcomes of the sniffs since the engine started, in the order the
/// design writes them: a TLS ClientHello, plain HTTP, something else, nothing
/// within the wait, and a client that went away.
#[derive(Serialize)]
pub struct SniffedJson {
    pub tls: u64,
    pub http: u64,
    pub other: u64,
    pub timeout: u64,
    pub closed: u64,
}

pub async fn http(State(app): State<App>) -> Json<HttpJson> {
    // The engine's counts carry no `Serialize` of their own (plan P16): this
    // endpoint owns the shape, so the fields are copied over one by one.
    let counts = app.engine.http_counts();
    Json(HttpJson {
        sniffed: SniffedJson {
            tls: counts.sniffed.tls,
            http: counts.sniffed.http,
            other: counts.sniffed.other,
            timeout: counts.sniffed.timeout,
            closed: counts.sniffed.closed,
        },
        requests: counts.requests,
    })
}
