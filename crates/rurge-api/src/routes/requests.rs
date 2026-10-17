//! `GET /v1/requests/recent|active` and `POST /v1/requests/kill`.

use crate::App;
use crate::error::{ApiError, ApiResult, json_body, query_params};
use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Query, State};
use rurge_config::rule::ProtocolKind;
use rurge_config::session::{ListenerKind, Transport};
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
    pub transport: &'static str,
    pub src: String,
    pub listen_port: Option<u16>,
    /// The process behind the session; `null` when it is not known (not from
    /// this machine, rurge's own, or not found yet), and the path alone is
    /// `null` for a process known by its name only.
    pub pid: Option<u32>,
    pub process_path: Option<String>,
    pub dst: String,
    pub remote_addr: Option<String>,
    pub rule: Option<String>,
    pub policy: Vec<String>,
    pub sni: Option<String>,
    pub protocol: Option<&'static str>,
    pub up: u64,
    pub down: u64,
    pub started_ms: u64,
    pub elapsed_ms: u64,
    pub connect_ms: Option<u64>,
    pub first_byte_ms: Option<u64>,
    pub dns_ms: Option<u64>,
    pub status: &'static str,
    pub reject_kind: Option<String>,
    pub error: Option<String>,
    pub notes: Vec<String>,
}

