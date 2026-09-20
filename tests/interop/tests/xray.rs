//! rurge's `vmess` outbound against a real xray on the loopback: the second
//! reference for the hand-written VMess codec. Every target is a loopback IP
//! literal. Without an xray binary each test prints why it is skipped
//! (`RURGE_INTEROP_REQUIRED=1` turns that into a failure).

use rurge_config::config::{LoadOptions, from_text};
use rurge_engine::EngineFactory;
use rurge_interop::xray::{Xray, XrayInbound, xray_or_skip};
use rurge_net::connector::{ConnectOpts, SystemResolve, Target};
use rurge_net::socket::NoopSocketHook;
use rurge_policy::OutboundFactory;
use rurge_proto::OutboundRef;
use rurge_proto::testing::echo_server;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const VMESS_ID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";

fn outbound(profile: &str, name: &str) -> OutboundRef {
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
    let factory = EngineFactory::new(&cfg, Arc::new(SystemResolve), Arc::new(NoopSocketHook));
    let spec = cfg.spec(name).expect("the policy has a spec");
    factory
        .build(spec, factory.direct_connector(&spec.common))
        .expect("the policy builds")
}

/// Every step is bounded: nobody runs these locally, and on CI an unbounded
/// read turns a regression into a hung job instead of a failed one.
async fn roundtrip(out: &OutboundRef, echo: SocketAddr, payload: &[u8]) {
    let bound = Duration::from_secs(10);
    let target = Target::new(rurge_config::HostName::Ip(echo.ip()), echo.port());
    let stream = tokio::time::timeout(bound, out.connect_tcp(&target, &ConnectOpts::default()))
        .await
        .expect("the tunnel is established within the bound")
        .expect("the tunnel is established");
    // read while writing: an echo of this size does not fit the socket buffers
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut back = vec![0u8; payload.len()];
    let both = async {
        tokio::join!(async { writer.write_all(payload).await.unwrap() }, async {
            reader.read_exact(&mut back).await.unwrap()
        })
    };
    tokio::time::timeout(bound, both)
        .await
        .expect("the payload makes it there and back within the bound");
    assert!(back == payload, "the echo differs");
}

#[tokio::test]
async fn vmess_with_either_cipher_with_and_without_websocket() {
    let Some(bin) = xray_or_skip("vmess_with_either_cipher_with_and_without_websocket") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let xray = Xray::spawn(
        &bin,
        dir.path(),
        vec![
            XrayInbound {
                uuid: VMESS_ID.into(),
                ws_path: None,
            },
            XrayInbound {
                uuid: VMESS_ID.into(),
                ws_path: Some("/v".into()),
            },
        ],
    );
    let echo = echo_server().await;
    let line = |name: &str, index: usize, rest: &str| {
        format!(
            "{name} = vmess, 127.0.0.1, {}, username={VMESS_ID}, vmess-aead=true{rest}\n",
            xray.port(index)
        )
    };
    let profile = format!(
        "[Proxy]\n{}{}{}[Rule]\nFINAL,DIRECT\n",
        line("Plain", 0, ""),
        line("Chacha", 0, ", encrypt-method=chacha20-ietf-poly1305"),
        line("Ws", 1, ", ws=true, ws-path=/v"),
    );
    // more than one chunk each way: the masks and the nonce counter have to stay in step
    let big: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    for name in ["Plain", "Chacha", "Ws"] {
        let out = outbound(&profile, name);
        roundtrip(&out, echo, b"interop").await;
        roundtrip(&out, echo, &big).await;
    }
}
