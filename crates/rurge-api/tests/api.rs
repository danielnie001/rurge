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
use rurge_config::{RemoteRef, RemoteRole, SourceInfo};
use rurge_dns::system::StaticSystemDns;
use rurge_dns::testing::MockDns;
use rurge_engine::control::{Control, LogLevel, ReloadReport};
use rurge_engine::profile::{ProfileSources, SourcesOptions};
use rurge_engine::stack::{StackOptions, resource_client};
use rurge_engine::state::{STATE_FILE, State, StateStore};
use rurge_engine::{Engine, EngineShared, ListenerSpec, Running, Runtime, RuntimeOptions};
use rurge_net::BoxFuture;
use rurge_net::resource::{ResourceOptions, ResourceSource, cache_file, resource_key};
use rurge_net::testing::{TestServer, seed_cache};
use rurge_rules::geoip::{ASN_FILE, COUNTRY_FILE};
use rurge_rules::{DbKind, GeoDb, GeoUrls, OutboundMode};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

const KEY: &str = "s3cret";

#[derive(Default)]
struct FakeControl {
    reloads: AtomicUsize,
    stops: AtomicUsize,
    levels: Mutex<Vec<LogLevel>>,
    system_proxy: AtomicBool,
    fail_system_proxy: AtomicBool,
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
    fn set_system_proxy(&self, enabled: bool) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            if self.fail_system_proxy.load(Ordering::SeqCst) {
                return Err("no supported desktop proxy settings were found".to_string());
            }
            self.system_proxy.store(enabled, Ordering::SeqCst);
            Ok(())
        })
    }
    fn system_proxy_enabled(&self) -> bool {
        self.system_proxy.load(Ordering::SeqCst)
    }
}

struct Api {
    dir: tempfile::TempDir,
    conf: PathBuf,
    state_path: PathBuf,
    engine: Arc<Engine>,
    listeners: Vec<(ListenerSpec, Running)>,
    target: TestServer,
    dns: MockDns,
    control: Arc<FakeControl>,
    store: Arc<StateStore>,
    addr: SocketAddr,
    token: CancellationToken,
}

impl Api {
    /// A second API server over the same engine, which has the downloaded
    /// sources of the profile that the first one never has. Where it listens.
    async fn with_sources(&self, sources: Arc<ProfileSources>) -> SocketAddr {
        let (addr, server) = serve(
            "127.0.0.1:0".parse().unwrap(),
            KEY.to_string(),
            ApiContext {
                engine: self.engine.clone(),
                control: self.control.clone(),
                load_options: load_options(),
                sources: Some(sources),
            },
            self.token.clone(),
        )
        .await
        .unwrap();
        tokio::spawn(server);
        addr
    }

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
    fn big_url(&self) -> String {
        format!("http://target.test:{}/big", self.target_port())
    }
}

/// The binary does not declare Tailscale, so dropping it from the test
/// capabilities yields the load-time W0007 warning the tests look for.
fn load_options() -> LoadOptions {
    let mut opts = LoadOptions::for_tests();
    opts.capabilities
        .policy_kinds
        .remove(&rurge_config::PolicyKind::Tailscale);
    opts
}

async fn api() -> Api {
    api_with("").await
}

/// `rules` are inserted before `DOMAIN,ads.test,REJECT` / `FINAL,DIRECT`.
async fn api_with(rules: &str) -> Api {
    api_in(rules, "").await
}

/// `groups` go after `Pick`. A proxy would be tested at a loopback port,
/// never at the default `http://bing.com/`; no test here has one tested,
/// and none may count on what that port answers.
async fn api_in(rules: &str, groups: &str) -> Api {
    api_seeded(rules, groups, |_| {}).await
}

/// `api_in`, `seed` having filled the data directory before the engine starts.
async fn api_seeded(rules: &str, groups: &str, seed: impl FnOnce(&Path)) -> Api {
    api_built(rules, groups, None, seed).await
}

/// `api_seeded` with a generation that downloads, its two GeoIP databases
/// asked of `geo`. By the time it is built every first download has been
/// tried, and those `geo` cannot give have failed.
async fn api_online(rules: &str, groups: &str, geo: &TestServer, seed: impl FnOnce(&Path)) -> Api {
    api_built(rules, groups, Some(geo), seed).await
}

/// `geo` is the server the generation downloads its GeoIP databases from; it
/// does not go to the network without one.
async fn api_built(
    rules: &str,
    groups: &str,
    geo: Option<&TestServer>,
    seed: impl FnOnce(&Path),
) -> Api {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    dns.set("cached.test", &["10.0.0.1"], &[], 300);
    let target = TestServer::spawn().await;
    target.set("/hello", "hi there");
    target.set("/big", "x".repeat(4096));
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let conf = dir.path().join("t.conf");
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\ndns-server = {}\nipv6 = false\n\
internet-test-url = http://target.test:{}/hello\nproxy-test-url = http://127.0.0.1:9/\n\
[Proxy]\nHK = tailscale, section-name=ts\nBlock = reject-tinygif\n\
Locked = trojan, 1.2.3.4, 443, password=x\n\
[Proxy Group]\nPick = select, HK, DIRECT\n{groups}\n\
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
                no_network: geo.is_none(),
                geo_urls: geo.map_or_else(GeoUrls::default, |server| GeoUrls {
                    country: server.url("/geo/country.mmdb"),
                    asn: server.url("/geo/asn.mmdb"),
                }),
                dns_cache_size: 2000,
                system: Arc::new(StaticSystemDns::default()),
                wait: if geo.is_some() {
                    Duration::from_secs(5)
                } else {
                    Duration::ZERO
                },
                dns_connector: None,
                socket_hook: Arc::new(rurge_net::socket::NoopSocketHook),
            },
            outbound_mode: OutboundMode::Rule,
            idle_timeout: Duration::from_secs(600),
            shared: EngineShared::default(),
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
            sources: None,
        },
        token.clone(),
    )
    .await
    .unwrap();
    tokio::spawn(fut);
    Api {
        dir,
        conf,
        state_path,
        engine,
        listeners,
        target,
        dns,
        control,
        store,
        addr,
        token,
    }
}

