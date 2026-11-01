//! `GET /v1/rurge/capture…` and `POST /v1/rurge/capture/clear` (phase 4 M4
//! design 7.1 … 7.6): what the HTTP engine captured, read out of the store
//! `Engine::capture()` hands over. A handler takes the entries out of the
//! store and lets its lock go before it writes them: what is read stays whole
//! even when the store evicts or clears it in between. Nothing captured goes
//! into a log line or an error text, and the values of the four credential
//! headers are never in an answer.

use crate::App;
use crate::error::{ApiError, ApiResult, query_params};
use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rurge_engine::capture::{Body, Entry, Head, NotDecoded, Outcome, ResponseHead, Truncated};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

/// How many entries `/v1/rurge/capture/sessions/{name}` lists when the query
/// says nothing, as `/v1/requests/recent` has it.
const DEFAULT_LIMIT: usize = 100;

/// What `POST /v1/rurge/capture/clear` says of a body it cannot take a session
/// name from. Fixed: nothing the caller sent comes back in it.
const BAD_BODY: &str = "the body must be a JSON object; \"session\", when given, must be a string";

/// The headers whose values are always `***` (`phase 4 M4 design 6.6`): a
/// credential of the client's or of rurge's does not leave the store.
const MASKED: [&str; 4] = [
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
];

#[derive(Serialize)]
pub struct LimitsJson {
    pub total: u64,
    pub body: u64,
}

/// What the store holds (`phase 4 M4 design 7.1`).
#[derive(Serialize)]
pub struct StatusJson {
    pub limits: LimitsJson,
    pub used: u64,
    pub entries: usize,
    /// The four counts since the engine started: what was stored, evicted,
    /// dropped, and stored with a body that was truncated.
    pub stored: u64,
    pub evicted: u64,
    pub dropped: u64,
    pub truncated: u64,
}

/// A session and what it costs (`phase 4 M4 design 7.2`).
#[derive(Serialize)]
pub struct SessionJson {
    pub name: String,
    pub entries: usize,
    pub bytes: u64,
}

/// One entry as the list of its session gives it (`phase 4 M4 design 7.3`).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryJson {
    pub id: u64,
    pub started_ms: u64,
    pub method: String,
    pub url: String,
    /// The status of the response, `null` when the request was answered none.
    pub status: Option<u16>,
    pub outcome: &'static str,
    pub request_body_size: u64,
    pub response_body_size: u64,
    /// Whether the store kept only a part of a body.
    pub truncated: bool,
    pub total_ms: u64,
}

#[derive(Deserialize)]
pub struct SessionQuery {
    pub limit: Option<usize>,
}

#[derive(Deserialize)]
pub struct EntryQuery {
    pub decode: Option<String>,
}

pub async fn status(State(app): State<App>) -> Json<StatusJson> {
    let status = app.engine.capture().status();
    Json(StatusJson {
        limits: LimitsJson {
            total: status.limits.total,
            body: status.limits.body,
        },
        used: status.used,
        entries: status.entries,
        stored: status.stored,
        evicted: status.evicted,
        dropped: status.dropped,
        truncated: status.truncated,
    })
}

pub async fn sessions(State(app): State<App>) -> Json<Vec<SessionJson>> {
    let sessions = app
        .engine
        .capture()
        .sessions()
        .into_iter()
        .map(|s| SessionJson {
            name: s.name.to_string(),
            entries: s.entries,
            bytes: s.bytes,
        })
        .collect();
    Json(sessions)
}

pub async fn session(
    State(app): State<App>,
    Path(name): Path<String>,
    q: Result<Query<SessionQuery>, QueryRejection>,
) -> ApiResult<Json<Vec<SummaryJson>>> {
    let q = query_params(q)?;
    let limit = match q.limit {
        Some(0) => return Err(ApiError::bad_request("limit must be at least 1")),
        Some(n) => n,
        None => DEFAULT_LIMIT,
    };
    let entries = app.engine.capture().session(&name);
    if entries.is_empty() {
        return Err(ApiError::not_found(format!(
            "unknown capture session `{name}`"
        )));
    }
    // the store keeps them oldest first; the API lists the newest first
    let listed = entries
        .iter()
        .rev()
        .take(limit)
        .map(|entry| SummaryJson {
            id: entry.id,
            started_ms: entry.started_ms(),
            method: entry.request.method.as_str().to_string(),
            url: entry.request.url.clone(),
            status: entry.response.as_ref().map(|r| r.status.as_u16()),
            outcome: outcome_name(entry.outcome),
            request_body_size: entry.request_body.size,
            response_body_size: entry.response_body.size,
            truncated: entry.request_body.truncated.is_some()
                || entry.response_body.truncated.is_some(),
            total_ms: millis(entry.timings.total),
        })
        .collect();
    Ok(Json(listed))
}

pub async fn entry(
    State(app): State<App>,
    Path(id): Path<String>,
    q: Result<Query<EntryQuery>, QueryRejection>,
) -> ApiResult<Json<Value>> {
    let decode = match query_params(q)?.decode.as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => return Err(ApiError::bad_request("decode must be true or false")),
    };
    let id: u64 = id
        .parse()
        .map_err(|_| ApiError::bad_request("entry id must be a number"))?;
    let store = app.engine.capture();
    let entry = store
        .entry(id)
        .ok_or_else(|| ApiError::not_found(format!("unknown capture entry {id}")))?;
    Ok(Json(entry_json(&entry, decode, store.limits().body)))
}

