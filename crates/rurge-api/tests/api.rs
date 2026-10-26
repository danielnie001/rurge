//! Real engine (MockDns + TestServer + loopback listeners) behind the API,
//! driven with a hyper-util client (M4 design §9).

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use rurge_api::{ApiContext, BAN_FAILURES, ServerFuture, serve};
use rurge_config::config::{LoadOptions, from_text};
use rurge_config::session::ListenerKind;
use rurge_config::{RemoteRef, RemoteRole, SourceInfo};
use rurge_dns::system::StaticSystemDns;
use rurge_dns::testing::MockDns;
use rurge_engine::control::{Control, EnhancedStatus, LogLevel, ReloadReport};
use rurge_engine::profile::{ProfileSources, SourcesOptions};
use rurge_engine::stack::{StackOptions, resource_client};
use rurge_engine::state::{STATE_FILE, State, StateStore};
use rurge_engine::{
    Engine, EngineShared, ListenerSpec, Running, Runtime, RuntimeOptions, TunCounters,
};
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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
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
    enhanced: Mutex<EnhancedStatus>,
    fail_enhanced: AtomicBool,
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
    fn set_enhanced_mode(&self, enabled: bool) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            if self.fail_enhanced.load(Ordering::SeqCst) {
                return Err("no virtual adapter could be made (test)".to_string());
            }
            self.enhanced.lock().unwrap().enabled = enabled;
            Ok(())
        })
    }
    fn enhanced_mode(&self) -> EnhancedStatus {
        *self.enhanced.lock().unwrap()
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

    /// A second API server over the same engine, for the test that ends it:
    /// its future is handed back, not spawned, and the token that ends it is
    /// its own.
    async fn serve_apart(&self) -> (SocketAddr, ServerFuture, CancellationToken) {
        let token = CancellationToken::new();
        let (addr, server) = serve(
            "127.0.0.1:0".parse().unwrap(),
            KEY.to_string(),
            ApiContext {
                engine: self.engine.clone(),
                control: self.control.clone(),
                load_options: load_options(),
                sources: None,
            },
            token.clone(),
        )
        .await
        .unwrap();
        (addr, server, token)
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
                dns_override: Default::default(),
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
        "only system_proxy and enhanced_mode are live"
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

/// `GET /v1/features/enhanced_mode`: the two switches it has, the ten reasons a
/// packet is dropped, and the panics of the TCP stacks (M3-D22, P12's names).
/// A mode that is off says zero for all of them.
#[tokio::test]
async fn enhanced_mode_get_has_the_counters_and_the_shape() {
    let api = api().await;
    let off = json!({
        "enabled": false,
        "dropped": {
            "malformed": 0,
            "fragment": 0,
            "martian": 0,
            "unsupported": 0,
            "queueFull": 0,
            "unknownFake": 0,
            "busy": 0,
            "beyondWindow": 0,
            "rstLimited": 0,
            "icmpLimited": 0,
        },
        "stackPanics": 0,
    });
    assert_eq!(get(&api, "/v1/features/enhanced_mode").await, (200, off));
    {
        let mut status = api.control.enhanced.lock().unwrap();
        status.enabled = true;
        status.counters = TunCounters {
            malformed: 1,
            fragment: 2,
            martian: 3,
            unsupported: 4,
            queue_full: 5,
            unknown_fake: 6,
            busy: 7,
            beyond_window: 8,
            rst_limited: 9,
            icmp_limited: 10,
            stack_panics: 11,
        };
    }
    assert_eq!(
        get(&api, "/v1/features/enhanced_mode").await,
        (
            200,
            json!({
                "enabled": true,
                "dropped": {
                    "malformed": 1,
                    "fragment": 2,
                    "martian": 3,
                    "unsupported": 4,
                    "queueFull": 5,
                    "unknownFake": 6,
                    "busy": 7,
                    "beyondWindow": 8,
                    "rstLimited": 9,
                    "icmpLimited": 10,
                },
                "stackPanics": 11,
            })
        )
    );
}

/// `POST /v1/features/enhanced_mode` goes to the control, whose refusal is a
/// 500 with its reason; an open that is refused leaves the mode off, which is
/// what `GET` goes on saying.
#[tokio::test]
async fn enhanced_mode_post_goes_to_the_control_and_its_error_is_a_500() {
    let api = api().await;
    assert_eq!(
        get(&api, "/v1/features/enhanced_mode").await.1["enabled"],
        false,
        "it begins off"
    );
    api.control.fail_enhanced.store(true, Ordering::SeqCst);
    let (status, body) = post(
        &api,
        "/v1/features/enhanced_mode",
        json!({ "enabled": true }),
    )
    .await;
    assert_eq!(status, 500, "{body}");
    assert_eq!(body["error"], "no virtual adapter could be made (test)");
    assert_eq!(
        get(&api, "/v1/features/enhanced_mode").await.1["enabled"],
        false,
        "the refusal left it off"
    );
    // and once it is taken, the same switch turns it off again
    api.control.fail_enhanced.store(false, Ordering::SeqCst);
    assert_eq!(
        post(
            &api,
            "/v1/features/enhanced_mode",
            json!({ "enabled": true })
        )
        .await,
        (200, json!({}))
    );
    assert_eq!(
        get(&api, "/v1/features/enhanced_mode").await.1["enabled"],
        true
    );
    // the body is read as the other features' are
    let (status, _) = post(
        &api,
        "/v1/features/enhanced_mode",
        json!({ "enabled": "yes" }),
    )
    .await;
    assert_eq!(status, 400);
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
    // the virtual adapter has its bucket too, and an HTTP session is not in it
    assert_eq!(body["listener"]["tun"]["in"], 0, "{body}");
    assert_eq!(body["listener"]["tun"]["out"], 0, "{body}");
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

/// The current speeds are the bytes that moved since the last sample, each way
/// under its own name: a tunnel that is only written to shows an upload speed
/// and no download speed, and both are back at zero once nothing moves.
#[tokio::test]
async fn the_current_speed_follows_the_bytes_that_move_and_stops_with_them() {
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
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
    // a request that never ends: the target answers nothing, so only upload
    let payload = format!(
        "GET /hello HTTP/1.1\r\nHost: {host}\r\nX-Pad: {}\r\n",
        "p".repeat(400)
    );
    s.write_all(payload.as_bytes()).await.unwrap();
    s.flush().await.unwrap();
    let mut total = Value::Null;
    wait_until(async || {
        total = get(&api, "/v1/traffic").await.1["total"].clone();
        total["outCurrentSpeed"].as_u64().unwrap_or(0) > 0
    })
    .await;
    assert_eq!(total["inCurrentSpeed"], 0, "{total}");
    wait_until(async || {
        let total = get(&api, "/v1/traffic").await.1["total"].clone();
        total["outCurrentSpeed"] == 0 && total["inCurrentSpeed"] == 0
    })
    .await;
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

/// A `subnet` group takes an override as an automatic group does (phase 3 M1
/// design 6.2): one of its members, which the group dials instead of what
/// the network says until an empty `policy` clears it, and which is not
/// written to state.json.
#[tokio::test]
async fn select_on_a_subnet_group_overrides_it_until_cleared() {
    let api = api_in(
        "DOMAIN,target.test,Net",
        "Net = subnet, TYPE:WIFI = DIRECT, default = HK",
    )
    .await;
    let select = |policy: &str| json!({ "group_name": "Net", "policy": policy });
    // the network is not known: the default, a protocol rurge does not speak,
    // which refuses
    assert_eq!(
        get(&api, "/v1/policy_groups/select?group_name=Net").await,
        (200, json!({ "policy": "HK" }))
    );
    let (head, _) = get_via_proxy(api.http(), &api.target_url()).await;
    assert!(!head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(
        post(&api, "/v1/policy_groups/select", select("DIRECT")).await,
        (200, json!({}))
    );
    assert_eq!(
        get(&api, "/v1/policy_groups/select?group_name=Net").await,
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
    assert_eq!(saved["group_selections"]["t.conf"]["Net"], Value::Null);
    // an empty `policy` clears it
    assert_eq!(
        post(&api, "/v1/policy_groups/select", select("")).await,
        (200, json!({}))
    );
    assert_eq!(
        get(&api, "/v1/policy_groups/select?group_name=Net").await,
        (200, json!({ "policy": "HK" }))
    );
    assert_eq!(
        post(&api, "/v1/policy_groups/select", select("Block")).await,
        (400, json!({ "error": "`Block` is not a member of `Net`" }))
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

const CHECK: &str = "/v1/profiles/check";

/// The manual: "Otherwise, the `error` field is null." A script written to it
/// reads `error` to tell a valid profile, so the key is always there and, for
/// a profile with no error in it, null, whatever the request has for a body. A
/// warning is no error: the profile of the harness has one.
#[tokio::test]
async fn profile_check_says_null_in_error_for_a_valid_profile() {
    let api = api().await;
    for body in [None, Some(json!({}))] {
        let (status, _, bytes) = call_raw(api.addr, "POST", CHECK, Some(KEY), body.clone()).await;
        let answer: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status, 200, "{answer}");
        // `answer["error"]` reads as null where there is no such key, too
        assert_eq!(
            answer.get("error"),
            Some(&Value::Null),
            "{body:?}: {answer}"
        );
        assert_eq!(answer["ok"], true, "{answer}");
        assert!(answer["warnings"].as_u64().unwrap() >= 1, "{answer}");
    }
}

/// A profile with errors in it says what is wrong in `error`: the first error
/// in the order of `diagnostics`, as a log record would write it, and how many
/// more there are. The check does not touch the profile that runs.
#[tokio::test]
async fn profile_check_says_what_is_wrong_in_error() {
    let api = api().await;
    let sound = std::fs::read_to_string(&api.conf).unwrap();
    // The warning of the harness's profile comes before all of these in
    // `diagnostics`: the first error is not the first diagnostic.
    let one = sound.replace("FINAL,DIRECT", "FINAL,NoSuchPolicy");
    let two = one.replace("DOMAIN,ads.test,REJECT", "DOMAIN,ads.test,NoSuchPolicy");
    let three = two.replace(
        "DOMAIN,ads.test,NoSuchPolicy",
        "DOMAIN,ads.test,NoSuchPolicy\nDOMAIN,more.test,NoSuchPolicy",
    );
    for (text, count) in [(one, 1), (two, 2), (three, 3)] {
        std::fs::write(&api.conf, text).unwrap();
        let (status, body) = post(&api, CHECK, json!({})).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            (&body["ok"], &body["errors"]),
            (&json!(false), &json!(count))
        );
        let listed = body["diagnostics"].as_array().unwrap();
        let errors: Vec<&Value> = listed.iter().filter(|d| d["severity"] == "error").collect();
        assert_eq!(errors.len(), count, "{body}");
        let place = |d: &Value| {
            format!(
                "{}:{}",
                d["span"]["file"].as_str().unwrap(),
                d["span"]["line"]
            )
        };
        let error = body["error"].as_str().unwrap_or_else(|| panic!("{body}"));
        let first = errors[0];
        assert!(
            error.contains(first["code"].as_str().unwrap())
                && error.contains(first["message"].as_str().unwrap())
                && error.contains(&place(first)),
            "not the first error: {error}"
        );
        if count == 1 {
            assert!(!error.contains(" (and "), "{error}");
        } else {
            assert!(
                error.ends_with(&format!(" (and {} more)", count - 1)),
                "{error}"
            );
            assert!(!error.contains(&place(errors[count - 1])), "{error}");
        }
    }
    let (status, rules) = get(&api, "/v1/rules").await;
    let rules = rules.to_string();
    assert!(
        status == 200 && rules.contains("FINAL,DIRECT") && !rules.contains("NoSuchPolicy"),
        "{rules}"
    );
}

/// `name` is the manual's way to pick one of the profiles Surge keeps in a
/// directory. rurge has the one that runs, the file `-c` names, so a name is
/// either that one's (the file's name, or that without its last extension) or
/// refused. A refusal is given before anything is loaded.
#[tokio::test]
async fn profile_check_takes_the_name_of_the_running_profile_and_no_other() {
    let api = api().await;
    // the harness's profile is `t.conf`
    let (status, sound) = post(&api, CHECK, json!({})).await;
    assert_eq!(status, 200, "{sound}");
    // what a body that is no name gets while the profile can be loaded
    let (status, no_name) = post(&api, CHECK, json!([])).await;
    assert_eq!(status, 400, "{no_name}");
    for body in [
        json!({ "other": 5 }),
        json!({ "name": "t.conf" }),
        json!({ "name": "t" }),
        json!({ "name": "t.conf", "other": 5 }),
    ] {
        assert_eq!(
            post(&api, CHECK, body.clone()).await,
            (200, sound.clone()),
            "{body}"
        );
    }
    let text = std::fs::read_to_string(&api.conf).unwrap();
    std::fs::write(
        &api.conf,
        text.replace("FINAL,DIRECT", "FINAL,NoSuchPolicy"),
    )
    .unwrap();
    let (_, broken) = post(&api, CHECK, json!({})).await;
    assert_eq!(broken["ok"], false, "{broken}");
    for name in ["t.conf", "t"] {
        let body = json!({ "name": name });
        assert_eq!(
            post(&api, CHECK, body.clone()).await,
            (200, broken.clone()),
            "{body}"
        );
    }
    let refused = json!({ "error": "profile not found" });
    // not a 200 with the errors of the file it was not asked about
    assert_eq!(
        post(&api, CHECK, json!({ "name": "S3CR3T-profile" })).await,
        (404, refused.clone())
    );
    // Both refusals are shown to come before the load the same way: the file
    // is gone, which a load fails on and which is a 500 for the name that is
    // the profile's, and neither a name that is refused nor a body that is no
    // name gets that far.
    std::fs::remove_file(&api.conf).unwrap();
    let (status, body) = post(&api, CHECK, json!({ "name": "t.conf" })).await;
    assert_eq!(status, 500, "{body}");
    assert_eq!(post(&api, CHECK, json!([])).await, (400, no_name));
    let absolute = api.conf.display().to_string();
    for name in [
        "S3CR3T-profile",
        "T.conf",
        "T.CONF",
        "T",
        "t.conf ",
        " t",
        "t.",
        "t.conf.bak",
        "conf",
        ".conf",
        "./t.conf",
        "dir/t.conf",
        "dir\\t.conf",
        "",
        absolute.as_str(),
    ] {
        let sent = json!({ "name": name });
        let (status, body) = post(&api, CHECK, sent.clone()).await;
        assert_eq!((status, &body), (404, &refused), "{sent}");
        assert!(!body.to_string().contains("S3CR3T"), "{body}");
    }
}

/// A body that is no JSON object, or has a `name` that is no string, is
/// refused with one text, and nothing that was sent comes back in it.
#[tokio::test]
async fn profile_check_refuses_a_body_that_is_no_name_with_one_text() {
    let api = api().await;
    let mut said = Vec::new();
    for body in [
        "[]",
        r#"["S3CR3T"]"#,
        r#""S3CR3T""#,
        "31337",
        "true",
        "null",
        r#"{"name": 31337}"#,
        r#"{"name": true}"#,
        r#"{"name": null}"#,
        r#"{"name": ["S3CR3T"]}"#,
        r#"{"name": {"S3CR3T": 1}}"#,
        "S3CR3T is no JSON",
        r#"{"name": "S3CR3T""#,
    ] {
        let (status, _, bytes) =
            call_text(api.addr, "POST", CHECK, Some(KEY), Some(body.to_string())).await;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        assert_eq!(status, 400, "{body}: {text}");
        for marker in ["S3CR3T", "31337"] {
            assert!(!text.contains(marker), "{body}: {text}");
        }
        said.push(text);
    }
    let first: Value = serde_json::from_str(&said[0]).unwrap();
    assert!(
        first["error"].as_str().is_some_and(|e| !e.is_empty()),
        "{first}"
    );
    assert!(said.iter().all(|text| *text == said[0]), "{said:?}");
}

/// A body of nothing but JSON whitespace is no body.
#[tokio::test]
async fn profile_check_takes_a_body_of_whitespace_for_no_body() {
    let api = api().await;
    let (status, sound) = post(&api, CHECK, json!({})).await;
    assert_eq!(status, 200, "{sound}");
    for body in [" \n", "\t\r\n ", "   "] {
        let (status, _, bytes) =
            call_text(api.addr, "POST", CHECK, Some(KEY), Some(body.to_string())).await;
        assert_eq!(status, 200, "{body:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap(),
            sound,
            "{body:?}"
        );
    }
}

/// `POST /v1/profiles/check` with the request written in one go, its head
/// saying `content_type` (nothing, for `None`).
async fn check_with_content_type(
    addr: SocketAddr,
    content_type: Option<&str>,
    body: &str,
) -> Reply {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let content_type = content_type.map_or_else(String::new, |t| format!("Content-Type: {t}\r\n"));
    let request = format!(
        "POST {CHECK} HTTP/1.1\r\nHost: {addr}\r\nX-Key: {KEY}\r\n{content_type}\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let (raw, ended_in) = read_to_the_end(&mut stream).await;
    Reply::parse(&raw, ended_in, String::new())
}

/// The content type of the request is not looked at: a script posts its body
/// as its HTTP library does, and a library may well call it a form.
#[tokio::test]
async fn profile_check_does_not_look_at_the_content_type() {
    let api = api().await;
    let (status, sound) = post(&api, CHECK, json!({})).await;
    assert_eq!(status, 200, "{sound}");
    for content_type in [
        None,
        Some("text/plain"),
        Some("application/x-www-form-urlencoded"),
        Some("application/json; charset=utf-8"),
    ] {
        check_with_content_type(api.addr, content_type, r#"{"name": "t.conf"}"#)
            .await
            .assert_answered(200, &sound);
        check_with_content_type(api.addr, content_type, r#"{"name": "S3CR3T"}"#)
            .await
            .assert_answered(404, &json!({ "error": "profile not found" }));
        check_with_content_type(api.addr, content_type, "S3CR3T")
            .await
            .assert_answered(400, &post(&api, CHECK, json!([])).await.1);
    }
}

/// The 2 MiB that a body may have for a handler to read it. A body of more is
/// refused as one that cannot be made sense of is: the same 400, for a body
/// that is nothing but whitespace as for any other. The answer is given when
/// the limit is crossed, with the rest of the body still on its way, so how the
/// connection ends is not asserted (the 64 KiB over are within what is read
/// away after the answer; a sender of much more can lose the answer, as it can
/// at any endpoint that reads its body).
///
/// The request is written in two pieces, unlike most hand-written clients
/// here: no answer can be given before more than 2 MiB of the body were read,
/// so the gap between the writes cannot lose one. The head goes first so that
/// the reading half exists while the body is written.
#[tokio::test]
async fn profile_check_refuses_a_body_too_large_to_read_with_the_same_text() {
    let api = api().await;
    let body = vec![b' '; 2 * 1024 * 1024 + 64 * 1024];
    let stream = send_a_head(api.addr, "POST", CHECK, Some(KEY), body.len()).await;
    let (mut reading, mut writing) = stream.into_split();
    let ((status, text), wrote) = tokio::join!(
        read_one_answer(&mut reading),
        tokio::time::timeout(Duration::from_secs(10), writing.write_all(&body)),
    );
    let wrote = wrote.expect("the body was neither taken nor refused within 10 s");
    assert_eq!(status, 400, "{text}; writing returned {wrote:?}");
    let (_, refused) = post(&api, CHECK, json!([])).await;
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), refused);
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
/// the daemon follows them. The managed profile says `strict`, and nothing is
/// due by it.
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
            strict: role == RemoteRole::Managed,
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
    // a set is read line by line and says how many it could not use; nothing
    // else is read that way
    let a_set = |mut item: Value| {
        item["skipped"] = json!(0);
        item["truncated"] = json!(0);
        item["unknownParams"] = json!(0);
        item
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
        a_set(downloaded(
            served.url("/rules.list").to_string(),
            "ruleset",
            key_of(&served.url("/rules.list")),
        )),
        a_set(downloaded(
            served.url("/domains.txt").to_string(),
            "domainset",
            key_of(&served.url("/domains.txt")),
        )),
        downloaded(
            "policy-path of `Sub`".to_string(),
            "policy-group",
            key_of(&served.url(&format!("/nodes?token={TOKEN}"))),
        ),
        a_set(json!({
            "path": file.display().to_string(), "type": "ruleset",
            "key": resource_key(&ResourceSource::File(file)), "local": true,
            "ready": true, "updating": false, "fromModule": null
        })),
        database("GeoLite2-Country.mmdb", "/geo/country.mmdb"),
        database("GeoLite2-ASN.mmdb", "/geo/asn.mmdb"),
    ];
    // by name
    expected.sort_by(|a, b| {
        (a["path"].as_str(), a["key"].as_str()).cmp(&(b["path"].as_str(), b["key"].as_str()))
    });
    assert_eq!(listed(api.addr).await, expected);
}

/// A rule set and a domain set say how many of their lines were not used:
/// the lines nobody could read, the lines beyond the limit, and the rules that
/// carry a parameter nobody knows. Which kind of set it is does not matter,
/// nor does whether the file is local; a set that was read whole says none.
/// The sets did load, so none of this is an `error`.
#[tokio::test]
async fn external_resources_say_how_many_lines_of_a_set_were_not_used() {
    // lines 3 and 4 cannot be read; the rules at 5 and 6 carry a parameter
    // nobody knows
    const RULES: &str = "# list\nDOMAIN,a.test\nnot a rule\nFINAL,DIRECT\nDOMAIN,b.test,bogus\nIP-CIDR,10.0.0.0/8,no-resolv\n";
    // `bad domain` and `*.wild.test` are no names
    const NAMES: &str = ".a.test\nbad domain\n*.wild.test\nb.test\n";
    // an address of this test's own: nothing is served there, and an engine
    // that asked would reach no one else
    let held = TestServer::spawn().await;
    let url_of = |path: &str| held.url(path);
    let (rules, clean, names) = (
        url_of("/rules.list"),
        url_of("/clean.list"),
        url_of("/names.txt"),
    );
    let checked = SystemTime::now() - Duration::from_secs(600);
    let api = api_seeded(
        &format!(
            "RULE-SET,{rules},DIRECT\nRULE-SET,{clean},DIRECT\nDOMAIN-SET,{names},DIRECT\nRULE-SET,local.list,DIRECT"
        ),
        "",
        |dir| {
            seed_cache(dir, &rules, RULES.as_bytes(), checked);
            seed_cache(dir, &clean, b"DOMAIN,clean.test\n", checked);
            seed_cache(dir, &names, NAMES.as_bytes(), checked);
            std::fs::write(dir.join("local.list"), RULES).unwrap();
        },
    )
    .await;
    let defines = listed(api.addr).await;
    let item_of = |key: String| {
        defines
            .iter()
            .find(|d| d["key"] == json!(key))
            .unwrap_or_else(|| panic!("no item has the key {key}: {defines:?}"))
    };
    let at = checked.duration_since(UNIX_EPOCH).unwrap().as_secs();

    // the whole item, beside the manual's fields
    assert_eq!(
        item_of(key_of(&rules)),
        &json!({
            "path": rules.to_string(), "type": "ruleset", "key": key_of(&rules),
            "local": false, "ready": true, "updatedAt": at, "updating": false,
            "fromModule": null, "skipped": 2, "truncated": 0, "unknownParams": 2
        })
    );
    let numbers =
        |item: &Value| ["skipped", "truncated", "unknownParams"].map(|field| item[field].clone());
    let file = resource_key(&ResourceSource::File(api.dir.path().join("local.list")));
    for (key, kind, expected) in [
        (key_of(&rules), "ruleset", [2, 0, 2]),
        (file, "ruleset", [2, 0, 2]),
        (key_of(&names), "domainset", [2, 0, 0]),
        (key_of(&clean), "ruleset", [0, 0, 0]),
    ] {
        let item = item_of(key);
        assert_eq!(item["type"], json!(kind), "{item}");
        assert_eq!(numbers(item), expected.map(|n| json!(n)), "{item}");
        assert!(item.get("error").is_none(), "{item}");
    }
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
    // the sources of the profile are no sets: no lines to say anything of
    for source in &defines[..2] {
        for field in ["skipped", "truncated", "unknownParams"] {
            assert!(source.get(field).is_none(), "{source}");
        }
    }
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
    // and why, which no update of the managed profile has failed to say
    assert_eq!(defines[0]["error"], json!("http status 500"));
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

// ---------------------------------------------------------------------------
// a body that comes after the answer

/// A connection with the head of a request that announces `body_len` bytes of
/// body sent on it, and nothing more.
async fn send_a_head(
    addr: SocketAddr,
    method: &str,
    path: &str,
    key: Option<&str>,
    body_len: usize,
) -> TcpStream {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\n");
    if let Some(key) = key {
        head.push_str(&format!("X-Key: {key}\r\n"));
    }
    head.push_str(&format!(
        "Content-Type: application/json\r\nContent-Length: {body_len}\r\n\r\n"
    ));
    stream.write_all(head.as_bytes()).await.unwrap();
    stream
}

/// Whether an answer begins to arrive on `stream` within `within`: `Some` once
/// its first byte is in this side's receive buffer, where `peek` leaves it,
/// unread; `None` if none came in time.
async fn peek_at_the_answer(stream: &TcpStream, within: Duration) -> Option<usize> {
    let mut first = [0u8; 1];
    match tokio::time::timeout(within, stream.peek(&mut first)).await {
        Err(_) => None,
        Ok(Ok(0)) => panic!("the connection ended before any answer"),
        Ok(Ok(n)) => Some(n),
        Ok(Err(e)) => panic!("{:?} ({e}) before any answer", e.kind()),
    }
}

/// `send_a_head`, and then the wait until the first byte of the answer is in
/// this side's receive buffer, where it stays: the client has not read it. A
/// server that answers on the head alone has answered by then, and this is the
/// client that loses its answer if the connection is torn down under it.
async fn begin_a_request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    key: Option<&str>,
    body_len: usize,
) -> TcpStream {
    let stream = send_a_head(addr, method, path, key, body_len).await;
    assert!(
        peek_at_the_answer(&stream, Duration::from_secs(5))
            .await
            .is_some(),
        "{method} {path}: no answer began to arrive within 5 s of its head"
    );
    stream
}

/// Reads `stream` until it ends, for at most 5 s: what came, and the error
/// that ended it, which is `None` for an end of stream.
async fn read_to_the_end(
    stream: &mut (impl AsyncRead + Unpin),
) -> (Vec<u8>, Option<std::io::Error>) {
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::time::timeout_at(deadline, stream.read(&mut chunk)).await {
            Err(_) => panic!(
                "the connection was still open 5 s after it was read from; it gave {:?}",
                String::from_utf8_lossy(&raw)
            ),
            Ok(Ok(0)) => return (raw, None),
            Ok(Ok(n)) => raw.extend_from_slice(&chunk[..n]),
            Ok(Err(e)) => return (raw, Some(e)),
        }
    }
}

/// What a client was answered and how its connection ended.
struct Reply {
    /// 0 when no answer came.
    status: u16,
    content_length: Option<usize>,
    body: String,
    /// How many bytes came, whole.
    read: usize,
    /// The error that ended the reading; `None` for an end of stream.
    ended_in: Option<std::io::Error>,
    /// What writing to the connection returned, for the message of a failure.
    wrote: String,
}

impl Reply {
    fn parse(raw: &[u8], ended_in: Option<std::io::Error>, wrote: String) -> Reply {
        let (head, body) = match raw.windows(4).position(|w| w == b"\r\n\r\n") {
            Some(at) => (&raw[..at], &raw[at + 4..]),
            None => (raw, &raw[raw.len()..]),
        };
        let head = String::from_utf8_lossy(head);
        let mut lines = head.lines();
        let status = lines
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        let content_length = lines
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse().ok());
        Reply {
            status,
            content_length,
            body: String::from_utf8_lossy(body).into_owned(),
            read: raw.len(),
            ended_in,
            wrote,
        }
    }

    fn describe(&self) -> String {
        let ended = match &self.ended_in {
            None => "an end of stream".to_string(),
            Some(e) => format!("{:?} ({e})", e.kind()),
        };
        format!(
            "{} bytes were read, then {ended}; writing returned {}",
            self.read, self.wrote
        )
    }

    /// An answer that came whole: this status, and this JSON body with a
    /// `Content-Length` that says so. How the connection ended is not looked at.
    fn assert_answered(&self, status: u16, body: &Value) {
        let what = self.describe();
        assert_eq!(self.status, status, "{what}");
        let sent: Value = serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("the body is no JSON ({e}): {:?}; {what}", self.body));
        assert_eq!(&sent, body, "{what}");
        assert_eq!(
            self.content_length,
            Some(self.body.len()),
            "the body is not as long as its Content-Length says; {what}"
        );
    }

    /// The answer a server gives whole and then ends the connection after:
    /// `assert_answered`, and then the end of the stream.
    fn assert_whole(&self, status: u16, body: &Value) {
        self.assert_answered(status, body);
        assert!(
            self.ended_in.is_none(),
            "the connection did not end with an end of stream; {}",
            self.describe()
        );
    }
}

/// A request whose body comes after the answer, as it does from a client that
/// writes the head and the body apart (CPython's `http.client` does): the head
/// at once, and the body once the answer has begun to arrive and some time has
/// passed. Then everything the connection gives is read.
async fn send_with_a_late_body(
    addr: SocketAddr,
    method: &str,
    path: &str,
    key: Option<&str>,
    body: &str,
) -> Reply {
    let mut stream = begin_a_request(addr, method, path, key, body.len()).await;
    // These 200 ms are not what makes the test right: whenever the body comes,
    // the answer must be whole and the connection must end with its end of
    // stream, and a server that reads the body away makes it so. They are what
    // lets a server that does not do that tear the connection down first, so
    // that this test fails on it.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let wrote = stream.write_all(body.as_bytes()).await;
    let (raw, ended_in) = read_to_the_end(&mut stream).await;
    Reply::parse(&raw, ended_in, format!("{wrote:?}"))
}

/// The endpoints that do not read their body (a stop, a DNS flush and a reload)
/// answer on the head alone, and the client that sends the body late still gets
/// the answer.
#[tokio::test]
async fn a_late_body_does_not_cost_a_stop_its_answer() {
    let api = api().await;
    let reply = send_with_a_late_body(api.addr, "POST", "/v1/stop", Some(KEY), "{}").await;
    reply.assert_whole(200, &json!({}));
    wait_until(async || api.control.stops.load(Ordering::SeqCst) == 1).await;
}

async fn is_in_the_dns_cache(api: &Api, name: &str) -> bool {
    get(api, "/v1/dns").await.1["dnsCache"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["domain"] == json!(name))
}

#[tokio::test]
async fn a_late_body_does_not_cost_a_dns_flush_its_answer() {
    let api = api().await;
    api.engine
        .resolver()
        .lookup("cached.test", rurge_dns::LookupOpts::default())
        .await
        .unwrap();
    assert!(is_in_the_dns_cache(&api, "cached.test").await);
    let reply = send_with_a_late_body(api.addr, "POST", "/v1/dns/flush", Some(KEY), "{}").await;
    reply.assert_whole(200, &json!({}));
    assert!(
        !is_in_the_dns_cache(&api, "cached.test").await,
        "the flush was done"
    );
}

#[tokio::test]
async fn a_late_body_does_not_cost_a_reload_its_answer() {
    let api = api().await;
    let reply =
        send_with_a_late_body(api.addr, "POST", "/v1/profiles/reload", Some(KEY), "{}").await;
    reply.assert_whole(
        200,
        &json!({ "ok": true, "errors": 0, "warnings": 1, "listenersRebound": false }),
    );
    assert_eq!(api.control.reloads.load(Ordering::SeqCst), 1);
}

/// The profile check reads its body, as the log level endpoint does: a body
/// that comes after the head is waited for, and once it has come the check is
/// answered, the refusal of a name and the refusal of a body alike. Nothing is
/// answered on the head alone, so there is no answer to lose.
#[tokio::test]
async fn a_late_body_does_not_cost_a_profile_check_its_answer() {
    let api = api().await;
    for (body, status) in [
        ("{}", 200),
        (r#"{"name":"S3CR3T-profile"}"#, 404),
        ("[]", 400),
    ] {
        let mut stream = send_a_head(api.addr, "POST", CHECK, Some(KEY), body.len()).await;
        // The wait is one in which nothing happens, and that is what is
        // asserted: an answer cannot come before the body does, and 200 ms is
        // what a loopback leaves to an endpoint that would answer on the head.
        assert_eq!(
            peek_at_the_answer(&stream, Duration::from_millis(200)).await,
            None,
            "{body}: answered before its body came"
        );
        stream.write_all(body.as_bytes()).await.unwrap();
        let (late, text) = read_one_answer(&mut stream).await;
        // the answer the same body is given when it comes with the head
        let (whole, answer) = post(&api, CHECK, serde_json::from_str(body).unwrap()).await;
        assert_eq!((late, whole), (status, status), "{body}: {text}");
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            answer,
            "{body}"
        );
    }
}

/// A request refused before its body is read is answered on the head alone too.
#[tokio::test]
async fn a_late_body_does_not_cost_a_wrong_key_its_401() {
    let api = api().await;
    let reply = send_with_a_late_body(api.addr, "POST", "/v1/dns/flush", Some("wrong"), "{}").await;
    reply.assert_whole(401, &json!({ "error": "unauthorized" }));
}

#[tokio::test]
async fn a_late_body_does_not_cost_a_banned_source_its_403() {
    let api = api().await;
    for _ in 0..BAN_FAILURES {
        let (status, _) = call(api.addr, "GET", "/v1/outbound", Some("wrong"), None).await;
        assert_eq!(status, 401);
    }
    let reply = send_with_a_late_body(api.addr, "POST", "/v1/dns/flush", Some(KEY), "{}").await;
    reply.assert_whole(403, &json!({ "error": "banned" }));
}

#[tokio::test]
async fn a_late_body_does_not_cost_an_unknown_path_its_404() {
    let api = api().await;
    let reply = send_with_a_late_body(api.addr, "POST", "/v1/nope", Some(KEY), "{}").await;
    reply.assert_whole(404, &json!({ "error": "no such endpoint" }));
}

/// The key is checked before the route, so without it these two would both be
/// 401: this has the right key.
#[tokio::test]
async fn a_late_body_does_not_cost_a_wrong_method_its_405() {
    let api = api().await;
    let reply = send_with_a_late_body(api.addr, "DELETE", "/v1/outbound", Some(KEY), "{}").await;
    reply.assert_whole(405, &json!({ "error": "method not allowed" }));
}

/// An endpoint that reads its body waits for it, and was right before: there
/// is no answer to wait for after the head, so the body is sent after 200 ms
/// in which nothing happens, which is what is asserted.
#[tokio::test]
async fn an_endpoint_that_reads_its_body_gets_a_late_body_and_answers() {
    let api = api().await;
    let body = r#"{"level":"debug"}"#;
    let mut stream = send_a_head(api.addr, "POST", "/v1/log/level", Some(KEY), body.len()).await;
    assert_eq!(
        peek_at_the_answer(&stream, Duration::from_millis(200)).await,
        None,
        "answered before its body came"
    );
    stream.write_all(body.as_bytes()).await.unwrap();
    // The request was read whole, so the connection stays open for the next
    // one: the answer is read by its length.
    let (status, text) = read_one_answer(&mut stream).await;
    assert_eq!((status, text.as_str()), (200, "{}"));
    assert_eq!(*api.control.levels.lock().unwrap(), vec![LogLevel::Debug]);
}

/// A request that is its head and nothing else, in one write, is answered and
/// closed as it always was.
#[tokio::test]
async fn a_request_without_a_body_is_answered_and_closed_as_before() {
    let api = api().await;
    let mut stream = TcpStream::connect(api.addr).await.unwrap();
    let request = format!(
        "GET /v1/outbound HTTP/1.1\r\nHost: {}\r\nX-Key: {KEY}\r\nConnection: close\r\n\r\n",
        api.addr
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let (raw, ended_in) = read_to_the_end(&mut stream).await;
    Reply::parse(
        &raw,
        ended_in,
        "nothing was written after the request".into(),
    )
    .assert_whole(200, &json!({ "mode": "rule" }));
}

/// A late body of many reads, below the 256 KiB that are read away, is read
/// away like a short one. This does not pin how much is read away: a system
/// takes more than was read without a reset, and how much more is the
/// system's (a server that stopped at 64 KiB of these 200 was not found out
/// on Windows 11). The unit tests of the lingering pin that to the byte.
#[tokio::test]
async fn a_late_body_of_many_reads_does_not_cost_its_answer() {
    let api = api().await;
    let body = " ".repeat(200 * 1024);
    let reply = send_with_a_late_body(api.addr, "POST", "/v1/dns/flush", Some(KEY), &body).await;
    reply.assert_whole(200, &json!({}));
}

/// A body larger than what is read away, from a client that reads while it
/// sends: it has its answer, and its sending ends, taken or refused. The
/// answer is in this side's receive buffer before the first byte of the body
/// leaves, so this was so before the API closed in stages too; what is pinned
/// is that reading away does not hold such a sender up. How the connection
/// ends is not asserted.
///
/// The other order is not tested, because it is not the API's to decide: a
/// client that sends more than is read away before it has read its answer can
/// still lose the answer. The server closes with the rest unread, the system
/// answers what comes after with a reset, and whether an answer that was
/// received and not yet read survives a reset is the client's system's doing
/// (megabytes sent before a late read lose it on Windows).
#[tokio::test]
async fn a_body_larger_than_what_is_read_away_does_not_hold_its_sender_up() {
    let api = api().await;
    let garbage = vec![b'x'; 300 * 1024];
    let stream = begin_a_request(api.addr, "POST", "/v1/dns/flush", Some(KEY), garbage.len()).await;
    let (mut reading, mut writing) = stream.into_split();
    let (read, wrote) = tokio::join!(
        read_to_the_end(&mut reading),
        tokio::time::timeout(Duration::from_secs(10), writing.write_all(&garbage)),
    );
    let wrote = wrote.expect("the 300 KiB were neither taken nor refused within 10 s");
    Reply::parse(&read.0, read.1, format!("{wrote:?}")).assert_answered(200, &json!({}));
}

/// Reads one answer off a connection that stays open: the head, and then as
/// many bytes of body as its `Content-Length` says.
async fn read_one_answer(stream: &mut (impl AsyncRead + Unpin)) -> (u16, String) {
    let mut raw = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk))
            .await
            .expect("no answer within 5 s")
            .unwrap();
        assert_ne!(n, 0, "the connection ended inside an answer: {raw:?}");
        raw.extend_from_slice(&chunk[..n]);
        let reply = Reply::parse(&raw, None, String::new());
        if raw.windows(4).any(|w| w == b"\r\n\r\n")
            && reply.content_length == Some(reply.body.len())
        {
            return (reply.status, reply.body);
        }
    }
}

/// What the lingering time of the crate is: a client that does not close its
/// connection holds a stopping server this long.
const LINGER: Duration = Duration::from_secs(1);

/// A client that has had an answer and keeps its connection open: the server
/// and the client, and the token that stops the server.
async fn a_client_that_keeps_its_connection(
    api: &Api,
) -> (tokio::task::JoinHandle<()>, TcpStream, CancellationToken) {
    let (addr, server, stop) = api.serve_apart().await;
    let server = tokio::spawn(server);
    let mut client = TcpStream::connect(addr).await.unwrap();
    let request = format!("GET /v1/outbound HTTP/1.1\r\nHost: {addr}\r\nX-Key: {KEY}\r\n\r\n");
    client.write_all(request.as_bytes()).await.unwrap();
    let (status, body) = read_one_answer(&mut client).await;
    assert_eq!((status, body.as_str()), (200, r#"{"mode":"rule"}"#));
    (server, client, stop)
}

/// When the server is told to stop it closes the connections that are idle,
/// and does not finish until they have been read away: a client that closes
/// ends the wait at once.
#[tokio::test]
async fn serve_waits_for_a_closing_connection_until_the_client_closes_it() {
    let api = api().await;
    let (server, mut client, stop) = a_client_that_keeps_its_connection(&api).await;
    let stopped = Instant::now();
    stop.cancel();
    // the server closed its side: the end of the stream
    let (rest, ended_in) = read_to_the_end(&mut client).await;
    assert!(
        rest.is_empty() && ended_in.is_none(),
        "{rest:?} {ended_in:?}"
    );
    // Nothing happens here, and that is what is asserted: the client has not
    // closed, so the server is not finished. 100 ms is a tenth of the time it
    // would wait for a client that never closes, and the server had finished
    // at once before it waited at all.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !server.is_finished(),
        "the server finished with a connection open"
    );
    drop(client);
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("the server did not finish after the client closed")
        .unwrap();
    // It was the close that ended the wait, not the end of the lingering time:
    // that time begins after the server was told to stop, so a server that
    // waited it out cannot have finished before this.
    assert!(
        stopped.elapsed() < LINGER,
        "the server finished {:?} after it was told to stop",
        stopped.elapsed()
    );
}

/// A client that never closes holds a stopping server for the lingering time
/// and no longer.
#[tokio::test]
async fn serve_stops_waiting_for_a_connection_that_never_closes_when_the_lingering_time_is_up() {
    let api = api().await;
    let (server, client, stop) = a_client_that_keeps_its_connection(&api).await;
    let stopped = Instant::now();
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("the server did not finish within 10 s")
        .unwrap();
    let waited = stopped.elapsed();
    assert!(
        waited >= LINGER.mul_f64(0.9) && waited <= LINGER * 3,
        "the server finished {waited:?} after it was told to stop"
    );
    drop(client);
}