impl From<&RequestRecord> for RequestJson {
    fn from(r: &RequestRecord) -> RequestJson {
        let (status, reject_kind) = match &r.status {
            RecordStatus::Active => ("active", None),
            RecordStatus::Completed => ("completed", None),
            RecordStatus::Rejected(kind) => ("rejected", Some(kind.clone())),
            RecordStatus::Failed => ("failed", None),
        };
        // read once: the lookup may settle the slot at any moment, and a
        // second read could show the path of a process whose PID is `null`
        let process = r.process.found();
        RequestJson {
            id: r.id,
            listener: listener_name(r.listener),
            transport: match r.transport {
                Transport::Tcp => "tcp",
                Transport::Udp => "udp",
            },
            src: r.src.to_string(),
            // `0` is no port: a session rurge started itself came in on no
            // listener
            listen_port: (r.listen_port != 0).then_some(r.listen_port),
            pid: process.and_then(|p| p.pid),
            process_path: process.and_then(|p| p.path.clone()),
            dst: r.dst.clone(),
            remote_addr: r.remote_addr.map(|a| a.to_string()),
            rule: r.rule.clone(),
            policy: r.policy.clone(),
            sni: r.sni.clone(),
            protocol: r.protocol.map(protocol_name),
            up: r.up,
            down: r.down,
            started_ms: r.started_ms,
            elapsed_ms: r.elapsed_ms,
            connect_ms: r.connect_ms,
            first_byte_ms: r.first_byte_ms,
            dns_ms: r.dns_ms,
            status,
            reject_kind,
            error: r.error.clone(),
            notes: r.notes.clone(),
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
    let limit = match q.limit {
        Some(0) => return Err(ApiError::bad_request("limit must be at least 1")),
        Some(n) => n,
        None => DEFAULT_LIMIT,
    };
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
    use rurge_config::session::ProcessInfo;
    use rurge_engine::{ProcessSlot, RecordStatus};

    fn record(id: u64, listener: ListenerKind) -> RequestRecord {
        RequestRecord {
            id,
            listener,
            transport: Transport::Tcp,
            src: "127.0.0.1:1".parse().unwrap(),
            listen_port: 6152,
            process: ProcessSlot::unknown(),
            dst: "1.1.1.1:443".into(),
            remote_addr: None,
            rule: None,
            policy: vec![],
            sni: None,
            protocol: None,
            up: 0,
            down: 0,
            started_ms: 0,
            elapsed_ms: 0,
            connect_ms: None,
            first_byte_ms: None,
            dns_ms: None,
            status: RecordStatus::Active,
            error: None,
            notes: vec![],
        }
    }

    /// Every field of a record reaches the JSON under its own name, with its
    /// own value, and the JSON has no other. The values are all different, so
    /// that two fields that changed places show.
    #[test]
    fn every_field_of_a_record_is_sent_under_its_own_name() {
        let record = RequestRecord {
            id: 12,
            listener: ListenerKind::Socks5,
            transport: Transport::Udp,
            src: "127.0.0.1:51234".parse().unwrap(),
            listen_port: 6153,
            process: ProcessSlot::known(ProcessInfo {
                pid: Some(4321),
                name: "app.exe".into(),
                path: Some("C:\\Apps\\app.exe".into()),
            }),
            dst: "example.com:443".into(),
            remote_addr: Some("127.0.0.1:8443".parse().unwrap()),
            rule: Some("DOMAIN-SUFFIX,example.com,Proxy".into()),
            policy: vec!["Proxy".into(), "HK".into()],
            sni: Some("example.com".into()),
            protocol: Some(ProtocolKind::Https),
            up: 1234,
            down: 56789,
            started_ms: 1_757_200_000_000,
            elapsed_ms: 812,
            connect_ms: Some(35),
            first_byte_ms: Some(120),
            dns_ms: Some(14),
            status: RecordStatus::Rejected("REJECT-TINYGIF".into()),
            error: Some("an explanation".into()),
            notes: vec!["a note".into()],
        };
        assert_eq!(
            serde_json::to_value(RequestJson::from(&record)).unwrap(),
            json!({
                "id": 12,
                "listener": "socks5",
                "transport": "udp",
                "src": "127.0.0.1:51234",
                "listenPort": 6153,
                "pid": 4321,
                "processPath": "C:\\Apps\\app.exe",
                "dst": "example.com:443",
                "remoteAddr": "127.0.0.1:8443",
                "rule": "DOMAIN-SUFFIX,example.com,Proxy",
                "policy": ["Proxy", "HK"],
                "sni": "example.com",
                "protocol": "https",
                "up": 1234,
                "down": 56789,
                "startedMs": 1_757_200_000_000_u64,
                "elapsedMs": 812,
                "connectMs": 35,
                "firstByteMs": 120,
                "dnsMs": 14,
                "status": "rejected",
                "rejectKind": "REJECT-TINYGIF",
                "error": "an explanation",
                "notes": ["a note"],
            })
        );
    }

    /// The process is `null` twice over when it is not known — a session
    /// nobody looked up, one whose lookup is still under way — and the path
    /// alone is `null` for a process known by its name only.
    #[test]
    fn a_record_without_a_process_sends_nulls() {
        let mut r = record(1, ListenerKind::Http);
        for slot in [ProcessSlot::unknown(), ProcessSlot::pending()] {
            r.process = slot;
            let v = serde_json::to_value(RequestJson::from(&r)).unwrap();
            assert_eq!(v["pid"], Value::Null);
            assert_eq!(v["processPath"], Value::Null);
        }
        // a lookup that found nothing is no process either
        let found_none = ProcessSlot::pending();
        found_none.settle(None);
        r.process = found_none;
        let v = serde_json::to_value(RequestJson::from(&r)).unwrap();
        assert_eq!((&v["pid"], &v["processPath"]), (&Value::Null, &Value::Null));
        // known by its name only (a `comm` on Linux, `System` on Windows)
        r.process = ProcessSlot::known(ProcessInfo {
            pid: Some(7),
            name: "app".into(),
            path: None,
        });
        let v = serde_json::to_value(RequestJson::from(&r)).unwrap();
        assert_eq!(v["pid"], 7);
        assert_eq!(v["processPath"], Value::Null);
        // found after the record was made: the record shows it when it is read
        let late = ProcessSlot::pending();
        r.process = late.clone();
        let v = serde_json::to_value(RequestJson::from(&r)).unwrap();
        assert_eq!(v["pid"], Value::Null);
        late.settle(Some(ProcessInfo {
            pid: Some(9),
            name: "app".into(),
            path: Some("/usr/bin/app".into()),
        }));
        let v = serde_json::to_value(RequestJson::from(&r)).unwrap();
        assert_eq!(v["pid"], 9);
        assert_eq!(v["processPath"], "/usr/bin/app");
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
        assert_eq!(j.transport, "tcp");
        r.transport = Transport::Udp;
        assert_eq!(RequestJson::from(&r).transport, "udp");
        assert_eq!(j.status, "rejected");
        assert_eq!(j.reject_kind.as_deref(), Some("REJECT-TINYGIF"));
        assert_eq!(j.protocol, Some("mtproto"));
    }

    /// `notes` is always a list, empty when the session has none.
    #[test]
    fn notes_are_a_list() {
        let mut r = record(1, ListenerKind::Http);
        let v = serde_json::to_value(RequestJson::from(&r)).unwrap();
        assert_eq!(v["notes"], json!([]));
        r.notes = vec!["Sub-rule matched: DOMAIN,a.test (in S)".to_string()];
        let v = serde_json::to_value(RequestJson::from(&r)).unwrap();
        assert_eq!(
            v["notes"],
            json!(["Sub-rule matched: DOMAIN,a.test (in S)"])
        );
    }

    /// The two timing fields (M3c design 4.1): `null` until the moment has
    /// come, milliseconds after.
    #[test]
    fn the_timings_are_null_until_known() {
        let mut r = record(1, ListenerKind::Http);
        let v = serde_json::to_value(RequestJson::from(&r)).unwrap();
        assert_eq!(v["connectMs"], Value::Null);
        assert_eq!(v["firstByteMs"], Value::Null);
        r.connect_ms = Some(12);
        r.first_byte_ms = Some(34);
        let v = serde_json::to_value(RequestJson::from(&r)).unwrap();
        assert_eq!(v["connectMs"], 12);
        assert_eq!(v["firstByteMs"], 34);
    }

    /// The listener's port is a number, `null` for a session that came in on
    /// no listener (rurge's own); the address of the session's own connection
    /// and the time spent on lookups are `null` when there is none.
    #[test]
    fn the_listen_port_the_remote_address_and_the_lookup_time() {
        let mut r = record(1, ListenerKind::Http);
        let v = serde_json::to_value(RequestJson::from(&r)).unwrap();
        assert_eq!(v["listenPort"], 6152);
        assert_eq!(v["remoteAddr"], Value::Null);
        assert_eq!(v["dnsMs"], Value::Null);
        let mut own = record(2, ListenerKind::Internal);
        own.listen_port = 0;
        let v = serde_json::to_value(RequestJson::from(&own)).unwrap();
        assert_eq!(v["listenPort"], Value::Null);
        r.remote_addr = Some("192.0.2.7:443".parse().unwrap());
        r.dns_ms = Some(0);
        let v = serde_json::to_value(RequestJson::from(&r)).unwrap();
        assert_eq!(v["remoteAddr"], "192.0.2.7:443");
        assert_eq!(v["dnsMs"], 0);
        r.remote_addr = Some("[2001:db8::1]:443".parse().unwrap());
        let v = serde_json::to_value(RequestJson::from(&r)).unwrap();
        assert_eq!(v["remoteAddr"], "[2001:db8::1]:443");
    }
}
