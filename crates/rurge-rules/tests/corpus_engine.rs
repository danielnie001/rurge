//! Corpus smoke test: every profile in `tests/corpus/valid/*.conf` (repo
//! root) must build a `SetRegistry` and a `RuleEngine`, and evaluating one
//! session against it must produce a decision without panicking. This is a
//! shallow but broad check that the engine tolerates whatever shapes of
//! real-ish profiles the config corpus covers, run offline (no network).

use rurge_config::HostName;
use rurge_config::config::{LoadOptions, load};
use rurge_config::session::SessionInfo;
use rurge_net::connector::{DirectConnector, SystemResolve};
use rurge_net::http::{HttpClient, HttpClientConfig};
use rurge_net::resource::{ResourceManager, ResourceOptions};
use rurge_rules::engine::NoResolve;
use rurge_rules::matcher::NoGeo;
use rurge_rules::{OutboundMode, RuleEngine, SetRegistry};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/valid")
}

#[tokio::test]
async fn every_valid_corpus_profile_builds_and_evaluates() {
    let dir = corpus_dir();
    let cache_root = tempfile::tempdir().unwrap();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read corpus dir {}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("conf"))
        .collect();
    entries.sort();
    assert!(
        !entries.is_empty(),
        "no *.conf files found under {}",
        dir.display()
    );

    for path in entries {
        let loaded = load(&path, &LoadOptions::for_tests())
            .unwrap_or_else(|e| panic!("{}: cannot load: {e}", path.display()));
        let codes: Vec<&str> = loaded.diagnostics.iter().map(|d| d.code).collect();
        assert!(
            !loaded.diagnostics.has_errors(),
            "{}: profile errors {codes:?}",
            path.display()
        );
        let cfg = loaded.config;

        let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
        let client = Arc::new(HttpClient::new(connector, HttpClientConfig::default()).unwrap());
        let resources = ResourceManager::with_options(
            cache_root.path().to_path_buf(),
            client,
            ResourceOptions {
                offline: true,
                ..ResourceOptions::default()
            },
        );
        let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
        let (registry, _diags) = SetRegistry::build(&cfg, resources, base_dir);
        let engine = RuleEngine::build(&cfg, registry.as_ref(), Arc::new(NoGeo))
            .unwrap_or_else(|e| panic!("{}: engine build failed: {e}", path.display()));

        let session = SessionInfo::tcp(HostName::parse("www.example.com"), 443);
        let decision = engine
            .evaluate(&session, OutboundMode::Rule, &NoResolve)
            .await;
        assert!(
            decision.matched.is_some(),
            "{}: evaluate produced no matched rule",
            path.display()
        );
    }
}
