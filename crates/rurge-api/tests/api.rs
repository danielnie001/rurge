//! Real engine (MockDns + TestServer + loopback listeners) behind the API,
//! driven with a hyper-util client (M4 design §9).

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use rurge_api::{ApiContext, serve};
use rurge_config::config::{LoadOptions, from_text};
use rurge_config::session::ListenerKind;
use rurge_dns::system::StaticSystemDns;
use rurge_dns::testing::MockDns;
use rurge_engine::control::{Control, LogLevel, ReloadReport};
use rurge_engine::stack::StackOptions;
use rurge_engine::state::{STATE_FILE, State, StateStore};
use rurge_engine::{Engine, ListenerSpec, Running, Runtime, RuntimeOptions};
use rurge_net::BoxFuture;
use rurge_net::testing::TestServer;
use rurge_policy::GroupSelections;
use rurge_rules::{GeoUrls, OutboundMode};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

const KEY: &str = "s3cret";

#[derive(Default)]
struct FakeControl {
    reloads: AtomicUsize,
    stops: AtomicUsize,
    levels: Mutex<Vec<LogLevel>>,
}

impl Control for FakeControl {
    fn reload(&self) -> BoxFuture<'_, ReloadReport> {
        Box::pin(async move {
            self.reloads.fetch_add(1, Ordering::SeqCst);
            ReloadReport {
                ok: true,
                errors: 0,
                warnings: 1,
                listeners_rebound: false,
            }
        })
    }
    fn stop(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.stops.fetch_add(1, Ordering::SeqCst);
        })
    }
    fn set_log_level(&self, level: LogLevel) -> Result<(), String> {
        self.levels.lock().unwrap().push(level);
        Ok(())
    }
    fn set_system_proxy(&self, _enabled: bool) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Err("not implemented".to_string()) })
    }
}

struct Api {
    _dir: tempfile::TempDir,
    /// Read by Task 6's `POST /v1/profiles/*` tests; unread until then.
    #[allow(dead_code)]
    conf: PathBuf,
    state_path: PathBuf,
    /// Read directly by Task 6's `/v1/dns*` tests; unread until then.
    #[allow(dead_code)]
    engine: Arc<Engine>,
    listeners: Vec<(ListenerSpec, Running)>,
    target: TestServer,
    /// Read by Task 6's `/v1/dns*` tests; unread until then.
    #[allow(dead_code)]
    dns: MockDns,
    control: Arc<FakeControl>,
    store: Arc<StateStore>,
    addr: SocketAddr,
    _token: CancellationToken,
}

impl Api {
    fn http(&self) -> SocketAddr {
        self.listeners
            .iter()
            .find(|(spec, _)| spec.kind == ListenerKind::Http)
            .map(|(_, r)| r.local_addr)
            .expect("http listener")
    }
    fn target_port(&self) -> u16 {
        self.target.url("/").port().unwrap()
    }
    fn target_url(&self) -> String {
        format!("http://target.test:{}/hello", self.target_port())
    }
}

/// The binary declares no proxy protocols, so drop shadowsocks from the test
/// capabilities to get the load-time W0007 warning the tests look for.
fn load_options() -> LoadOptions {
    let mut opts = LoadOptions::for_tests();
    opts.capabilities
        .policy_kinds
        .remove(&rurge_config::PolicyKind::Shadowsocks);
    opts
}

async fn api() -> Api {
    api_with("").await
}

