//! `GET /v1/requests/recent|active` and `POST /v1/requests/kill`.

use crate::App;
use crate::error::{ApiError, ApiResult, json_body, query_params};
use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Query, State};
use rurge_config::rule::ProtocolKind;
use rurge_config::session::ListenerKind;
use rurge_engine::{RecordStatus, RequestRecord};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const DEFAULT_LIMIT: usize = 100;

pub(crate) fn listener_name(kind: ListenerKind) -> &'static str {
    match kind {
        ListenerKind::Http => "http",
        ListenerKind::Socks5 => "socks5",
        ListenerKind::Tun => "tun",
        ListenerKind::Forward => "forward",
        ListenerKind::Internal => "internal",
    }
}

fn protocol_name(p: ProtocolKind) -> &'static str {
    match p {
        ProtocolKind::Http => "http",
        ProtocolKind::Https => "https",
        ProtocolKind::Tcp => "tcp",
        ProtocolKind::Udp => "udp",
        ProtocolKind::Quic => "quic",
        ProtocolKind::Stun => "stun",
        ProtocolKind::MtProto => "mtproto",
        ProtocolKind::Doh => "doh",
        ProtocolKind::Doh3 => "doh3",
        ProtocolKind::Doq => "doq",
        ProtocolKind::Dot => "dot",
        ProtocolKind::Dns => "dns",
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestJson {
    pub id: u64,
    pub listener: &'static str,
    pub src: String,
    pub dst: String,
    pub rule: Option<String>,
    pub policy: Vec<String>,
    pub sni: Option<String>,
    pub protocol: Option<&'static str>,
    pub up: u64,
    pub down: u64,
    pub started_ms: u64,
    pub elapsed_ms: u64,
    pub status: &'static str,
    pub reject_kind: Option<String>,
    pub error: Option<String>,
}

impl From<&RequestRecord> for RequestJson {
    fn from(r: &RequestRecord) -> RequestJson {
        let (status, reject_kind) = match &r.status {
            RecordStatus::Active => ("active", None),
            RecordStatus::Completed => ("completed", None),
            RecordStatus::Rejected(kind) => ("rejected", Some(kind.clone())),
            RecordStatus::Failed => ("failed", None),
        };
        RequestJson {
            id: r.id,
            listener: listener_name(r.listener),
            src: r.src.to_string(),
            dst: r.dst.clone(),
            rule: r.rule.clone(),
            policy: r.policy.clone(),
            sni: r.sni.clone(),
            protocol: r.protocol.map(protocol_name),
            up: r.up,
            down: r.down,
            started_ms: r.started_ms,
            elapsed_ms: r.elapsed_ms,
            status,
            reject_kind,
            error: r.error.clone(),
        }
    }
}

#[derive(Serialize)]
pub struct RequestsJson {
    pub requests: Vec<RequestJson>,
}

#[derive(Deserialize)]
pub struct RecentQuery {
    pub limit: Option<usize>,
}

pub async fn recent(
    State(app): State<App>,
    q: Result<Query<RecentQuery>, QueryRejection>,
) -> ApiResult<Json<RequestsJson>> {
    let q = query_params(q)?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).max(1);
    let requests = app
        .engine
        .request_log()
        .recent(limit)
        .iter()
        .map(RequestJson::from)
        .collect();
    Ok(Json(RequestsJson { requests }))
}

pub async fn active(State(app): State<App>) -> Json<RequestsJson> {
    let requests = app
        .engine
        .request_log()
        .active()
        .iter()
        .map(RequestJson::from)
        .collect();
    Json(RequestsJson { requests })
}

#[derive(Deserialize)]
pub struct KillBody {
    pub id: u64,
}

/// 404 when the id is not in flight, 409 for rurge's own sessions (M4 §12).
pub(crate) fn kill_check(active: &[RequestRecord], id: u64) -> ApiResult<()> {
    match active.iter().find(|r| r.id == id) {
        None => Err(ApiError::not_found(format!(
            "no active request with id {id}"
        ))),
        Some(r) if r.listener == ListenerKind::Internal => {
            Err(ApiError::conflict("not killable: internal session"))
        }
        Some(_) => Ok(()),
    }
}

pub async fn kill(
    State(app): State<App>,
    body: Result<Json<KillBody>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = json_body(body)?;
    kill_check(&app.engine.request_log().active(), body.id)?;
    if !app.engine.kill(body.id) {
        return Err(ApiError::not_found(format!(
            "no active request with id {}",
            body.id
        )));
    }
    tracing::info!(id = body.id, "request killed via http-api");
    Ok(Json(json!({})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_engine::RecordStatus;

    fn record(id: u64, listener: ListenerKind) -> RequestRecord {
        RequestRecord {
            id,
            listener,
            src: "127.0.0.1:1".parse().unwrap(),
            dst: "1.1.1.1:443".into(),
            rule: None,
            policy: vec![],
            sni: None,
            protocol: None,
            up: 0,
            down: 0,
            started_ms: 0,
            elapsed_ms: 0,
            status: RecordStatus::Active,
            error: None,
        }
    }

    #[test]
    fn kill_check_distinguishes_missing_internal_and_killable() {
        let active = [
            record(1, ListenerKind::Http),
            record(2, ListenerKind::Internal),
        ];
        assert!(kill_check(&active, 1).is_ok());
        let e = kill_check(&active, 2).unwrap_err();
        assert_eq!(e.status.as_u16(), 409);
        assert_eq!(e.message, "not killable: internal session");
        let e = kill_check(&active, 3).unwrap_err();
        assert_eq!(e.status.as_u16(), 404);
    }

    #[test]
    fn json_names_are_lowercase_and_reject_kind_is_split_out() {
        let mut r = record(1, ListenerKind::Socks5);
        r.status = RecordStatus::Rejected("REJECT-TINYGIF".into());
        r.protocol = Some(rurge_config::rule::ProtocolKind::MtProto);
        let j = RequestJson::from(&r);
        assert_eq!(j.listener, "socks5");
        assert_eq!(j.status, "rejected");
        assert_eq!(j.reject_kind.as_deref(), Some("REJECT-TINYGIF"));
        assert_eq!(j.protocol, Some("mtproto"));
    }
}
