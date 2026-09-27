//! Sessions that leave through the `ssh` outbound: profile text → Runtime →
//! Engine → loopback listeners → `FakeSsh` → `TestServer` (phase 2 M4
//! design §5).

mod common;

use common::*;
use rurge_config::spec::ShadowTlsVersion;
use rurge_proto::testing::{Camouflage, FakeShadowTls, ShadowTlsScript};

async fn ssh_server(origin: &TestServer, opts: FakeSshOpts) -> FakeSsh {
    FakeSsh::start(FakeSshOpts {
        user: "u".into(),
        connect_to: Some(origin_addr(origin)),
        ..opts
    })
    .await
}

#[tokio::test]
async fn a_connect_leaves_through_ssh_with_the_name_unresolved() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let server = ssh_server(
        &origin,
        FakeSshOpts {
            password: Some("pw".into()),
            ..FakeSshOpts::default()
        },
    )
    .await;
    let h = harness(Profile {
        proxies: &format!(
            "S = ssh, 127.0.0.1, {}, username=u, password=pw",
            server.addr.port()
        ),
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    let response = get(&mut tunnel, "target.test", "/hello").await;
    assert!(response.ends_with("hi there"), "{response}");
    // the SSH server resolves the name: rurge never looked it up
    assert_eq!(server.requested(), [("target.test".to_string(), 8080)]);
    assert!(h.dns.queries().is_empty());
    drop(tunnel);
    let log = h.engine.request_log();
    wait_until("the session to finish", || !log.recent(10).is_empty()).await;
    assert_eq!(log.recent(10)[0].policy, ["S"]);
}

/// A key from `[Keystore]`, and a server pinned by `server-fingerprint`.
#[tokio::test]
async fn a_keystore_key_logs_in_to_a_pinned_server() {
    let origin = TestServer::spawn().await;
    origin.set("/k", "keyed");
    let key = random_key(Algorithm::Ed25519);
    let server = ssh_server(
        &origin,
        FakeSshOpts {
            keys: vec![key.public_key().clone()],
            ..FakeSshOpts::default()
        },
    )
    .await;
    let h = harness(Profile {
        proxies: &format!(
            "S = ssh, 127.0.0.1, {}, username=u, private-key=key1, server-fingerprint=\"{}\"",
            server.addr.port(),
            fingerprint_of(&server.host_key)
        ),
        keystore: &format!(
            "key1 = type=openssh-private-key, base64={}",
            keystore_base64(&key)
        ),
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:80").await;
    let response = get(&mut tunnel, "target.test", "/k").await;
    assert!(response.ends_with("keyed"), "{response}");
    assert_eq!(server.logins(), 1);
}

/// The SSH session runs inside Shadow TLS like any TCP protocol's.
#[tokio::test]
async fn an_ssh_session_runs_inside_shadow_tls() {
    let origin = TestServer::spawn().await;
    origin.set("/st", "wrapped");
    let server = ssh_server(
        &origin,
        FakeSshOpts {
            password: Some("pw".into()),
            ..FakeSshOpts::default()
        },
    )
    .await;
    let fixture = TlsFixture::new(&["site.test"]);
    let site = Camouflage::spawn(&fixture, &[&rustls::version::TLS13], 2).await;
    let front = FakeShadowTls::spawn(ShadowTlsScript::new(
        ShadowTlsVersion::V3,
        "st-pw",
        site.addr(),
        server.addr,
    ))
    .await;
    let h = harness_trusting(
        Profile {
            proxies: &format!(
                "S = ssh, 127.0.0.1, {}, username=u, password=pw, shadow-tls-password=st-pw, shadow-tls-version=3, shadow-tls-sni=site.test",
                front.addr().port()
            ),
            rules: "DOMAIN,target.test,S",
            ..Profile::default()
        },
        fixture.roots(),
    )
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:80").await;
    let response = get(&mut tunnel, "target.test", "/st").await;
    assert!(response.ends_with("wrapped"), "{response}");
    assert!(front.sessions()[0].authenticated);
    assert_eq!(server.logins(), 1);
}

/// A connectivity test goes through the SSH session like any connection.
#[tokio::test]
async fn a_policy_test_goes_through_the_ssh_session() {
    let origin = TestServer::spawn().await;
    let server = ssh_server(
        &origin,
        FakeSshOpts {
            password: Some("pw".into()),
            ..FakeSshOpts::default()
        },
    )
    .await;
    let h = harness(Profile {
        proxies: &format!(
            "S = ssh, 127.0.0.1, {}, username=u, password=pw",
            server.addr.port()
        ),
        ..Profile::default()
    })
    .await;
    let results = h
        .engine
        .test_policies(&["S".to_string()], Some(origin.url("/")))
        .await
        .unwrap();
    let (name, result) = &results[0];
    assert_eq!(name, "S");
    let result = result.as_ref().expect("an ssh policy can be tested");
    assert!(result.outcome.is_ok(), "{:?}", result.outcome);
    assert_eq!(server.requested().len(), 1, "one connection, two HEADs");
}
