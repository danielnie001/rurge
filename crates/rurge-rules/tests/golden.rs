//! Golden tests: each case in `tests/golden/*.toml` builds a profile from its
//! rules, evaluates one session and compares policy / reason / sub-rule.

use rurge_config::HostName;
use rurge_config::config::{LoadOptions, from_text};
use rurge_config::rule::ProtocolKind;
use rurge_config::session::SessionInfo;
use rurge_net::connector::{DirectConnector, SystemResolve};
use rurge_net::http::{HttpClient, HttpClientConfig};
use rurge_net::resource::{ResourceManager, ResourceOptions};
use rurge_rules::engine::{FixedResolve, LazyResolver, NoResolve};
use rurge_rules::matcher::{NoGeo, ResolvedAddrs};
use rurge_rules::{OutboundMode, RuleEngine, SetRegistry};
use serde::Deserialize;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

#[derive(Deserialize)]
struct File {
    case: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    rules: Vec<String>,
    #[serde(default)]
    rulesets: Vec<String>,
    #[serde(rename = "final", default = "default_final")]
    final_rule: String,
    host: String,
    #[serde(default = "default_port")]
    port: u16,
    sni: Option<String>,
    user_agent: Option<String>,
    protocol: Option<String>,
    #[serde(default)]
    resolve: Vec<IpAddr>,
    #[serde(default)]
    no_dns: bool,
    policy: String,
    reason: Option<String>,
    resolved: Option<bool>,
    sub_rule: Option<String>,
}

fn default_final() -> String {
    "FINAL,DIRECT".to_string()
}

fn default_port() -> u16 {
    443
}

fn profile(case: &Case) -> String {
    let mut text = String::from("[Proxy]\nP = direct\n[Rule]\n");
    for r in &case.rules {
        text.push_str(r);
        text.push('\n');
    }
    text.push_str(&case.final_rule);
    text.push('\n');
    for rs in &case.rulesets {
        text.push_str(rs);
        text.push('\n');
    }
    text
}

async fn run_case(case: &Case, dir: &Path) {
    let loaded = from_text(
        &profile(case),
        &dir.join("golden.conf"),
        &LoadOptions::for_tests(),
    );
    let codes: Vec<&str> = loaded.diagnostics.iter().map(|d| d.code).collect();
    assert!(
        !loaded.diagnostics.has_errors(),
        "{}: profile errors {codes:?}",
        case.name
    );
    let cfg = loaded.config;
    let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
    let client = Arc::new(HttpClient::new(connector, HttpClientConfig::default()).unwrap());
    let resources = ResourceManager::with_options(
        dir.to_path_buf(),
        client,
        ResourceOptions {
            offline: true,
            ..ResourceOptions::default()
        },
    );
    let (registry, _) = SetRegistry::build(&cfg, resources, dir);
    let engine = RuleEngine::build(&cfg, registry.as_ref(), Arc::new(NoGeo)).unwrap();
    let mut session = SessionInfo::tcp(HostName::parse(&case.host), case.port);
    session.sni = case.sni.clone();
    session.user_agent = case.user_agent.clone();
    session.protocol = case
        .protocol
        .as_deref()
        .map(|p| ProtocolKind::parse(p).expect("protocol"));
    let resolver: Box<dyn LazyResolver> = if case.no_dns {
        Box::new(NoResolve)
    } else {
        let mut addrs = ResolvedAddrs::default();
        for ip in &case.resolve {
            match ip {
                IpAddr::V4(v) => addrs.v4.push(*v),
                IpAddr::V6(v) => addrs.v6.push(*v),
            }
        }
        Box::new(FixedResolve(addrs))
    };
    let d = engine
        .evaluate(&session, OutboundMode::Rule, resolver.as_ref())
        .await;
    let policy = d
        .policy()
        .map(|p| p.name())
        .unwrap_or_else(|| "(dns failed)".to_string());
    assert_eq!(policy, case.policy, "{}: policy", case.name);
    if let Some(r) = &case.reason {
        assert_eq!(d.reason.as_str(), r, "{}: reason", case.name);
    }
    if let Some(expect) = case.resolved {
        assert_eq!(d.resolved.is_some(), expect, "{}: resolved", case.name);
    }
    if let Some(s) = &case.sub_rule {
        assert_eq!(
            d.sub_rule.as_ref().map(|h| h.entry.as_str()),
            Some(s.as_str()),
            "{}: sub-rule",
            case.name
        );
    }
}

#[tokio::test]
async fn manual_examples() {
    let dir = tempfile::tempdir().unwrap();
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/manual.toml"),
    )
    .unwrap();
    let file: File = toml::from_str(&text).unwrap();
    assert!(file.case.len() >= 20);
    for case in &file.case {
        run_case(case, dir.path()).await;
    }
}
