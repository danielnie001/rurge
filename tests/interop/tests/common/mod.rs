//! What the test files of this directory share: the harness, the helpers and
//! the imports (re-exported, so a test file needs `use common::*` and nothing
//! else). Each test file is a crate of its own and uses a part of all this
//! only: hence the two `allow`s.

#![allow(dead_code, unused_imports)]

pub use rurge_config::config::{LoadOptions, from_text};
pub use rurge_engine::EngineFactory;
pub use rurge_interop::{Inbound, InboundKind, SingBox, TlsFiles, sing_box_or_skip};
pub use rurge_net::connector::{ConnectOpts, SystemResolve, Target};
pub use rurge_net::socket::NoopSocketHook;
pub use rurge_net::testing::TestServer;
pub use rurge_policy::OutboundFactory;
pub use rurge_proto::testing::{TlsFixture, echo_server};
pub use rurge_proto::{OutboundError, OutboundRef};
pub use std::net::SocketAddr;
pub use std::path::Path;
pub use std::sync::Arc;
pub use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The outbound of policy `name` in `profile`, built the way the engine
/// builds it; `fixture`'s CA is trusted when given.
pub fn outbound(profile: &str, name: &str, fixture: Option<&Arc<TlsFixture>>) -> OutboundRef {
    let loaded = from_text(
        profile,
        Path::new("interop.conf"),
        &LoadOptions::for_tests(),
    );
    assert!(
        !loaded.diagnostics.has_errors(),
        "{:?}",
        loaded
            .diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
    );
    let cfg = loaded.config;
    let factory = match fixture {
        Some(f) => EngineFactory::with_roots(
            &cfg,
            Arc::new(SystemResolve),
            Arc::new(NoopSocketHook),
            f.roots(),
        ),
        None => EngineFactory::new(&cfg, Arc::new(SystemResolve), Arc::new(NoopSocketHook)),
    };
    let spec = cfg.spec(name).expect("the policy has a spec");
    factory
        .build(spec, factory.direct_connector(&spec.common))
        .expect("the policy builds")
}

pub fn target(addr: SocketAddr) -> Target {
    Target::new(rurge_config::HostName::Ip(addr.ip()), addr.port())
}

/// Every step is bounded: nobody runs these locally, and on CI an unbounded
/// read turns a regression into a hung job instead of a failed one.
pub async fn roundtrip(out: &OutboundRef, echo: SocketAddr) {
    let bound = std::time::Duration::from_secs(10);
    let mut stream = tokio::time::timeout(
        bound,
        out.connect_tcp(&target(echo), &ConnectOpts::default()),
    )
    .await
    .expect("the tunnel is established within the bound")
    .expect("the tunnel is established");
    tokio::time::timeout(bound, stream.write_all(b"interop"))
        .await
        .expect("the write reaches sing-box within the bound")
        .unwrap();
    let mut buf = [0u8; 7];
    tokio::time::timeout(bound, stream.read_exact(&mut buf))
        .await
        .expect("the echo comes back within the bound")
        .unwrap();
    assert_eq!(&buf, b"interop");
}

pub fn plain(kind: InboundKind, users: &[(&str, &str)]) -> Inbound {
    Inbound {
        kind,
        users: users
            .iter()
            .map(|(u, p)| (u.to_string(), p.to_string()))
            .collect(),
        tls: None,
        ws_path: None,
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The fixture's leaf certificate and key as PEM files in `dir`.
pub fn leaf_files(fixture: &TlsFixture, dir: &Path) -> TlsFiles {
    let write = |name: &str, text: String| {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    };
    TlsFiles {
        certificate: write("leaf.pem", fixture.leaf_pem()),
        key: write("leaf.key", fixture.leaf_key_pem()),
        client_ca: None,
    }
}

pub fn trojan_inbound(tls: TlsFiles, ws_path: Option<&str>) -> Inbound {
    Inbound {
        kind: InboundKind::Trojan,
        users: vec![("u".into(), "s3same".into())],
        tls: Some(tls),
        ws_path: ws_path.map(str::to_string),
    }
}

pub const VMESS_ID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";

pub fn vmess_inbound(tls: Option<TlsFiles>, ws_path: Option<&str>) -> Inbound {
    Inbound {
        kind: InboundKind::Vmess,
        users: vec![("u".into(), VMESS_ID.into())],
        tls,
        ws_path: ws_path.map(str::to_string),
    }
}

/// More than one chunk each way: the length masks and the nonce counter have
/// to stay in step. Bounded like `roundtrip`.
pub async fn roundtrip_big(out: &OutboundRef, echo: SocketAddr) {
    let bound = std::time::Duration::from_secs(20);
    let payload: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    let stream = tokio::time::timeout(
        bound,
        out.connect_tcp(&target(echo), &ConnectOpts::default()),
    )
    .await
    .expect("the tunnel is established within the bound")
    .expect("the tunnel is established");
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut back = vec![0u8; payload.len()];
    let both = async {
        tokio::join!(async { writer.write_all(&payload).await.unwrap() }, async {
            reader.read_exact(&mut back).await.unwrap()
        })
    };
    tokio::time::timeout(bound, both)
        .await
        .expect("100 000 bytes make it there and back within the bound");
    assert!(back == payload, "the echo differs");
}