/// What a request took (`phase 4 M4 design 7.5`), in whole milliseconds;
/// what the engine could not tell is `null`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimingsJson {
    pub connect_ms: Option<u64>,
    pub dns_ms: Option<u64>,
    pub wait_ms: Option<u64>,
    pub receive_ms: Option<u64>,
    pub total_ms: u64,
    pub reused: bool,
}

/// One entry whole (`phase 4 M4 design 7.5`): the request, its response if
/// there was one, and everything the store knows besides.
fn entry_json(entry: &Entry, decode: bool, body_limit: u64) -> Value {
    let response = entry
        .response
        .as_ref()
        .map(|head| response_json(head, &entry.response_body, decode, body_limit));
    json!({
        "id": entry.id,
        "session": entry.session.as_ref(),
        "startedMs": entry.started_ms(),
        "request": head_json(&entry.request, &entry.request_body, decode, body_limit),
        "response": response,
        "timings": TimingsJson {
            connect_ms: entry.timings.connect.map(millis),
            dns_ms: entry.timings.dns.map(millis),
            wait_ms: entry.timings.wait.map(millis),
            receive_ms: entry.timings.receive.map(millis),
            total_ms: millis(entry.timings.total),
            reused: entry.timings.reused,
        },
        "remoteAddr": entry.remote.map(|addr| addr.to_string()),
        "outcome": outcome_name(entry.outcome),
        "rejectKind": reject_kind(entry.outcome),
        "error": &entry.error,
        "rule": &entry.rule,
        "policy": &entry.policy,
        "notes": &entry.notes,
    })
}

fn head_json(head: &Head, body: &Body, decode: bool, body_limit: u64) -> Value {
    let decoded = if decode {
        Some(body.decoded(&head.headers, body_limit))
    } else {
        None
    };
    json!({
        "method": head.method.as_str(),
        "url": head.url,
        "version": format!("{:?}", head.version),
        "headers": headers_json(&head.headers),
        "body": body_json(body, decoded),
    })
}

fn response_json(head: &ResponseHead, body: &Body, decode: bool, body_limit: u64) -> Value {
    let decoded = if decode {
        Some(body.decoded(&head.headers, body_limit))
    } else {
        None
    };
    json!({
        "status": head.status.as_u16(),
        "version": format!("{:?}", head.version),
        "headers": headers_json(&head.headers),
        "body": body_json(body, decoded),
    })
}

/// The headers of one side in the order the map keeps them, each as a name and
/// a value: the bytes of a value that are no UTF-8 are shown as U+FFFD, and
/// the values of the four credential headers are `***`, never read out (the
/// store keeps them as they came).
fn headers_json(headers: &HeaderMap) -> Value {
    Value::Array(
        headers
            .iter()
            .map(|(name, value)| {
                let value = if MASKED.contains(&name.as_str()) {
                    "***".to_string()
                } else {
                    String::from_utf8_lossy(value.as_bytes()).into_owned()
                };
                json!({ "name": name.as_str(), "value": value })
            })
            .collect(),
    )
}

/// What was kept of a body, and, when `decode` asked, what it decodes to
/// (`phase 4 M4 design 7.5`).
fn body_json(body: &Body, decoded: Option<Result<Option<Bytes>, NotDecoded>>) -> Value {
    let mut value = json!({
        "size": body.size,
        "stored": body.data.len() as u64,
        "truncated": match body.truncated {
            None => Value::Null,
            Some(Truncated::Limit) => json!("limit"),
            Some(Truncated::Budget) => json!("budget"),
        },
        "base64": STANDARD.encode(&body.data),
    });
    let Some(decoded) = decoded else {
        return value;
    };
    match decoded {
        Ok(Some(data)) => {
            value["base64"] = json!(STANDARD.encode(&data));
            value["decoded"] = json!(true);
            value["notDecoded"] = Value::Null;
        }
        Ok(None) => {
            value["decoded"] = json!(false);
            value["notDecoded"] = Value::Null;
        }
        Err(reason) => {
            value["decoded"] = json!(false);
            value["notDecoded"] = json!(reason.to_string());
        }
    }
    value
}

/// How a request ended (`phase 4 M4 design 7.3`).
fn outcome_name(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Completed => "completed",
        Outcome::Upgraded => "upgraded",
        Outcome::Rejected(_) => "rejected",
        Outcome::Failed => "failed",
    }
}

/// The REJECT flavour of a refused request, `null` for the others.
fn reject_kind(outcome: Outcome) -> Option<&'static str> {
    match outcome {
        Outcome::Rejected(kind) => Some(kind.name()),
        _ => None,
    }
}

/// How long something took, in whole milliseconds, rounded down.
fn millis(d: Duration) -> u64 {
    d.as_millis() as u64
}

pub async fn clear(
    State(app): State<App>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Json<Value>> {
    let session = requested_session(body)?;
    let cleared = app.engine.capture().clear(session.as_deref());
    Ok(Json(json!({ "cleared": cleared })))
}

/// The `session` a clear request names, if it names one, or why the body has
/// none to read (`phase 4 M4 design 7.6`). The body is read here and not by an
/// extractor that answers for itself: the text such an extractor gives for a
/// body of the wrong type quotes what was sent.
fn requested_session(body: Result<Bytes, BytesRejection>) -> ApiResult<Option<String>> {
    let bad = || ApiError::bad_request(BAD_BODY);
    let body = body.map_err(|_| bad())?;
    // No body is every session, and neither is a body of nothing but
    // whitespace.
    if body
        .iter()
        .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
    {
        return Ok(None);
    }
    match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(mut fields)) => match fields.remove("session") {
            None => Ok(None),
            Some(Value::String(name)) => Ok(Some(name)),
            Some(_) => Err(bad()),
        },
        _ => Err(bad()),
    }
}
