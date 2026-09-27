//! The `ssh` outbound against OpenSSH's `sshd` (phase 2 M4 design §10): with
//! OpenSSH's default algorithms, and with only the two Surge's manual
//! requires. Unix only: a `sshd` that is not run as root logs in only the
//! user running it, with a key.

#![cfg(unix)]

mod common;

use common::*;
use rurge_interop::sshd::{Algorithms, Sshd, sshd_or_skip};
use rurge_proto_ssh::testing::{
    Algorithm, fingerprint_of, keystore_base64, openssh_text, random_key,
};

async fn forward_through(algorithms: Algorithms, test: &str) {
    let Some(binary) = sshd_or_skip(test) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let host = random_key(Algorithm::Ed25519);
    let client = random_key(Algorithm::Ed25519);
    let sshd = Sshd::spawn(
        &binary,
        dir.path(),
        &openssh_text(&host),
        &fingerprint_of(client.public_key()),
        algorithms,
    );
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .expect("USER or LOGNAME names the user sshd logs in");
    let profile = format!(
        "[Proxy]\nS = ssh, 127.0.0.1, {}, username={user}, private-key=key1, server-fingerprint=\"{}\"\n\
[Keystore]\nkey1 = type=openssh-private-key, base64={}\n[Rule]\nFINAL,DIRECT\n",
        sshd.port(),
        fingerprint_of(host.public_key()),
        keystore_base64(&client)
    );
    let out = outbound(&profile, "S", None);
    roundtrip(&out, echo_server().await).await;
}

#[tokio::test]
async fn ssh_forwards_through_openssh() {
    forward_through(Algorithms::Default, "ssh_forwards_through_openssh").await;
}

#[tokio::test]
async fn ssh_reaches_an_openssh_offering_only_surges_algorithms() {
    forward_through(
        Algorithms::SurgeMinimum,
        "ssh_reaches_an_openssh_offering_only_surges_algorithms",
    )
    .await;
}