/// `rules` are inserted before `DOMAIN,ads.test,REJECT` / `FINAL,DIRECT`.
async fn api_with(rules: &str) -> Api {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    dns.set("cached.test", &["10.0.0.1"], &[], 300);
    let target = TestServer::spawn().await;
    target.set("/hello", "hi there");
    let dir = tempfile::tempdir().unwrap();
    let conf = dir.path().join("t.conf");
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\ndns-server = {}\nipv6 = false\n\
internet-test-url = http://target.test:{}/hello\n\
[Proxy]\nHK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\nBlock = reject-tinygif\n\
[Proxy Group]\nPick = select, HK, DIRECT\n\
[Rule]\n{rules}\nDOMAIN,ads.test,REJECT\nFINAL,DIRECT\n",
        dns.addr(),
        target.url("/").port().unwrap()
    );
    std::fs::write(&conf, &profile).unwrap();
    let loaded = from_text(&profile, &conf, &load_options());
    assert!(!loaded.diagnostics.has_errors());
    let runtime = Runtime::build(
        loaded.config,
        RuntimeOptions {
            stack: StackOptions {
                data_dir: dir.path().to_path_buf(),
                no_network: true,
                geo_urls: GeoUrls::default(),
                dns_cache_size: 2000,
                system: Arc::new(StaticSystemDns::default()),
                wait: Duration::ZERO,
                dns_connector: None,
            },
            outbound_mode: OutboundMode::Rule,
            idle_timeout: Duration::from_secs(600),
            selections: GroupSelections::new(),
            request_log_size: 1000,
        },
    )
    .await
    .unwrap();
    let engine = Engine::new(runtime);
    let state_path = dir.path().join(STATE_FILE);
    let (store, _) = StateStore::open(state_path.clone()).await;
    engine.attach_state(store.clone());
    engine.start_sampler();
    let listeners = engine.bind_listeners().await.unwrap();
    let control = Arc::new(FakeControl::default());
    let token = CancellationToken::new();
    let (addr, fut) = serve(
        "127.0.0.1:0".parse().unwrap(),
        KEY.to_string(),
        ApiContext {
            engine: engine.clone(),
            control: control.clone(),
            load_options: load_options(),
        },
        token.clone(),
    )
    .await
    .unwrap();
    tokio::spawn(fut);
    Api {
        _dir: dir,
        conf,
        state_path,
        engine,
        listeners,
        target,
        dns,
        control,
        store,
        addr,
        _token: token,
    }
}

async fn call_raw(
    addr: SocketAddr,
    method: &str,
    path: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> (u16, String, Bytes) {
    let client = Client::builder(TokioExecutor::new()).build_http::<Full<Bytes>>();
    let mut req = Request::builder()
        .method(method)
        .uri(format!("http://{addr}{path}"));
    if let Some(k) = key {
        req = req.header("x-key", k);
    }
    let body = match body {
        Some(v) => {
            req = req.header("content-type", "application/json");
            Full::new(Bytes::from(v.to_string()))
        }
        None => Full::new(Bytes::new()),
    };
    let resp = client.request(req.body(body).unwrap()).await.unwrap();
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, content_type, bytes)
}

async fn call(
    addr: SocketAddr,
    method: &str,
    path: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> (u16, Value) {
    let (status, _, bytes) = call_raw(addr, method, path, key, body).await;
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, value)
}

async fn get(api: &Api, path: &str) -> (u16, Value) {
    call(api.addr, "GET", path, Some(KEY), None).await
}

async fn post(api: &Api, path: &str, body: Value) -> (u16, Value) {
    call(api.addr, "POST", path, Some(KEY), Some(body)).await
}

