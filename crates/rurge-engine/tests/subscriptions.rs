//! `policy-path` subscriptions through the whole engine (phase 2 M3 design
//! §5): what the registry holds from the first build on.

mod common;
use common::*;

/// A local subscription is read while the generation is built: the group
/// has its members from the first dial on (M3-D5).
#[tokio::test]
async fn a_subscription_file_is_in_from_the_first_generation() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("nodes.txt"),
        "N1 = http, n1.test, 80\nN2 = http, n2.test, 80\n",
    )
    .unwrap();
    let dns = MockDns::spawn().await;
    let text = Profile {
        groups: "Sub = select, policy-path=nodes.txt",
        ..Profile::default()
    }
    .text(dns.addr());
    let engine = Engine::new(runtime(dir.path(), &text, EngineShared::default()).await);
    assert_eq!(engine.registry().members("Sub").unwrap(), ["N1", "N2"]);
}
