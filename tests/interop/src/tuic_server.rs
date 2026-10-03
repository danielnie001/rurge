//! The official TUIC server, tuic 0.8.5, as a child process on the loopback:
//! the reference for `tuic` — TUIC v4, which sing-box does not speak (phase
//! 2 M7 design 6.4, M7-D3, M7-D6). It is GPL-licensed and only ever runs as
//! a process of its own; nothing of it is linked or copied. CI installs it
//! on Linux only: elsewhere a missing binary is always a skip, even with
//! `RURGE_INTEROP_REQUIRED=1`. Only `RURGE_TEST_TUIC_SERVER` names it, never
//! `PATH`: a `tuic-server` found there is likely a 1.x one, which speaks
//! TUIC v5 only and reads another configuration.
//!
//! The same rules as for sing-box apply: nothing is downloaded or installed
//! here, and the rendered configuration listens on 127.0.0.1 only — but for
//! the socket the server relays UDP through, which 0.8.5 always binds to a
//! free port on every address (`[::]:0`), with no setting for it. It has no
//! TCP port to probe: it is up once its log says so.

use crate::{REQUIRED_ENV, Reference, TlsFiles, free_udp_port};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BINARY_ENV: &str = "RURGE_TEST_TUIC_SERVER";

/// What the server logs (at `info`) once its socket is bound.
const STARTED: &str = "Server started";

/// `RURGE_TEST_TUIC_SERVER`, and nothing else.
pub fn locate() -> Option<PathBuf> {
    std::env::var_os(BINARY_ENV)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure on Linux,
/// the only platform CI installs it on.
pub fn tuic_server_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if cfg!(target_os = "linux") && std::env::var(REQUIRED_ENV).as_deref() == Ok("1") {
        panic!("{REQUIRED_ENV}=1 but {BINARY_ENV} names no tuic-server");
    }
    eprintln!(
        "skipping {test}: no tuic-server 0.8.5 ({BINARY_ENV}; installed on Linux CI only); see tests/interop/README.md"
    );
    None
}

/// The whole configuration: one server on `127.0.0.1:port` letting `token`
/// in, with the certificate and key of `tls` (PEM paths) and ALPN `h3` — by
/// default it selects none, and a QUIC client that offered one then fails
/// the handshake (phase 2 M7 design 3.2) —, logging at `info`, which says
/// when it is up. The server refuses a key it does not know, so nothing
/// else is written; its other defaults stay (Cubic, a 15 s idle timeout, a
/// 1 s authentication timeout, at most 1500 bytes taken from a target in a
/// UDP packet).
pub fn render(port: u16, token: &str, tls: &TlsFiles) -> Value {
    assert!(
        tls.client_ca.is_none(),
        "tuic-server asks for no client certificate"
    );
    json!({
        "port": port,
        "token": [token],
        "certificate": tls.certificate,
        "private_key": tls.key,
        "ip": "127.0.0.1",
        "alpn": ["h3"],
        "log_level": "info",
    })
}

/// A running tuic-server; killed and reaped on drop.
pub struct TuicServer {
    server: Reference,
    port: u16,
}

impl TuicServer {
    /// Writes the configuration into `dir`, starts `binary` there and waits
    /// until it says it listens.
    pub fn spawn(binary: &Path, dir: &Path, token: &str, tls: &TlsFiles) -> TuicServer {
        let port = free_udp_port();
        let config = dir.join("tuic-server.json");
        std::fs::write(&config, render(port, token, tls).to_string()).expect("write the config");
        let mut command = Command::new(binary);
        command.arg("-c").arg(&config).current_dir(dir);
        let mut server = Reference::start(
            "tuic-server",
            command,
            Vec::new(),
            dir.join("tuic-server.log"),
        );
        server.wait_for_output(STARTED);
        TuicServer { server, port }
    }

    /// The loopback UDP port.
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn log_text(&self) -> String {
        self.server.log_text()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> TlsFiles {
        TlsFiles {
            certificate: "leaf.pem".into(),
            key: "leaf.key".into(),
            client_ca: None,
        }
    }

    #[test]
    fn the_configuration_stays_on_the_loopback() {
        let config = render(4001, "t0ken", &files());
        let text = config.to_string();
        for forbidden in ["0.0.0.0", "::"] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
        }
        assert_eq!(config["ip"], "127.0.0.1");
    }

    /// Every key is one 0.8.5 reads: it refuses any other.
    #[test]
    fn the_configuration_is_written_as_tuic_server_reads_it() {
        assert_eq!(
            render(4001, "t0ken", &files()),
            json!({
                "port": 4001,
                "token": ["t0ken"],
                "certificate": "leaf.pem",
                "private_key": "leaf.key",
                "ip": "127.0.0.1",
                "alpn": ["h3"],
                "log_level": "info",
            })
        );
    }
}
