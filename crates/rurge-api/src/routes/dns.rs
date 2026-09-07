//! `GET /v1/dns`, `POST /v1/dns/flush`, `POST /v1/test/dns_delay`.

use crate::App;
use crate::error::{ApiError, ApiResult, json_body};
use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheJson {
    pub domain: String,
    pub data: Vec<String>,
    pub expires_time: Option<f64>,
    pub server: String,
    pub stale: bool,
    pub negative: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsJson {
    pub dns_cache: Vec<CacheJson>,
    pub upstreams: Vec<String>,
    pub bootstrap: Vec<String>,
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub async fn dns(State(app): State<App>) -> Json<DnsJson> {
    let resolver = app.engine.runtime().stack.resolver.clone();
    let now = now_secs();
    let dns_cache = resolver
        .cache_snapshot()
        .into_iter()
        .map(|e| CacheJson {
            domain: e.name,
            data: e
                .v4
                .iter()
                .map(|a| a.to_string())
                .chain(e.v6.iter().map(|a| a.to_string()))
                .collect(),
            expires_time: e.expires_in.map(|d| now + d.as_secs_f64()),
            server: e.source,
            stale: e.stale,
            negative: e.negative,
        })
        .collect();
    Json(DnsJson {
        dns_cache,
        upstreams: resolver.primary_upstreams(),
        bootstrap: resolver.bootstrap_upstreams(),
    })
}

pub async fn flush(State(app): State<App>) -> Json<Value> {
    app.engine.runtime().stack.resolver.flush();
    tracing::info!("dns cache flushed via http-api");
    Json(json!({}))
}

#[derive(Deserialize, Default)]
pub struct DelayBody {
    pub name: Option<String>,
}

#[derive(Serialize)]
pub struct DelayJson {
    pub upstream: String,
    pub ms: Option<u64>,
    pub error: Option<String>,
}

/// The host part of a URL without pulling in a URL parser: strips the scheme,
/// userinfo, port, path and query.
pub(crate) fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next()?,
        None => authority.split(':').next()?,
    };
    (!host.is_empty()).then(|| host.to_string())
}

pub async fn dns_delay(
    State(app): State<App>,
    body: Result<Json<DelayBody>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = json_body(body)?;
    let rt = app.engine.runtime();
    let name = match body.name.filter(|n| !n.trim().is_empty()) {
        Some(n) => n,
        None => host_of(&rt.config.general.internet_test_url).ok_or_else(|| {
            ApiError::bad_request("no name given and internet-test-url has no host")
        })?,
    };
    let delays: Vec<DelayJson> = rt
        .stack
        .resolver
        .measure_delay(&name)
        .await
        .into_iter()
        .map(|d| match d.result {
            Ok(elapsed) => DelayJson {
                upstream: d.upstream,
                ms: Some(elapsed.as_millis() as u64),
                error: None,
            },
            Err(e) => DelayJson {
                upstream: d.upstream,
                ms: None,
                error: Some(e),
            },
        })
        .collect();
    Ok(Json(json!({ "delays": delays })))
}

#[cfg(test)]
mod tests {
    use super::host_of;

    #[test]
    fn host_of_extracts_the_authority_host() {
        assert_eq!(host_of("http://bing.com/").as_deref(), Some("bing.com"));
        assert_eq!(
            host_of("http://target.test:8080/hello?x=1").as_deref(),
            Some("target.test")
        );
        assert_eq!(
            host_of("https://user:pw@example.com").as_deref(),
            Some("example.com")
        );
        assert_eq!(host_of("http://[::1]:53/").as_deref(), Some("::1"));
        assert_eq!(host_of(""), None);
    }
}
