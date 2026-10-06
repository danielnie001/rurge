//! masque-server v0.13.0 (Vincent-bin/masque-server, MIT) as a child process
//! on the loopback: the reference for `masque` (phase 2 M7 design 7.4,
//! M7-D6). It is built for Linux only: elsewhere a missing binary is always
//! a skip, even with `RURGE_INTEROP_REQUIRED=1`. The same rules as for
//! sing-box apply: nothing is downloaded or installed here, and the rendered
//! configuration listens on 127.0.0.1 only — one HTTP/3 listener, no
//! observability endpoint, and no CONNECT-IP, which would open a TUN device.
//! It has no TCP port to probe: it is up once its log says so.
//!
//! The server lets nobody in without Basic credentials, checked against
//! Argon2id hashes: its one user is the one its own end-to-end tests use.
//! Its target policy refuses loopback and private addresses unless told
//! otherwise, which the loopback echoes need.

use crate::{REQUIRED_ENV, Reference, TlsFiles, free_udp_port, toml_path, toml_string};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BINARY_ENV: &str = "RURGE_TEST_MASQUE_SERVER";

/// The user the server lets in, and its password — those of masque-server's
/// own end-to-end tests (`tests/e2e/masque-e2e.toml`, `compose.yml`).
pub const USERNAME: &str = "test";
pub const PASSWORD: &str = "test-password";

/// The Argon2id hash of [`PASSWORD`] the configuration holds, from the same
/// file.
const PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$1xNVXhqKU7jJ6cqTBJKphQ$GXXAINVTW1qhloFtN1IR8lSr7pI7QEY79fq4K6d8scQ";

/// What the server logs (at `info`) once every listener is bound.
const READY: &str = "server ready";

/// `RURGE_TEST_MASQUE_SERVER`, else the first `masque-server` on `PATH`.
pub fn locate() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BINARY_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("masque-server"))
        .find(|candidate| candidate.is_file())
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure on Linux,
/// the only platform masque-server is built for.
pub fn masque_server_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if cfg!(target_os = "linux") && std::env::var(REQUIRED_ENV).as_deref() == Ok("1") {
        panic!("{REQUIRED_ENV}=1 but no masque-server was found ({BINARY_ENV} or PATH)");
    }
    eprintln!(
        "skipping {test}: no masque-server ({BINARY_ENV} or PATH; Linux only); see tests/interop/README.md"
    );
    None
}

/// The whole configuration: one HTTP/3 listener of one shard on
/// `127.0.0.1:port` with the certificate and key of `tls` (PEM paths;
/// BoringSSL reads a PKCS#8 key) and Basic authentication of [`USERNAME`];
/// TCP and UDP to any target, the loopback included; CONNECT-IP off. The
/// server refuses a key it does not know, so nothing else is written; its
/// other defaults stay (a 60 s idle timeout, the RFC 9298 default template
/// for CONNECT-UDP, QUIC datagrams on).
pub fn render(port: u16, tls: &TlsFiles) -> String {
    assert!(
        tls.client_ca.is_none(),
        "masque-server's Basic listener asks for no client certificate"
    );
    format!(
        "[tls]\n\
         cert_path = {}\n\
         key_path = {}\n\
         \n\
         [[listeners]]\n\
         listen_addr = \"127.0.0.1:{port}\"\n\
         transport = \"http3\"\n\
         shards = 1\n\
         \n\
         [listeners.auth]\n\
         enabled = true\n\
         mode = \"basic\"\n\
         \n\
         [[listeners.auth.users]]\n\
         username = {}\n\
         password_hash = {}\n\
         \n\
         [tcp_proxy]\n\
         allow_targets = [\"0.0.0.0/0\", \"::/0\"]\n\
         deny_targets = []\n\
         \n\
         [udp_proxy]\n\
         allow_targets = [\"0.0.0.0/0\", \"::/0\"]\n\
         deny_targets = []\n\
         \n\
         [ip_proxy]\n\
         enabled = false\n",
        toml_path(&tls.certificate),
        toml_path(&tls.key),
        toml_string(USERNAME),
        toml_string(PASSWORD_HASH)
    )
}

/// A running masque-server; killed and reaped on drop.
pub struct MasqueServer {
    server: Reference,
    port: u16,
}

impl MasqueServer {
    /// Writes the configuration into `dir`, starts `binary` there and waits
    /// until it says it is ready.
    pub fn spawn(binary: &Path, dir: &Path, tls: &TlsFiles) -> MasqueServer {
        let port = free_udp_port();
        let config = dir.join("masque.toml");
        std::fs::write(&config, render(port, tls)).expect("write the config");
        let mut command = Command::new(binary);
        command.arg("--config").arg(&config).current_dir(dir);
        // its own filter logs `server ready`; another in the environment
        // might not
        command.env_remove("RUST_LOG");
        let mut server = Reference::start(
            "masque-server",
            command,
            Vec::new(),
            dir.join("masque-server.log"),
        );
        server.wait_for_output(READY);
        MasqueServer { server, port }
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
            certificate: "/tmp/masque/leaf.pem".into(),
            key: "/tmp/masque/leaf.key".into(),
            client_ca: None,
        }
    }

    /// One listener, on the loopback; no endpoint or device beyond it.
    #[test]
    fn the_configuration_stays_on_the_loopback() {
        let text = render(4001, &files());
        let addresses: Vec<&str> = text.lines().filter(|l| l.contains("listen_addr")).collect();
        assert_eq!(addresses, ["listen_addr = \"127.0.0.1:4001\""]);
        assert_eq!(text.matches("[[listeners]]").count(), 1);
        for forbidden in ["observability", "http2", "tun_", "clients"] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
        }
        assert!(text.ends_with("[ip_proxy]\nenabled = false\n"), "{text}");
    }

    /// Every key is one v0.13.0 reads: it refuses any other.
    #[test]
    fn the_configuration_is_written_as_masque_server_reads_it() {
        assert_eq!(
            render(4001, &files()),
            "[tls]\n\
             cert_path = \"/tmp/masque/leaf.pem\"\n\
             key_path = \"/tmp/masque/leaf.key\"\n\
             \n\
             [[listeners]]\n\
             listen_addr = \"127.0.0.1:4001\"\n\
             transport = \"http3\"\n\
             shards = 1\n\
             \n\
             [listeners.auth]\n\
             enabled = true\n\
             mode = \"basic\"\n\
             \n\
             [[listeners.auth.users]]\n\
             username = \"test\"\n\
             password_hash = \"$argon2id$v=19$m=19456,t=2,p=1$1xNVXhqKU7jJ6cqTBJKphQ$GXXAINVTW1qhloFtN1IR8lSr7pI7QEY79fq4K6d8scQ\"\n\
             \n\
             [tcp_proxy]\n\
             allow_targets = [\"0.0.0.0/0\", \"::/0\"]\n\
             deny_targets = []\n\
             \n\
             [udp_proxy]\n\
             allow_targets = [\"0.0.0.0/0\", \"::/0\"]\n\
             deny_targets = []\n\
             \n\
             [ip_proxy]\n\
             enabled = false\n"
        );
    }
}
