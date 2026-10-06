//! rurge's `trust-tunnel` and `h2-connect` outbounds against the TrustTunnel
//! endpoint v1.1.0 (Linux and macOS only), phase 2 M6 design 5.5: its TCP
//! mode is a standard HTTP/2 CONNECT with Basic authentication, so it serves
//! `h2-connect`'s plain CONNECT too (sing-box 1.14.2's `http` inbound speaks
//! HTTP/1 only). Over TCP only: no reference server for CONNECT-UDP. Every
//! target is a loopback IP literal; the endpoint picks its host by SNI, so
//! every line sends `sni=tt.test` and trusts the fixture's CA. With `h3=true`
//! a `trust-tunnel` goes over HTTP/3 instead (phase 2 M7 design 7.2), to an
//! endpoint that listens with QUIC alone (`[listen_protocols.quic]`).

mod common;

use common::*;
use rurge_interop::trusttunnel::{HOSTNAME, Listen, TrustTunnel, trusttunnel_or_skip};
use std::time::Duration;

const USER: &str = "alice";
const PASSWORD: &str = "s3cret-tt";

/// The endpoint serving `listen`, with a certificate for its host, and the
/// fixture whose CA signed it.
fn endpoint(bin: &Path, dir: &Path, listen: Listen) -> (TrustTunnel, Arc<TlsFixture>) {
    let fixture = TlsFixture::new(&[HOSTNAME]);
    let server = TrustTunnel::spawn(bin, dir, listen, &leaf_files(&fixture, dir), USER, PASSWORD);
    (server, fixture)
}

/// `name = <kind>, 127.0.0.1, <port>, <params>, sni=tt.test`.
fn line(name: &str, kind: &str, port: u16, params: &str) -> String {
    format!("{name} = {kind}, 127.0.0.1, {port}, {params}, sni={HOSTNAME}\n")
}

fn profile(lines: &[String]) -> String {
    format!("[Proxy]\n{}[Rule]\nFINAL,DIRECT\n", lines.concat())
}

/// `count` tunnels opened at the same time and all held open, each echoing
/// its own bytes: more than `max-streams` of them share the pool's
/// connections. Bounded like `roundtrip`.
async fn tunnels_at_once(out: &OutboundRef, echo: SocketAddr, count: u8) {
    let bound = Duration::from_secs(10);
    let mut opening = tokio::task::JoinSet::new();
    for n in 0..count {
        let out = out.clone();
        opening.spawn(async move {
            let stream = tokio::time::timeout(
                bound,
                out.connect_tcp(&target(echo), &ConnectOpts::default()),
            )
            .await
            .expect("the tunnel is established within the bound")
            .expect("the tunnel is established");
            (n, stream)
        });
    }
    let mut tunnels = Vec::new();
    while let Some(opened) = opening.join_next().await {
        tunnels.push(opened.expect("the opening task completes"));
    }
    // every tunnel is open at once; each carries its own bytes
    for (n, stream) in &mut tunnels {
        let payload = vec![*n; 4096];
        let mut back = vec![0u8; payload.len()];
        let exchange = async {
            stream.write_all(&payload).await.unwrap();
            stream.read_exact(&mut back).await.unwrap();
        };
        tokio::time::timeout(bound, exchange)
            .await
            .expect("the echo comes back within the bound");
        assert!(back == payload, "tunnel {n} carried another tunnel's bytes");
    }
}

#[tokio::test]
async fn trust_tunnel_against_the_endpoint() {
    let Some(bin) = trusttunnel_or_skip("trust_tunnel_against_the_endpoint") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path(), Listen::Http2);
    let profile = profile(&[line(
        "TT",
        "trust-tunnel",
        server.port(),
        &format!("username={USER}, password={PASSWORD}"),
    )]);
    let (out, echo) = (
        outbound(&profile, "TT", Some(&fixture)),
        echo_server().await,
    );
    roundtrip(&out, echo).await;
    roundtrip_big(&out, echo).await;
}

