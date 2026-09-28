//! The `wireguard` outbound against sing-box's WireGuard endpoint (phase 2
//! M4 design §10; total design Q4): the handshake, TCP through the tunnel,
//! the reserved bytes sing-box writes into its messages to a peer with a
//! `client-id`, and the handshake test.

mod common;

use common::*;
use std::time::Duration;

#[tokio::test]
async fn a_connection_goes_through_a_sing_box_wireguard_endpoint() {
    let Some(bin) = sing_box_or_skip("a_connection_goes_through_a_sing_box_wireguard_endpoint")
    else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (ours, our_public) = keypair();
    let (theirs, their_public) = keypair();
    let (_sb, port) = SingBox::spawn_wireguard(
        &bin,
        dir.path(),
        &WireGuardEndpoint {
            private_key: theirs,
            address: "10.9.0.1/32".to_string(),
            peer_public_key: our_public,
            peer_allowed_ips: vec!["10.9.0.2/32".to_string()],
            reserved: Some([1, 2, 3]),
        },
    );
    let echo = echo_server().await;
    let profile = format!(
        "[Proxy]\nWG = wireguard, section-name=sb\n[Rule]\nFINAL,DIRECT\n\
[WireGuard sb]\nprivate-key = {}\nself-ip = 10.9.0.2\n\
peer = (public-key = {}, allowed-ips = 127.0.0.1/32, endpoint = 127.0.0.1:{port}, client-id = 1/2/3)\n",
        hex(&ours),
        hex(&their_public)
    );
    let out = outbound(&profile, "WG", None);
    roundtrip(&out, echo).await;
    roundtrip_big(&out, echo).await;
    let test = out.native_test().expect("a wireguard policy tests itself");
    tokio::time::timeout(Duration::from_secs(10), test)
        .await
        .expect("sing-box answers the handshake in time")
        .expect("a handshake");
}