async fn call_raw(
    addr: SocketAddr,
    method: &str,
    path: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> (u16, String, Bytes) {
    call_text(addr, method, path, key, body.map(|v| v.to_string())).await
}

/// `call_raw` with the body as the text it is sent as: for a body that is no
/// JSON.
async fn call_text(
    addr: SocketAddr,
    method: &str,
    path: &str,
    key: Option<&str>,
    body: Option<String>,
) -> (u16, String, Bytes) {
    let client = Client::builder(TokioExecutor::new()).build_http::<Full<Bytes>>();
    let mut req = Request::builder()
        .method(method)
        .uri(format!("http://{addr}{path}"));
    if let Some(k) = key {
        req = req.header("x-key", k);
    }
    let body = match body {
        Some(text) => {
            req = req.header("content-type", "application/json");
            Full::new(Bytes::from(text))
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
async fn the_global_policy_cannot_be_cleared_while_in_proxy_mode() {
    let api = api().await;
    assert_eq!(
        get(&api, "/v1/outbound/global").await,
        (200, json!({ "policy": null }))
    );
    assert_eq!(
        post(&api, "/v1/outbound/global", json!({ "policy": "Pick" }))
            .await
            .0,
        200
    );
    assert_eq!(
        post(&api, "/v1/outbound", json!({ "mode": "proxy" }))
            .await
            .0,
        200
    );
    let (status, body) = post(&api, "/v1/outbound/global", json!({ "policy": " " })).await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("while the outbound mode is proxy"),
        "{body}"
    );
    assert_eq!(
        get(&api, "/v1/outbound/global").await.1,
        json!({ "policy": "Pick" }),
        "unchanged"
    );
    // replacing it is fine, and so is clearing it once the mode moved on
    assert_eq!(
        post(&api, "/v1/outbound/global", json!({ "policy": "DIRECT" }))
            .await
            .0,
        200
    );
    assert_eq!(
        post(&api, "/v1/outbound", json!({ "mode": "rule" }))
            .await
            .0,
        200
    );
    assert_eq!(
        post(&api, "/v1/outbound/global", json!({ "policy": "" })).await,
        (200, json!({}))
    );
    assert_eq!(
        get(&api, "/v1/outbound/global").await.1,
        json!({ "policy": null })
    );
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
    assert_eq!(
        get(&api, "/v1/features/system_proxy").await,
        (200, json!({ "enabled": false }))
    );
    assert_eq!(
        post(
            &api,
            "/v1/features/system_proxy",
            json!({ "enabled": true })
        )
        .await,
        (200, json!({}))
    );
    assert_eq!(
        get(&api, "/v1/features/system_proxy").await,
        (200, json!({ "enabled": true }))
    );
    assert_eq!(
        get(&api, "/v1/features/mitm").await,
        (200, json!({ "enabled": false })),
        "only system_proxy is live"
    );
    assert_eq!(
        post(
            &api,
            "/v1/features/system_proxy",
            json!({ "enabled": false })
        )
        .await,
        (200, json!({}))
    );
    assert!(!api.control.system_proxy.load(Ordering::SeqCst));
    api.control.fail_system_proxy.store(true, Ordering::SeqCst);
    let (status, body) = post(
        &api,
        "/v1/features/system_proxy",
        json!({ "enabled": true }),
    )
    .await;
    assert_eq!(status, 500, "{body}");
    assert_eq!(
        body["error"],
        "no supported desktop proxy settings were found"
    );
    let (status, _) = post(
        &api,
        "/v1/features/system_proxy",
        json!({ "enabled": "yes" }),
    )
    .await;
    assert_eq!(status, 400);
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
    // a wrong method on a registered path is a JSON error too, not axum's
    // default empty-bodied 405
    let (status, body) = call(api.addr, "DELETE", "/v1/outbound", Some(KEY), None).await;
    assert_eq!(
        (status, body),
        (405, json!({ "error": "method not allowed" }))
    );
    // the auth layer wraps the fallback: an unknown path without a key is 401,
    // not 404 (no probing the endpoint list without the key)
    let (status, body) = call(api.addr, "GET", "/v1/nope", None, None).await;
    assert_eq!((status, body), (401, json!({ "error": "unauthorized" })));
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

/// Matching ends at the first `FINAL` and the last one gives the policy, so
/// the rules after the first are not in the list and the list ends with the
/// `FINAL` that gives the policy, under its position in the profile.
#[tokio::test]
async fn the_rules_after_the_first_final_are_not_listed() {
    let api = api_with("DOMAIN,before.test,DIRECT\nFINAL,Pick\nDOMAIN,between.test,REJECT").await;
    let (status, body) = get(&api, "/v1/rules").await;
    assert_eq!(status, 200);
    let rules = body["rules"].as_array().unwrap();
    let listed: Vec<(u64, &str)> = rules
        .iter()
        .map(|r| (r["index"].as_u64().unwrap(), r["rule"].as_str().unwrap()))
        .collect();
    // the profile's rules are: 0 before.test, 1 FINAL,Pick, 2 between.test,
    // 3 ads.test, 4 FINAL,DIRECT
    assert_eq!(
        listed,
        [(0, "DOMAIN,before.test,DIRECT"), (4, "FINAL,DIRECT")]
    );
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
    // a second, distinguishable request: ads.test is REJECTed by a domain
    // rule (no DNS entry needed, since domain rules never resolve)
    let _ = get_via_proxy(api.http(), "http://ads.test/").await;
    wait_until(async || {
        recent = get(&api, "/v1/requests/recent").await.1["requests"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        recent
            .iter()
            .any(|r| r["status"] == "rejected" && r["rejectKind"] == "REJECT")
    })
    .await;
    assert!(recent.len() >= 2, "{recent:?}");
    assert_eq!(
        get(&api, "/v1/requests/recent?limit=1").await.1["requests"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let (status, body) = get(&api, "/v1/requests/recent?limit=abc").await;
    assert_eq!(status, 400, "{body}");
    assert!(body["error"].is_string());
    // `limit=0` is a caller mistake, not "give me one record"
    assert_eq!(
        get(&api, "/v1/requests/recent?limit=0").await,
        (400, json!({ "error": "limit must be at least 1" }))
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

/// `notes` says which line of a set decided a request; a request no set had
/// a say in has an empty list.
#[tokio::test]
async fn requests_carry_the_notes_of_their_sessions() {
    let sets = tempfile::tempdir().unwrap();
    let list = sets.path().join("listed.list");
    std::fs::write(&list, "DOMAIN,target.test\n").unwrap();
    let api = api_with(&format!("RULE-SET,{},DIRECT", list.display())).await;
    let (head, _) = get_via_proxy(api.http(), &api.target_url()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let _ = get_via_proxy(api.http(), "http://ads.test/").await;
    let mut recent = Vec::new();
    wait_until(async || {
        recent = get(&api, "/v1/requests/recent").await.1["requests"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        recent.iter().any(|r| r["status"] == "completed")
            && recent.iter().any(|r| r["status"] == "rejected")
    })
    .await;
    let listed = recent.iter().find(|r| r["status"] == "completed").unwrap();
    assert_eq!(
        listed["notes"],
        json!([format!(
            "Sub-rule matched: DOMAIN,target.test (in {})",
            list.display()
        )])
    );
    assert!(listed["error"].is_null());
    let rejected = recent.iter().find(|r| r["status"] == "rejected").unwrap();
    assert_eq!(rejected["notes"], json!([]));
}

/// A request says which port of the proxy it came in on, where the socket it
/// opened itself went and how long it waited for name lookups — in flight and
/// finished; `null` when there is none of the last two.
#[tokio::test]
async fn requests_carry_the_listen_port_the_remote_address_and_the_lookup_time() {
    let api = api().await;
    let port = api.http().port();
    let target = format!("127.0.0.1:{}", api.target_port());
    let (head, _) = get_via_proxy(api.http(), &api.target_url()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let _ = get_via_proxy(api.http(), "http://ads.test/").await;
    let mut recent = Vec::new();
    wait_until(async || {
        recent = get(&api, "/v1/requests/recent").await.1["requests"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        recent.iter().any(|r| r["status"] == "completed")
            && recent.iter().any(|r| r["status"] == "rejected")
    })
    .await;
    // target.test is looked up (the mock DNS answers it), then connected to
    let served = recent.iter().find(|r| r["status"] == "completed").unwrap();
    assert_eq!(served["listenPort"], port);
    assert_eq!(served["remoteAddr"], target);
    assert!(served["dnsMs"].is_u64(), "{served}");
    // rejected by a domain rule: no lookup, no socket
    let rejected = recent.iter().find(|r| r["status"] == "rejected").unwrap();
    assert_eq!(rejected["listenPort"], port);
    assert!(
        rejected["remoteAddr"].is_null() && rejected["dnsMs"].is_null(),
        "{rejected}"
    );
    // a tunnel still open shows the same while it runs
    let dst = format!("target.test:{}", api.target_port());
    let mut tunnel = TcpStream::connect(api.http()).await.unwrap();
    tunnel
        .write_all(format!("CONNECT {dst} HTTP/1.1\r\nHost: {dst}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut buf = [0u8; 256];
    let n = tunnel.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
    let mut live = Value::Null;
    wait_until(async || {
        let body = get(&api, "/v1/requests/active").await.1;
        live = body["requests"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["dst"] == dst && r["status"] == "active")
            .cloned()
            .unwrap_or(Value::Null);
        !live.is_null()
    })
    .await;
    assert_eq!(live["listenPort"], port);
    assert_eq!(live["remoteAddr"], target);
    assert!(live["dnsMs"].is_u64(), "{live}");
}

#[tokio::test]
async fn traffic_reports_totals_by_policy_and_listener() {
    let api = api().await;
    let (head, resp_body) = get_via_proxy(api.http(), &api.big_url()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(resp_body.len(), 4096);
    let mut body = Value::Null;
    wait_until(async || {
        body = get(&api, "/v1/traffic").await.1;
        body["total"]["in"].as_u64().unwrap_or(0) >= 4096
    })
    .await;
    assert!(body["startTime"].as_f64().unwrap() > 1.0e9);
    assert!(body["total"]["inCurrentSpeed"].is_u64() && body["total"]["outCurrentSpeed"].is_u64());
    // `in` (down) must be at least the 4 KiB body, and strictly more than
    // `out` (up, a bare GET request line + headers) — this would fail if the
    // in/out mapping were ever swapped.
    let total_in = body["total"]["in"].as_u64().unwrap();
    let total_out = body["total"]["out"].as_u64().unwrap();
    assert!(total_in >= 4096 && total_in > total_out, "{body}");
    let direct_in = body["connector"]["DIRECT"]["in"].as_u64().unwrap();
    let direct_out = body["connector"]["DIRECT"]["out"].as_u64().unwrap();
    assert!(direct_in >= 4096 && direct_in > direct_out, "{body}");
    let http_in = body["listener"]["http"]["in"].as_u64().unwrap();
    let http_out = body["listener"]["http"]["out"].as_u64().unwrap();
    assert!(http_in >= 4096 && http_in > http_out, "{body}");
    assert_eq!(body["listener"]["socks5"]["in"], 0);
}

/// `total` must include the bytes of sessions that are still running: the
/// speeds already come from `snapshot_bytes` (finished + in-flight), so a
/// `total` read from the finished-only counters shows `in 0 B (in 5 MiB/s)`
/// during a long transfer.
#[tokio::test]
async fn traffic_total_includes_in_flight_bytes() {
    let api = api().await;
    let host = format!("target.test:{}", api.target_port());
    let mut s = TcpStream::connect(api.http()).await.unwrap();
    s.write_all(format!("CONNECT {host} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut buf = vec![0u8; 1024];
    let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf))
        .await
        .expect("connect reply in time")
        .unwrap();
    let head = String::from_utf8_lossy(&buf[..n]).into_owned();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    // A request that never ends in `\r\n\r\n`: the target never answers and the
    // tunnel stays open, so these bytes are only ever in-flight bytes.
    let payload = format!(
        "GET /hello HTTP/1.1\r\nHost: {host}\r\nX-Pad: {}\r\n",
        "p".repeat(400)
    );
    s.write_all(payload.as_bytes()).await.unwrap();
    s.flush().await.unwrap();
    let want = payload.len() as u64;
    let mut body = Value::Null;
    wait_until(async || {
        body = get(&api, "/v1/traffic").await.1;
        body["total"]["out"].as_u64().unwrap_or(0) >= want
    })
    .await;
    let active = get(&api, "/v1/requests/active").await.1;
    assert!(
        active["requests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["dst"] == json!(host)),
        "the tunnel must still be in flight: {active}"
    );
    drop(s);
}

#[tokio::test]
async fn dns_cache_flush_and_delay() {
    let api = api().await;
    let resolver = api.engine.resolver();
    resolver
        .lookup("cached.test", rurge_dns::LookupOpts::default())
        .await
        .unwrap();
    let (status, body) = get(&api, "/v1/dns").await;
    assert_eq!(status, 200);
    let entry = body["dnsCache"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["domain"] == "cached.test")
        .cloned()
        .expect("cached entry");
    assert_eq!(entry["data"], json!(["10.0.0.1"]));
    assert!(entry["expiresTime"].as_f64().unwrap() > 1.0e9);
    assert!(
        entry["server"]
            .as_str()
            .unwrap()
            .contains(&api.dns.addr().to_string()),
        "{entry}"
    );
    assert_eq!(entry["stale"], false);
    let upstreams = body["upstreams"].as_array().unwrap();
    assert!(
        upstreams
            .iter()
            .any(|u| u.as_str().unwrap().contains(&api.dns.addr().to_string())),
        "{upstreams:?}"
    );
    assert!(body["bootstrap"].is_array());
    assert_eq!(
        post(&api, "/v1/dns/flush", json!({})).await,
        (200, json!({}))
    );
    assert!(
        get(&api, "/v1/dns").await.1["dnsCache"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let (status, body) = post(&api, "/v1/test/dns_delay", json!({ "name": "target.test" })).await;
    assert_eq!(status, 200, "{body}");
    let delays = body["delays"].as_array().unwrap();
    assert_eq!(delays.len(), 1, "{delays:?}");
    assert!(
        delays[0]["upstream"]
            .as_str()
            .unwrap()
            .contains(&api.dns.addr().to_string())
    );
    assert!(
        delays[0]["ms"].is_number() && delays[0]["error"].is_null(),
        "{delays:?}"
    );
    // no name → the host of internet-test-url (target.test)
    let (status, body) = post(&api, "/v1/test/dns_delay", json!({})).await;
    assert_eq!(status, 200, "{body}");
    assert!(body["delays"][0]["ms"].is_number());
    assert!(
        api.dns
            .query_count("target.test", rurge_dns::message::Qtype::A)
            >= 2
    );
}

#[tokio::test]
async fn profiles_current_check_and_reload() {
    let api = api().await;
    let (status, content_type, bytes) =
        call_raw(api.addr, "GET", "/v1/profiles/current", Some(KEY), None).await;
    assert_eq!(status, 200);
    assert!(content_type.starts_with("text/plain"), "{content_type}");
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        text.contains("password=***") && !text.contains("password=x"),
        "{text}"
    );
    let (_, _, bytes) = call_raw(
        api.addr,
        "GET",
        "/v1/profiles/current?sensitive=1",
        Some(KEY),
        None,
    )
    .await;
    assert!(String::from_utf8_lossy(&bytes).contains("password=x"));
    let (status, body) = post(&api, "/v1/profiles/check", json!({})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["errors"], 0);
    assert!(
        body["warnings"].as_u64().unwrap() >= 1,
        "W0007 for the unsupported tailscale policy: {body}"
    );
    assert!(
        body["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "W0007"),
        "{body}"
    );
    // break the file on disk: check reports it, the running config is untouched
    let broken = std::fs::read_to_string(&api.conf)
        .unwrap()
        .replace("FINAL,DIRECT", "FINAL,NoSuchPolicy");
    std::fs::write(&api.conf, broken).unwrap();
    let (status, body) = post(&api, "/v1/profiles/check", json!({})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], false);
    assert!(body["errors"].as_u64().unwrap() >= 1);
    let unknown_policy = body["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == "E0007")
        .unwrap_or_else(|| panic!("{body}"));
    // what to do about it travels with the diagnostic
    assert_eq!(
        unknown_policy["hint"],
        rurge_config::codes::hint("E0007").expect("E0007 has a hint")
    );
    let (head, _) = get_via_proxy(api.http(), &api.target_url()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "still serving: {head}");
    let (status, body) = post(&api, "/v1/profiles/reload", json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(
        body,
        json!({ "ok": true, "errors": 0, "warnings": 1, "listenersRebound": false })
    );
    assert_eq!(api.control.reloads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn log_level_is_forwarded_to_control() {
    let api = api().await;
    assert_eq!(
        post(&api, "/v1/log/level", json!({ "level": "debug" })).await,
        (200, json!({}))
    );
    assert_eq!(*api.control.levels.lock().unwrap(), vec![LogLevel::Debug]);
    let (status, body) = post(&api, "/v1/log/level", json!({ "level": "loud" })).await;
    assert_eq!(status, 400, "{body}");
}

#[tokio::test]
async fn policy_groups_are_listed_and_a_policy_detail_is_redacted() {
    let api = api().await;
    let (status, body) = get(&api, "/v1/policy_groups").await;
    assert_eq!(status, 200);
    let members = body["Pick"].as_array().expect("the group's members");
    assert_eq!(members.len(), 2);
    assert_eq!(members[0]["name"], "HK");
    assert_eq!(members[0]["typeDescription"], "tailscale");
    assert_eq!(members[0]["isGroup"], false);
    assert_eq!(members[0]["enabled"], true);
    let hash = members[0]["lineHash"].as_str().unwrap();
    assert!(
        hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
        "{hash}"
    );
    assert_eq!(members[1]["name"], "DIRECT");
    assert_eq!(members[1]["typeDescription"], "DIRECT");

    let (status, body) = get(&api, "/v1/policies/detail?policy_name=HK").await;
    assert_eq!(
        (status, body),
        (200, json!({ "HK": "tailscale, section-name=ts" }))
    );
    let (status, body) = get(&api, "/v1/policies/detail?policy_name=Locked").await;
    assert_eq!(status, 200);
    let detail = body["Locked"].as_str().expect("the definition");
    assert!(detail.starts_with("trojan, 1.2.3.4, 443"), "{detail}");
    assert!(
        detail.contains("***") && !detail.contains("password=x"),
        "{detail}"
    );
    let (status, body) = get(&api, "/v1/policies/detail?policy_name=DIRECT").await;
    assert_eq!((status, body), (200, json!({ "DIRECT": "DIRECT" })));
    let (status, body) = get(&api, "/v1/policies/detail?policy_name=Nope").await;
    assert_eq!(
        (status, body),
        (404, json!({ "error": "unknown policy `Nope`" }))
    );
    let (status, _) = get(&api, "/v1/policies/detail").await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn a_select_group_is_switched_for_the_next_request_and_persisted() {
    let api = api_with("DOMAIN,target.test,Pick").await;
    assert_eq!(
        get(&api, "/v1/policy_groups/select?group_name=Pick").await,
        (200, json!({ "policy": "HK" }))
    );
    // HK is a protocol rurge does not speak: the request is refused
    let (head, _) = get_via_proxy(api.http(), &api.target_url()).await;
    assert!(!head.starts_with("HTTP/1.1 200"), "{head}");

    let (status, body) = post(
        &api,
        "/v1/policy_groups/select",
        json!({ "group_name": "Pick", "policy": "DIRECT" }),
    )
    .await;
    assert_eq!((status, body), (200, json!({})));
    assert_eq!(
        get(&api, "/v1/policy_groups/select?group_name=Pick").await,
        (200, json!({ "policy": "DIRECT" }))
    );
    let (head, body) = get_via_proxy(api.http(), &api.target_url()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hi there");

    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(&api.state_path).unwrap()).unwrap();
    assert_eq!(saved["group_selections"]["t.conf"]["Pick"], "DIRECT");
}

#[tokio::test]
async fn select_refuses_what_is_not_a_member_of_a_select_group() {
    let api = api().await;
    for (body, message) in [
        (
            json!({ "group_name": "Nope", "policy": "DIRECT" }),
            "unknown policy group `Nope`",
        ),
        (
            json!({ "group_name": "HK", "policy": "DIRECT" }),
            "unknown policy group `HK`",
        ),
        (
            json!({ "group_name": "Pick", "policy": "Block" }),
            "`Block` is not a member of `Pick`",
        ),
    ] {
        let (status, answer) = post(&api, "/v1/policy_groups/select", body).await;
        assert_eq!((status, answer), (400, json!({ "error": message })));
    }
    let (status, _) = post(
        &api,
        "/v1/policy_groups/select",
        json!({ "group_name": "Pick" }),
    )
    .await;
    assert_eq!(status, 400, "a body without `policy`");
    assert_eq!(
        get(&api, "/v1/policy_groups/select?group_name=Nope").await,
        (404, json!({ "error": "unknown policy group `Nope`" }))
    );
    let (status, _) = get(&api, "/v1/policy_groups/select").await;
    assert_eq!(status, 400);
    assert_eq!(
        get(&api, "/v1/policy_groups/select?group_name=Pick").await,
        (200, json!({ "policy": "HK" })),
        "nothing changed"
    );
}

/// Falls over from HK (a protocol rurge does not speak: it never passes)
/// to DIRECT, tested at `internet-test-url` on the loopback server.
const AUTO: &str = "Auto = fallback, HK, DIRECT";

#[tokio::test]
async fn policies_are_tested_on_request() {
    let api = api_in("", AUTO).await;
    // at a URL of the caller's: a one-off, nothing is kept
    let (status, body) = post(
        &api,
        "/v1/policies/test",
        json!({ "policy_names": ["DIRECT"], "url": api.target_url() }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(body["DIRECT"]["delay"].is_u64(), "{body}");
    assert!(body["DIRECT"]["time"].is_f64(), "{body}");
    let (_, results) = get(&api, "/v1/policy_groups/test_results").await;
    assert_eq!(results["Auto"]["DIRECT"], Value::Null, "{results}");
    // at each one's own test URL: kept, and the groups pick by it
    let (status, body) = post(
        &api,
        "/v1/policies/test",
        json!({ "policy_names": ["DIRECT", "Block", "HK", "Pick"] }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(body["DIRECT"]["delay"].is_u64(), "{body}");
    for name in ["Block", "HK", "Pick"] {
        assert_eq!(body[name], json!({ "error": "not testable" }), "{name}");
    }
    let (_, results) = get(&api, "/v1/policy_groups/test_results").await;
    assert!(results["Auto"]["DIRECT"]["delay"].is_u64(), "{results}");
    assert_eq!(
        post(
            &api,
            "/v1/policies/test",
            json!({ "policy_names": ["DIRECT", "Nope"] })
        )
        .await,
        (400, json!({ "error": "unknown policy `Nope`" }))
    );
    assert_eq!(
        post(
            &api,
            "/v1/policies/test",
            json!({ "policy_names": ["DIRECT"], "url": "not a url" })
        )
        .await,
        (400, json!({ "error": "`url` is not a URL" }))
    );
}

#[tokio::test]
async fn a_group_is_tested_on_request() {
    let api = api_in("", AUTO).await;
    assert_eq!(
        get(&api, "/v1/policy_groups/test_results").await,
        (200, json!({ "Auto": { "HK": null, "DIRECT": null } })),
        "the automatic groups only, nothing tested yet"
    );
    assert_eq!(
        post(
            &api,
            "/v1/policy_groups/test",
            json!({ "group_name": "Auto" })
        )
        .await,
        (200, json!({ "available": ["DIRECT"] }))
    );
    let (_, results) = get(&api, "/v1/policy_groups/test_results").await;
    assert!(results["Auto"]["DIRECT"]["delay"].is_u64(), "{results}");
    assert_eq!(results["Auto"]["HK"], Value::Null);
    assert_eq!(
        get(&api, "/v1/policy_groups/select?group_name=Auto").await,
        (200, json!({ "policy": "DIRECT" })),
        "HK fails: the group falls over to DIRECT"
    );
    assert_eq!(
        post(
            &api,
            "/v1/policy_groups/test",
            json!({ "group_name": "Nope" })
        )
        .await,
        (400, json!({ "error": "unknown policy group `Nope`" }))
    );
}

#[tokio::test]
async fn select_on_an_automatic_group_overrides_it_until_cleared() {
    let api = api_in("DOMAIN,target.test,Auto", AUTO).await;
    let select = |policy: &str| json!({ "group_name": "Auto", "policy": policy });
    assert_eq!(
        post(&api, "/v1/policy_groups/select", select("DIRECT")).await,
        (200, json!({}))
    );
    assert_eq!(
        get(&api, "/v1/policy_groups/select?group_name=Auto").await,
        (200, json!({ "policy": "DIRECT" }))
    );
    let (head, body) = get_via_proxy(api.http(), &api.target_url()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hi there");
    // an override is not written to state.json
    let saved: Value = std::fs::read_to_string(&api.state_path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    assert_eq!(saved["group_selections"]["t.conf"]["Auto"], Value::Null);
    // an empty `policy` clears it: nothing tested yet, the first member
    assert_eq!(
        post(&api, "/v1/policy_groups/select", select("")).await,
        (200, json!({}))
    );
    assert_eq!(
        get(&api, "/v1/policy_groups/select?group_name=Auto").await,
        (200, json!({ "policy": "HK" }))
    );
    assert_eq!(
        post(&api, "/v1/policy_groups/select", select("Block")).await,
        (400, json!({ "error": "`Block` is not a member of `Auto`" }))
    );
}

#[tokio::test]
async fn profile_check_includes_the_dry_build() {
    let api = api().await;
    let text = std::fs::read_to_string(&api.conf).unwrap().replace(
        "[Proxy Group]",
        "Up = https, proxy.test, 443, client-cert=cert1\n[Keystore]\ncert1 = type=p12, base64=QUJD, password=hunter2\n[Proxy Group]",
    );
    std::fs::write(&api.conf, text).unwrap();
    let (status, body) = post(&api, "/v1/profiles/check", json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(body["ok"], false);
    let codes: Vec<&str> = body["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["code"].as_str())
        .collect();
    assert!(codes.contains(&"E0022"), "{codes:?}");
    assert!(!body.to_string().contains("hunter2"));
}

/// The test databases of the rule engine's crate.
fn geoip_fixture(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../rurge-rules/tests/fixtures")
        .join(file)
}

/// Puts the test databases where the engine looks for the ones it uses.
fn with_geoip_databases(data: &Path) {
    let dir = data.join("geoip");
    std::fs::create_dir_all(&dir).unwrap();
    for (fixture, name) in [
        ("GeoIP2-Country-Test.mmdb", COUNTRY_FILE),
        ("GeoLite2-ASN-Test.mmdb", ASN_FILE),
    ] {
        std::fs::copy(geoip_fixture(fixture), dir.join(name)).unwrap();
    }
}

/// `/v1/geoip` answers from the databases the rules use: each field from the
/// one that has it, the dates the ones the databases were built on, and
/// `null` for what an address has no entry for.
#[tokio::test]
async fn geoip_says_what_the_databases_in_use_know() {
    let api = api_seeded("", "", with_geoip_databases).await;
    let built = |fixture: &str, kind| {
        GeoDb::validate(&std::fs::read(geoip_fixture(fixture)).unwrap(), kind).unwrap()
    };
    let country_date = built("GeoIP2-Country-Test.mmdb", DbKind::Country);
    let asn_date = built("GeoLite2-ASN-Test.mmdb", DbKind::Asn);
    let ask = async |ip: &str| get(&api, &format!("/v1/geoip?ip={ip}")).await;
    // in both databases
    assert_eq!(
        ask("89.160.20.112").await,
        (
            200,
            json!({
                "address": "89.160.20.112",
                "country": "SE",
                "asn": 29518,
                "organization": "Bredband2 AB",
                "geoip-db-date": country_date,
                "asn-db-date": asn_date,
            })
        )
    );
    // an IPv4-mapped address is the IPv4 address, as everywhere else
    assert_eq!(
        ask("::ffff:89.160.20.112").await,
        ask("89.160.20.112").await
    );
    // in the country database only; the address is echoed as parsed
    assert_eq!(
        ask("2001:0218:0:0:0:0:0:1").await,
        (
            200,
            json!({
                "address": "2001:218::1",
                "country": "JP",
                "asn": null,
                "organization": null,
                "geoip-db-date": country_date,
                "asn-db-date": asn_date,
            })
        )
    );
    // in neither: nothing to say, and the dates are still there
    assert_eq!(
        ask("127.0.0.1").await,
        (
            200,
            json!({
                "address": "127.0.0.1",
                "country": null,
                "asn": null,
                "organization": null,
                "geoip-db-date": country_date,
                "asn-db-date": asn_date,
            })
        )
    );
}

/// With no database in the data directory every field is `null`.
#[tokio::test]
async fn geoip_without_databases_has_nothing_to_say() {
    let api = api().await;
    assert_eq!(
        get(&api, "/v1/geoip?ip=89.160.20.112").await,
        (
            200,
            json!({
                "address": "89.160.20.112",
                "country": null,
                "asn": null,
                "organization": null,
                "geoip-db-date": null,
                "asn-db-date": null,
            })
        )
    );
}

/// An `ip` that is missing or no address is a 400 in the shape of every other
/// one; the endpoint wants the key like the rest.
#[tokio::test]
async fn geoip_wants_an_address_and_the_key() {
    let api = api().await;
    assert_eq!(
        get(&api, "/v1/geoip?ip=nonsense").await,
        (400, json!({ "error": "ip is not a valid IP address" }))
    );
    assert_eq!(
        get(&api, "/v1/geoip?ip=").await,
        (400, json!({ "error": "ip is not a valid IP address" }))
    );
    let (status, body) = get(&api, "/v1/geoip").await;
    assert_eq!(status, 400, "{body}");
    assert!(body["error"].as_str().unwrap().contains("`ip`"), "{body}");
    assert_eq!(
        call(api.addr, "GET", "/v1/geoip?ip=1.0.0.1", None, None).await,
        (401, json!({ "error": "unauthorized" }))
    );
}

// ---------------------------------------------------------------------------
// external resources

/// What the links of these tests carry in their query. No answer of the two
/// endpoints holds it.
const TOKEN: &str = "S3CR3T";

/// What a rule set says before it is updated: nothing about `target.test`.
const OLD_RULES: &str = "DOMAIN,other.test\n";

/// A call of one of the two endpoints, whose answer never holds the token.
async fn resources(
    addr: SocketAddr,
    method: &str,
    path: &str,
    key: Option<&str>,
    body: Option<String>,
) -> (u16, Value) {
    let (status, _, bytes) = call_text(addr, method, path, key, body).await;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    assert!(!text.contains(TOKEN), "{text}");
    let value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    (status, value)
}

/// What `GET /v1/external_resources` lists.
async fn listed(addr: SocketAddr) -> Vec<Value> {
    let (status, body) = resources(addr, "GET", "/v1/external_resources", Some(KEY), None).await;
    assert_eq!(status, 200, "{body}");
    body["defines"].as_array().expect("a list").clone()
}

/// The answer to an update of `key`, which is a key or `all`.
async fn updated(addr: SocketAddr, key: &str) -> (u16, Value) {
    resources(
        addr,
        "POST",
        "/v1/external_resources/update",
        Some(KEY),
        Some(json!({ "key": key }).to_string()),
    )
    .await
}

fn key_of(url: &url::Url) -> String {
    resource_key(&ResourceSource::Url(url.clone()))
}

/// An API over a profile that rejects what a rule set by address names, and has
/// a rule set by file, a domain set by address and a subscription whose link has
/// a token. Their copies were checked ten minutes ago, and `served` gives the
/// same, so nothing is due. With `online` the generation downloads: its
/// databases are asked of `served`, which has none.
async fn api_with_sets(online: bool) -> (Api, TestServer, SystemTime) {
    let served = TestServer::spawn().await;
    served.set("/rules.list", OLD_RULES);
    served.set("/domains.txt", "domains.example\n");
    served.set("/nodes", "N1 = http, n1.test, 80\n");
    let rules_url = served.url("/rules.list");
    let domains_url = served.url("/domains.txt");
    let nodes_url = served.url(&format!("/nodes?token={TOKEN}"));
    let rules = format!(
        "RULE-SET,{rules_url},REJECT\nRULE-SET,local.list,DIRECT\nDOMAIN-SET,{domains_url},DIRECT"
    );
    let groups = format!("Sub = select, policy-path={nodes_url}");
    let checked = SystemTime::now() - Duration::from_secs(600);
    let seed = |dir: &Path| {
        std::fs::write(dir.join("local.list"), "DOMAIN,local.example\n").unwrap();
        seed_cache(dir, &rules_url, OLD_RULES.as_bytes(), checked);
        seed_cache(dir, &domains_url, b"domains.example\n", checked);
        seed_cache(dir, &nodes_url, b"N1 = http, n1.test, 80\n", checked);
    };
    let api = if online {
        api_online(&rules, &groups, &served, seed).await
    } else {
        api_seeded(&rules, &groups, seed).await
    };
    (api, served, checked)
}

/// The link of the managed profile of `profile_sources`. It holds the token in
/// every place a link can: user name, password, path and query.
fn managed_link(served: &TestServer) -> url::Url {
    let mut link = served.url(&format!("/{TOKEN}/managed?token={TOKEN}"));
    link.set_username(TOKEN).unwrap();
    link.set_password(Some(TOKEN)).unwrap();
    link
}

/// The link of the file the profile of `profile_sources` includes.
fn included_link(served: &TestServer) -> url::Url {
    served.url(&format!("/{TOKEN}/inc?token={TOKEN}"))
}

/// The sources of a profile that is managed from `managed_link` and includes
/// `included_link`, both on `served`, both checked ten minutes ago, followed as
/// the daemon follows them.
fn profile_sources(api: &Api, served: &TestServer) -> Arc<ProfileSources> {
    let root = api.dir.path();
    let sources = ProfileSources::new(SourcesOptions {
        profile: api.conf.clone(),
        load: load_options(),
        data_dir: root.to_path_buf(),
        offline: false,
        client: resource_client().unwrap(),
        resources: ResourceOptions::default(),
    });
    let checked = SystemTime::now() - Duration::from_secs(600);
    let remote = |url: url::Url, role| {
        served.set(url.path(), "[Rule]\nFINAL,DIRECT\n");
        seed_cache(root, &url, b"[Rule]\nFINAL,DIRECT\n", checked);
        RemoteRef {
            path: Some(cache_file(root, &url)),
            url,
            role,
            interval: Duration::from_secs(3600),
            strict: false,
            lacks_section: false,
        }
    };
    sources.track(&SourceInfo {
        main: api.conf.clone(),
        includes: Vec::new(),
        main_text: api.conf.clone(),
        remote: vec![
            remote(managed_link(served), RemoteRole::Managed),
            remote(included_link(served), RemoteRole::Include),
        ],
    });
    sources
}

/// One item for each kind of resource, in the shape the manual gives: a local
/// resource has no `updatedAt`, `fromModule` is there and is `null`, `error` is
/// there when the last update failed, and `suspended` is not there.
#[tokio::test]
async fn external_resources_are_listed_one_item_for_each_kind() {
    let (api, served, checked) = api_with_sets(true).await;
    let at = checked.duration_since(UNIX_EPOCH).unwrap().as_secs();
    let downloaded = |path: String, kind: &str, key: String| {
        json!({
            "path": path, "type": kind, "key": key, "local": false, "ready": true,
            "updatedAt": at, "updating": false, "fromModule": null
        })
    };
    let database = |file: &str, path: &str| {
        json!({
            "path": file, "type": "geoip", "key": key_of(&served.url(path)), "local": false,
            "ready": false, "updatedAt": 0, "updating": false, "fromModule": null,
            "error": "http status 404"
        })
    };
    let file = api.dir.path().join("local.list");
    let mut expected = vec![
        downloaded(
            served.url("/rules.list").to_string(),
            "ruleset",
            key_of(&served.url("/rules.list")),
        ),
        downloaded(
            served.url("/domains.txt").to_string(),
            "domainset",
            key_of(&served.url("/domains.txt")),
        ),
        downloaded(
            "policy-path of `Sub`".to_string(),
            "policy-group",
            key_of(&served.url(&format!("/nodes?token={TOKEN}"))),
        ),
        json!({
            "path": file.display().to_string(), "type": "ruleset",
            "key": resource_key(&ResourceSource::File(file)), "local": true,
            "ready": true, "updating": false, "fromModule": null
        }),
        database("GeoLite2-Country.mmdb", "/geo/country.mmdb"),
        database("GeoLite2-ASN.mmdb", "/geo/asn.mmdb"),
    ];
    // by name
    expected.sort_by(|a, b| {
        (a["path"].as_str(), a["key"].as_str()).cmp(&(b["path"].as_str(), b["key"].as_str()))
    });
    assert_eq!(listed(api.addr).await, expected);
}

/// The sources of the profile come first, the managed profile before the
/// includes; the one that is managed says it is suspended while the engine
/// refuses sessions, and everything goes on being answered.
#[tokio::test]
async fn external_resources_of_the_profile_come_first_and_the_managed_one_is_flagged() {
    let (api, served, _) = api_with_sets(true).await;
    let sources = profile_sources(&api, &served);
    let addr = api.with_sources(sources).await;
    let origin = format!("http://127.0.0.1:{}/…", served.url("/").port().unwrap());

    let defines = listed(addr).await;
    let kinds: Vec<&str> = defines
        .iter()
        .map(|d| d["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds[..2], ["profile", "include"], "{defines:?}");
    assert_eq!(defines.len(), 8);
    assert_eq!(
        defines[0]["path"],
        json!(format!("managed profile ({origin})"))
    );
    assert_eq!(defines[0]["key"], json!(key_of(&managed_link(&served))));
    assert_eq!(defines[0]["ready"], json!(true));
    let include_key = key_of(&included_link(&served));
    assert_eq!(defines[1]["key"], json!(include_key));
    assert_eq!(
        defines[1]["path"],
        json!(format!("remote include {} ({origin})", &include_key[..8]))
    );
    assert!(defines.iter().all(|d| d.get("suspended").is_none()));

    api.engine.suspend(Some("http status 500".to_string()));
    let defines = listed(addr).await;
    assert_eq!(defines[0]["suspended"], json!(true));
    assert!(defines[1..].iter().all(|d| d.get("suspended").is_none()));
    assert_eq!(
        call(addr, "GET", "/v1/outbound", Some(KEY), None).await,
        (200, json!({ "mode": "rule" }))
    );
    assert_eq!(
        updated(addr, &include_key).await,
        (200, json!({ include_key: "success" }))
    );
}

/// An update of a rule set is asked of its server and takes effect: what the
/// set says is what the rules do from then on. A server that fails says how.
#[tokio::test]
async fn external_resources_are_updated_one_key_at_a_time() {
    let (api, served, _) = api_with_sets(true).await;
    let key = key_of(&served.url("/rules.list"));
    let (head, _) = get_via_proxy(api.http(), &api.target_url()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");

    served.set("/rules.list", "DOMAIN,target.test\n");
    assert_eq!(
        updated(api.addr, &key).await,
        (200, json!({ key.clone(): "success" }))
    );
    wait_until(async || {
        let (head, _) = get_via_proxy(api.http(), &api.target_url()).await;
        !head.starts_with("HTTP/1.1 200")
    })
    .await;

    // a server that answers 500
    served.set_status("/rules.list", 500);
    assert_eq!(
        updated(api.addr, &key).await,
        (200, json!({ key.clone(): "http status 500" }))
    );
    let defines = listed(api.addr).await;
    let item = defines.iter().find(|d| d["key"] == json!(key)).unwrap();
    assert_eq!(item["error"], json!("http status 500"));
    assert_eq!(item["ready"], json!(true), "the copy it had stays");

    // a file is read again
    let file = api.dir.path().join("local.list");
    let file_key = resource_key(&ResourceSource::File(file));
    assert_eq!(
        updated(api.addr, &file_key).await,
        (200, json!({ file_key: "success" }))
    );
}

/// `all` updates every resource that is downloaded, the sources of the profile
/// included, and none that is a file.
#[tokio::test]
async fn external_resources_are_all_updated_but_the_files() {
    let (api, served, _) = api_with_sets(true).await;
    let sources = profile_sources(&api, &served);
    let addr = api.with_sources(sources).await;
    let (status, body) = updated(addr, "all").await;
    assert_eq!(status, 200, "{body}");
    let file = api.dir.path().join("local.list");
    let file_key = resource_key(&ResourceSource::File(file));
    let mut expected = serde_json::Map::new();
    for link in [
        managed_link(&served),
        included_link(&served),
        served.url("/rules.list"),
        served.url("/domains.txt"),
    ] {
        expected.insert(key_of(&link), json!("success"));
    }
    expected.insert(
        key_of(&served.url(&format!("/nodes?token={TOKEN}"))),
        json!("success"),
    );
    for path in ["/geo/country.mmdb", "/geo/asn.mmdb"] {
        expected.insert(key_of(&served.url(path)), json!("http status 404"));
    }
    assert_eq!(body, Value::Object(expected));
    assert!(body.get(&file_key).is_none());
}

/// Offline nothing is downloaded, and every key that would be says so; a file
/// is read all the same.
#[tokio::test]
async fn external_resources_are_not_downloaded_offline() {
    let (api, served, _) = api_with_sets(false).await;
    let rules_key = key_of(&served.url("/rules.list"));
    let offline = "offline mode: nothing is downloaded";
    assert_eq!(
        updated(api.addr, &rules_key).await,
        (200, json!({ rules_key.clone(): offline }))
    );
    let (status, body) = updated(api.addr, "all").await;
    assert_eq!(status, 200, "{body}");
    let mut expected = serde_json::Map::new();
    for path in ["/rules.list", "/domains.txt"] {
        expected.insert(key_of(&served.url(path)), json!(offline));
    }
    expected.insert(
        key_of(&served.url(&format!("/nodes?token={TOKEN}"))),
        json!(offline),
    );
    assert_eq!(body, Value::Object(expected));
    assert_eq!(
        served.requests().len(),
        0,
        "offline asks nobody: {:?}",
        served.requests()
    );
}

/// A body that is not a JSON object with a string `key` is a 400, and so is a
/// key that names nothing: with a sentence of the endpoint's own.
#[tokio::test]
async fn external_resources_say_no_to_a_bad_body_and_to_a_key_that_names_nothing() {
    let (api, _served, _) = api_with_sets(false).await;
    let shape = json!({ "error": "the body must be a JSON object with a string \"key\"" });
    for body in [
        "S3CR3T and not JSON",
        "",
        r#"["S3CR3T"]"#,
        r#"{"S3CR3T": 1}"#,
        r#"{"key": 31337}"#,
        r#"{"key": 3.1337}"#,
        r#"{"key": true}"#,
        r#"{"key": ["S3CR3T"]}"#,
        r#"{"key": {"S3CR3T": 31337}}"#,
        r#"{"key": null}"#,
    ] {
        let (status, _, bytes) = call_text(
            api.addr,
            "POST",
            "/v1/external_resources/update",
            Some(KEY),
            Some(body.to_string()),
        )
        .await;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        assert_eq!(status, 400, "{body}: {text}");
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            shape,
            "{body}"
        );
    }
    // a key that names nothing, the empty string included
    for key in ["S3CR3T", "", "no-such-key"] {
        assert_eq!(
            updated(api.addr, key).await,
            (400, json!({ "error": "no external resource has this key" })),
            "{key:?}"
        );
    }
}

/// What the caller sent is not in the answer, whichever way it is wrong: as
/// the key, as a value of another type, or as the name of a field.
#[tokio::test]
async fn external_resources_do_not_repeat_what_a_bad_request_sent() {
    let (api, _served, _) = api_with_sets(false).await;
    for sent in ["S3CR3T", "31337", "true", "3.1337"] {
        for body in [
            format!(r#"{{"key": "{sent}"}}"#),
            format!(r#"{{"key": {sent}}}"#),
            format!(r#"{{"key": [{sent}]}}"#),
            format!(r#"{{"{sent}": "x"}}"#),
            format!(r#"[{sent}]"#),
            sent.to_string(),
        ] {
            let (status, _, bytes) = call_text(
                api.addr,
                "POST",
                "/v1/external_resources/update",
                Some(KEY),
                Some(body.clone()),
            )
            .await;
            let text = String::from_utf8_lossy(&bytes);
            assert_eq!(status, 400, "{body}: {text}");
            assert!(!text.contains(sent), "{body}: {text}");
        }
    }
}

/// Both endpoints want the key like the rest.
#[tokio::test]
async fn external_resources_want_the_key() {
    let api = api().await;
    for (method, path, body) in [
        ("GET", "/v1/external_resources", None),
        (
            "POST",
            "/v1/external_resources/update",
            Some(json!({ "key": "all" })),
        ),
    ] {
        assert_eq!(
            call(api.addr, method, path, None, body).await,
            (401, json!({ "error": "unauthorized" })),
            "{method} {path}"
        );
    }
}