#[tokio::test]
async fn a_wrong_trust_tunnel_password_is_refused() {
    let Some(bin) = trusttunnel_or_skip("a_wrong_trust_tunnel_password_is_refused") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path(), Listen::Http2);
    let profile = profile(&[line(
        "Wrong",
        "trust-tunnel",
        server.port(),
        &format!("username={USER}, password=nope"),
    )]);
    let out = outbound(&profile, "Wrong", Some(&fixture));
    let refused = tokio::time::timeout(
        Duration::from_secs(10),
        out.connect_tcp(&target(echo_server().await), &ConnectOpts::default()),
    )
    .await
    .expect("the endpoint answers within the bound");
    let Err(err) = refused else {
        panic!("a wrong password must not open a tunnel");
    };
    // the endpoint's `auth_failure_status_code` is 407 by default
    assert_eq!(err.to_string(), "trust-tunnel: authentication failed");
}

#[tokio::test]
async fn h2_connect_against_the_endpoint() {
    let Some(bin) = trusttunnel_or_skip("h2_connect_against_the_endpoint") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path(), Listen::Http2);
    let profile = profile(&[
        // the credentials positionally after the port, and named
        line(
            "Positional",
            "h2-connect",
            server.port(),
            &format!("{USER}, {PASSWORD}"),
        ),
        line(
            "Named",
            "h2-connect",
            server.port(),
            &format!("username={USER}, password={PASSWORD}"),
        ),
    ]);
    let echo = echo_server().await;
    for name in ["Positional", "Named"] {
        let out = outbound(&profile, name, Some(&fixture));
        roundtrip(&out, echo).await;
        roundtrip_big(&out, echo).await;
    }
}

#[tokio::test]
async fn h2_connect_streams_share_connections_on_the_endpoint() {
    let Some(bin) = trusttunnel_or_skip("h2_connect_streams_share_connections_on_the_endpoint")
    else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path(), Listen::Http2);
    let profile = profile(&[line(
        "Shared",
        "h2-connect",
        server.port(),
        &format!("{USER}, {PASSWORD}, max-streams=3"),
    )]);
    let (out, echo) = (
        outbound(&profile, "Shared", Some(&fixture)),
        echo_server().await,
    );
    tunnels_at_once(&out, echo, 7).await;
    // the pool still serves new tunnels after they are gone
    roundtrip(&out, echo).await;
}

/// `h3=true` against an endpoint that listens with QUIC alone: one chunk and
/// many, each tunnel a classic CONNECT on the policy's QUIC connection.
#[tokio::test]
async fn trust_tunnel_over_http3_against_the_endpoint() {
    let Some(bin) = trusttunnel_or_skip("trust_tunnel_over_http3_against_the_endpoint") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path(), Listen::Http3);
    let profile = profile(&[line(
        "TT3",
        "trust-tunnel",
        server.port(),
        &format!("username={USER}, password={PASSWORD}, h3=true"),
    )]);
    let (out, echo) = (
        outbound(&profile, "TT3", Some(&fixture)),
        echo_server().await,
    );
    roundtrip(&out, echo).await;
    roundtrip_big(&out, echo).await;
}

/// Refused over HTTP/3 as over HTTP/2: the endpoint answers 407.
#[tokio::test]
async fn a_wrong_trust_tunnel_password_over_http3_is_refused() {
    let Some(bin) = trusttunnel_or_skip("a_wrong_trust_tunnel_password_over_http3_is_refused")
    else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path(), Listen::Http3);
    let profile = profile(&[line(
        "Wrong",
        "trust-tunnel",
        server.port(),
        &format!("username={USER}, password=nope, h3=true"),
    )]);
    let out = outbound(&profile, "Wrong", Some(&fixture));
    let refused = tokio::time::timeout(
        Duration::from_secs(10),
        out.connect_tcp(&target(echo_server().await), &ConnectOpts::default()),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "the endpoint answers within the bound\n{}",
            server.log_text()
        )
    });
    let Err(err) = refused else {
        panic!(
            "a wrong password must not open a tunnel\n{}",
            server.log_text()
        );
    };
    assert_eq!(
        err.to_string(),
        "trust-tunnel: authentication failed",
        "{}",
        server.log_text()
    );
}
