//! Surge's official `snell-server` v5.0.1 as a child process on the loopback,
//! the reference for the `snell` outbound next to sing-box (phase 2 M6
//! design 4.4, M6-D5). It is built for Linux only: elsewhere a missing
//! binary is always a skip, even with `RURGE_INTEROP_REQUIRED=1`. The same
//! rules as for sing-box apply: nothing is downloaded or installed here, and
//! the rendered configuration listens on 127.0.0.1 only.

use crate::{REQUIRED_ENV, Reference, free_port};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BINARY_ENV: &str = "RURGE_TEST_SNELL_SERVER";

/// `RURGE_TEST_SNELL_SERVER`, else the first `snell-server` on `PATH`.
pub fn locate() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BINARY_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("snell-server"))
        .find(|candidate| candidate.is_file())
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure on Linux,
/// the only platform snell-server is built for.
pub fn snell_server_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if cfg!(target_os = "linux") && std::env::var(REQUIRED_ENV).as_deref() == Ok("1") {
        panic!("{REQUIRED_ENV}=1 but no snell-server was found ({BINARY_ENV} or PATH)");
    }
    eprintln!(
        "skipping {test}: no snell-server ({BINARY_ENV} or PATH; Linux only); see tests/interop/README.md"
    );
    None
}

/// The whole configuration: one server on `127.0.0.1:port`, IPv4 only, with
/// simple-obfs `http` in front when `obfs_http`.
pub fn render(port: u16, psk: &str, obfs_http: bool) -> String {
    let mut text =
        format!("[snell-server]\nlisten = 127.0.0.1:{port}\npsk = {psk}\nipv6 = false\n");
    if obfs_http {
        text.push_str("obfs = http\n");
    }
    text
}

/// A running snell-server; killed and reaped on drop.
pub struct SnellServer(Reference);

impl SnellServer {
    /// Writes the configuration into `dir`, starts `binary` there and waits
    /// until it accepts connections.
    pub fn spawn(binary: &Path, dir: &Path, psk: &str, obfs_http: bool) -> SnellServer {
        let port = free_port();
        let config = dir.join("snell-server.conf");
        std::fs::write(&config, render(port, psk, obfs_http)).expect("write the config");
        let mut command = Command::new(binary);
        command.arg("-c").arg(&config).current_dir(dir);
        SnellServer(Reference::start(
            "snell-server",
            command,
            vec![port],
            dir.join("snell-server.log"),
        ))
    }

    /// The loopback port.
    pub fn port(&self) -> u16 {
        self.0.port(0)
    }

    pub fn log_text(&self) -> String {
        self.0.log_text()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_configuration_stays_on_the_loopback() {
        for obfs_http in [false, true] {
            let text = render(4001, "sn3ll", obfs_http);
            for forbidden in ["0.0.0.0", "::", "egress-interface", "dns"] {
                assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
            }
            let listen: Vec<&str> = text.lines().filter(|l| l.starts_with("listen")).collect();
            assert_eq!(listen, ["listen = 127.0.0.1:4001"]);
        }
    }

    #[test]
    fn the_configuration_is_written_as_snell_server_reads_it() {
        assert_eq!(
            render(4001, "sn3ll", false),
            "[snell-server]\nlisten = 127.0.0.1:4001\npsk = sn3ll\nipv6 = false\n"
        );
        assert_eq!(
            render(4002, "sn3ll", true),
            "[snell-server]\nlisten = 127.0.0.1:4002\npsk = sn3ll\nipv6 = false\nobfs = http\n"
        );
    }
}