/// Plain HTTP GET through the proxy listener; returns (status line, body).
async fn get_via_proxy(proxy: SocketAddr, url: &str) -> (String, Vec<u8>) {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    let host = url
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap()
        .to_string();
    s.write_all(
        format!("GET {url} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut buf)).await;
    let text = String::from_utf8_lossy(&buf).into_owned();
    let (head, body) = match text.find("\r\n\r\n") {
        Some(pos) => (text[..pos].to_string(), buf[pos + 4..].to_vec()),
        None => (text, Vec::new()),
    };
    (head.lines().next().unwrap_or("").to_string(), body)
}

/// Polls `f` every 50 ms for up to 5 s.
async fn wait_until(mut f: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !f().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition not met in time"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn rejects_missing_or_wrong_key_and_bans_after_five_failures() {
    let api = api().await;
    let (status, body) = call(api.addr, "GET", "/v1/outbound", None, None).await;
    assert_eq!((status, body), (401, json!({ "error": "unauthorized" })));
    // the query form works too
    let (status, body) = call(
        api.addr,
        "GET",
        &format!("/v1/outbound?x-key={KEY}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["mode"], "rule");
    // the no-key attempt above was failure #1; three more make four
    for _ in 0..3 {
        let (status, _) = call(api.addr, "GET", "/v1/outbound", Some("wrong"), None).await;
        assert_eq!(status, 401);
    }
    let (status, _) = call(api.addr, "GET", "/v1/outbound", Some("wrong"), None).await;
    assert_eq!(
        status, 401,
        "the fifth failure still answers 401 (and starts the ban)"
    );
    let (status, body) = get(&api, "/v1/outbound").await;
    assert_eq!(
        (status, body),
        (403, json!({ "error": "banned" })),
        "even the right key is banned now"
    );
}

#[tokio::test]
async fn outbound_mode_and_global_policy_round_trip_and_persist() {
    let api = api().await;
    let (status, body) = get(&api, "/v1/outbound").await;
    assert_eq!((status, body), (200, json!({ "mode": "rule" })));
    let (status, body) = post(&api, "/v1/outbound", json!({ "mode": "proxy" })).await;
    assert_eq!(status, 400, "{body}");
    assert!(body["error"].as_str().unwrap().contains("global policy"));
    let (status, body) = post(&api, "/v1/outbound/global", json!({ "policy": "nope" })).await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = post(&api, "/v1/outbound/global", json!({ "policy": "Pick" })).await;
    assert_eq!((status, body), (200, json!({})));
    let (status, body) = post(&api, "/v1/outbound", json!({ "mode": "proxy" })).await;
    assert_eq!((status, body), (200, json!({})));
    assert_eq!(
        get(&api, "/v1/outbound").await.1,
        json!({ "mode": "proxy" })
    );
    assert_eq!(
        get(&api, "/v1/outbound/global").await.1,
        json!({ "policy": "Pick" })
    );
    let on_disk: State =
        serde_json::from_str(&std::fs::read_to_string(&api.state_path).unwrap()).unwrap();
    assert_eq!(on_disk.outbound_mode.as_deref(), Some("proxy"));
    assert_eq!(on_disk.global_policy.as_deref(), Some("Pick"));
    assert_eq!(api.store.snapshot().await, on_disk);
    // bad inputs
    let (status, body) = post(&api, "/v1/outbound", json!({ "mode": "loud" })).await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = post(&api, "/v1/outbound", json!({ "nope": 1 })).await;
    assert_eq!(status, 400, "{body}");
    assert!(body["error"].is_string());
    let (status, _, bytes) = call_raw(api.addr, "POST", "/v1/outbound", Some(KEY), None).await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
}

#[tokio::test]
async fn features_collections_stop_and_unknown_paths() {
    let api = api().await;
    assert_eq!(
        get(&api, "/v1/features/mitm").await,
        (200, json!({ "enabled": false }))
    );
    let (status, _) = post(&api, "/v1/features/mitm", json!({ "enabled": true })).await;
    assert_eq!(status, 501);
    let (status, _) = post(
        &api,
        "/v1/features/system_proxy",
        json!({ "enabled": true }),
    )
    .await;
    assert_eq!(status, 501, "M4a: not implemented");
    assert_eq!(get(&api, "/v1/features/teleport").await.0, 404);
    assert_eq!(
        get(&api, "/v1/modules").await.1,
        json!({ "enabled": [], "available": [] })
    );
    assert_eq!(get(&api, "/v1/scripting").await.1, json!({ "scripts": [] }));
    assert_eq!(get(&api, "/v1/events").await.1, json!({ "events": [] }));
    let (status, body) = get(&api, "/v1/nope").await;
    assert_eq!(
        (status, body),
        (404, json!({ "error": "no such endpoint" }))
    );
    assert_eq!(post(&api, "/v1/stop", json!({})).await, (200, json!({})));
    wait_until(async || api.control.stops.load(Ordering::SeqCst) == 1).await;
}

#[tokio::test]
async fn policies_and_rules_are_listed_with_hits() {
    let api = api().await;
    let (status, body) = get(&api, "/v1/policies").await;
    assert_eq!(status, 200);
    let proxies: Vec<&str> = body["proxies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        proxies.contains(&"DIRECT")
            && proxies.contains(&"REJECT-TINYGIF")
            && proxies.contains(&"HK")
            && proxies.contains(&"Block"),
        "{proxies:?}"
    );
    assert_eq!(body["policy-groups"], json!(["Pick"]));
    let (head, _) = get_via_proxy(api.http(), &api.target_url()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let (_, body) = get(&api, "/v1/rules").await;
    let rules = body["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0]["index"], 0);
    assert_eq!(rules[0]["rule"], "DOMAIN,ads.test,REJECT");
    assert_eq!(rules[0]["hits"], 0);
    assert_eq!(rules[1]["rule"], "FINAL,DIRECT");
    assert!(rules[1]["hits"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn recent_and_active_requests_and_kill() {
    let api = api().await;
    let (head, _) = get_via_proxy(api.http(), &api.target_url()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let mut recent = Vec::new();
    wait_until(async || {
        recent = get(&api, "/v1/requests/recent").await.1["requests"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        recent.iter().any(|r| r["status"] == "completed")
    })
    .await;
    let r = recent.iter().find(|r| r["status"] == "completed").unwrap();
    assert_eq!(r["listener"], "http");
    assert_eq!(r["dst"], format!("target.test:{}", api.target_port()));
    assert_eq!(r["policy"], json!(["DIRECT"]));
    assert_eq!(r["rule"], "FINAL,DIRECT");
    assert!(r["up"].as_u64().unwrap() > 0 && r["down"].as_u64().unwrap() > 0);
    assert!(r["startedMs"].as_u64().unwrap() > 0);
    assert!(r["rejectKind"].is_null() && r["error"].is_null());
    assert_eq!(
        get(&api, "/v1/requests/recent?limit=1").await.1["requests"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let (status, body) = post(&api, "/v1/requests/kill", json!({ "id": 999_999 })).await;
    assert_eq!(status, 404, "{body}");
    // a CONNECT tunnel stays active until killed
    let mut tunnel = TcpStream::connect(api.http()).await.unwrap();
    let dst = format!("target.test:{}", api.target_port());
    tunnel
        .write_all(format!("CONNECT {dst} HTTP/1.1\r\nHost: {dst}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut buf = [0u8; 256];
    let n = tunnel.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
    let mut active_id = 0;
    wait_until(async || {
        let body = get(&api, "/v1/requests/active").await.1;
        let found = body["requests"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["dst"] == dst && r["status"] == "active")
            .cloned();
        if let Some(r) = found {
            active_id = r["id"].as_u64().unwrap();
        }
        active_id != 0
    })
    .await;
    assert_eq!(
        post(&api, "/v1/requests/kill", json!({ "id": active_id })).await,
        (200, json!({}))
    );
    let closed = tokio::time::timeout(Duration::from_secs(3), tunnel.read(&mut buf)).await;
    assert!(
        matches!(closed, Ok(Ok(0)) | Ok(Err(_))),
        "killed tunnel closes: {closed:?}"
    );
    wait_until(async || {
        get(&api, "/v1/requests/active").await.1["requests"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["id"] != active_id)
    })
    .await;
}

#[tokio::test]
async fn traffic_reports_totals_by_policy_and_listener() {
    let api = api().await;
    let (head, _) = get_via_proxy(api.http(), &api.target_url()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let mut body = Value::Null;
    wait_until(async || {
        body = get(&api, "/v1/traffic").await.1;
        body["total"]["in"].as_u64().unwrap_or(0) > 0
    })
    .await;
    assert!(body["startTime"].as_f64().unwrap() > 1.0e9);
    assert!(body["total"]["out"].as_u64().unwrap() > 0);
    assert!(body["total"]["inCurrentSpeed"].is_u64() && body["total"]["outCurrentSpeed"].is_u64());
    assert!(body["connector"]["DIRECT"]["in"].as_u64().unwrap() > 0);
    assert!(body["listener"]["http"]["out"].as_u64().unwrap() > 0);
    assert_eq!(body["listener"]["socks5"]["in"], 0);
}
