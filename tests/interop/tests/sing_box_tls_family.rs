//! The TLS family against sing-box (trojan, vmess, anytls): the same helpers as `sing_box.rs`.

mod common;

use common::*;

#[tokio::test]
async fn trojan_with_and_without_websocket() {
    let Some(bin) = sing_box_or_skip("trojan_with_and_without_websocket") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let sb = SingBox::spawn(
        &bin,
        dir.path(),
        vec![
            trojan_inbound(leaf_files(&fixture, dir.path()), None),
            trojan_inbound(leaf_files(&fixture, dir.path()), Some("/ws")),
        ],
    );
    let echo = echo_server().await;
    let profile = format!(
        "[Proxy]\nPlain = trojan, 127.0.0.1, {}, password=s3same\nWs = trojan, 127.0.0.1, {}, password=s3same, ws=true, ws-path=/ws\n[Rule]\nFINAL,DIRECT\n",
        sb.port(0),
        sb.port(1)
    );
    roundtrip(&outbound(&profile, "Plain", Some(&fixture)), echo).await;
    roundtrip(&outbound(&profile, "Ws", Some(&fixture)), echo).await;
}

#[tokio::test]
async fn a_wrong_trojan_password_is_not_relayed() {
    let Some(bin) = sing_box_or_skip("a_wrong_trojan_password_is_not_relayed") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let sb = SingBox::spawn(
        &bin,
        dir.path(),
        vec![trojan_inbound(leaf_files(&fixture, dir.path()), None)],
    );
    let echo = echo_server().await;
    let profile = format!(
        "[Proxy]\nWrong = trojan, 127.0.0.1, {}, password=nope\n[Rule]\nFINAL,DIRECT\n",
        sb.port(0)
    );
    // the protocol has no reply, so connecting succeeds; nothing comes back
    let mut stream = outbound(&profile, "Wrong", Some(&fixture))
        .connect_tcp(&target(echo), &ConnectOpts::default())
        .await
        .expect("connecting succeeds");
    stream.write_all(b"interop").await.unwrap();
    let mut buf = [0u8; 7];
    let got = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stream.read_exact(&mut buf),
    )
    .await
    .expect("sing-box closes the connection (it has no fallback configured)");
    assert!(
        got.is_err() || &buf != b"interop",
        "the payload must not be echoed"
    );
}

#[tokio::test]
async fn vmess_with_and_without_tls_and_websocket() {
    let Some(bin) = sing_box_or_skip("vmess_with_and_without_tls_and_websocket") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let sb = SingBox::spawn(
        &bin,
        dir.path(),
        vec![
            vmess_inbound(None, None),
            vmess_inbound(None, Some("/v")),
            vmess_inbound(Some(leaf_files(&fixture, dir.path())), None),
            vmess_inbound(Some(leaf_files(&fixture, dir.path())), Some("/v")),
        ],
    );
    let echo = echo_server().await;
    let line = |name: &str, index: usize, rest: &str| {
        format!(
            "{name} = vmess, 127.0.0.1, {}, username={VMESS_ID}, vmess-aead=true{rest}\n",
            sb.port(index)
        )
    };
    let profile = format!(
        "[Proxy]\n{}{}{}{}{}[Rule]\nFINAL,DIRECT\n",
        line("Plain", 0, ""),
        line("Chacha", 0, ", encrypt-method=chacha20-ietf-poly1305"),
        line("Ws", 1, ", ws=true, ws-path=/v"),
        line("Tls", 2, ", tls=true"),
        line("TlsWs", 3, ", tls=true, ws=true, ws-path=/v"),
    );
    for name in ["Plain", "Chacha", "Ws", "Tls", "TlsWs"] {
        let out = outbound(&profile, name, Some(&fixture));
        roundtrip(&out, echo).await;
        roundtrip_big(&out, echo).await;
    }
}

#[tokio::test]
async fn a_wrong_vmess_id_is_not_relayed() {
    let Some(bin) = sing_box_or_skip("a_wrong_vmess_id_is_not_relayed") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(&bin, dir.path(), vec![vmess_inbound(None, None)]);
    let echo = echo_server().await;
    let profile = format!(
        "[Proxy]\nWrong = vmess, 127.0.0.1, {}, username=0233d11c-15a4-47d3-ade3-48ffca0ce118, vmess-aead=true\n[Rule]\nFINAL,DIRECT\n",
        sb.port(0)
    );
    // the server only ever answers a request it accepts: connecting succeeds
    let mut stream = outbound(&profile, "Wrong", None)
        .connect_tcp(&target(echo), &ConnectOpts::default())
        .await
        .expect("connecting succeeds");
    stream.write_all(b"interop").await.unwrap();
    let mut buf = [0u8; 7];
    let got = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        stream.read_exact(&mut buf),
    )
    .await;
    // an error, or silence until the bound: never the echo
    assert!(!matches!(got, Ok(Ok(_))), "a wrong id was relayed");
}

#[tokio::test]
async fn anytls_reuses_its_session_and_can_be_told_not_to() {
    let Some(bin) = sing_box_or_skip("anytls_reuses_its_session_and_can_be_told_not_to") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let sb = SingBox::spawn(
        &bin,
        dir.path(),
        vec![Inbound {
            kind: InboundKind::AnyTls,
            users: vec![("u".into(), "s3same".into())],
            tls: Some(leaf_files(&fixture, dir.path())),
            ws_path: None,
        }],
    );
    let echo = echo_server().await;
    let profile = format!(
        "[Proxy]\nA = anytls, 127.0.0.1, {0}, password=s3same\nOnce = anytls, 127.0.0.1, {0}, password=s3same, reuse=false\n[Rule]\nFINAL,DIRECT\n",
        sb.port(0)
    );
    // the same outbound three times: the second and third stream ride the
    // session the first one left behind
    let reused = outbound(&profile, "A", Some(&fixture));
    for _ in 0..3 {
        roundtrip(&reused, echo).await;
    }
    let once = outbound(&profile, "Once", Some(&fixture));
    for _ in 0..2 {
        roundtrip(&once, echo).await;
    }
}
