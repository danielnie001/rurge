//! An OpenSSH `sshd` child process on the loopback, for the `ssh`
//! interoperability tests (phase 2 M4 design §10). Nothing is installed or
//! configured outside a temporary directory: the rendered `sshd_config`
//! listens on 127.0.0.1 only, lets in nothing but the one key the test made,
//! and allows local port forwarding only. A `sshd` that is not run as root
//! logs in only the user running it, so the tests log in with a key.

use crate::{REQUIRED_ENV, Reference, free_port};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BINARY_ENV: &str = "RURGE_TEST_SSHD";

/// `RURGE_TEST_SSHD`, else `/usr/sbin/sshd`, else the first `sshd` on `PATH`.
pub fn locate() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BINARY_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let usual = Path::new("/usr/sbin/sshd");
    if usual.is_file() {
        return Some(usual.to_path_buf());
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("sshd"))
        .find(|candidate| candidate.is_file())
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure instead.
pub fn sshd_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if std::env::var(REQUIRED_ENV).as_deref() == Ok("1") {
        panic!("{REQUIRED_ENV}=1 but no sshd was found ({BINARY_ENV}, /usr/sbin/sshd or PATH)");
    }
    eprintln!(
        "skipping {test}: no sshd ({BINARY_ENV}, /usr/sbin/sshd or PATH); see tests/interop/README.md"
    );
    None
}

/// What the server offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algorithms {
    /// OpenSSH's defaults.
    Default,
    /// Only what Surge's manual requires: `curve25519-sha256` and
    /// `aes128-gcm@openssh.com`.
    SurgeMinimum,
}

/// The `sshd_config` of a server on `port`, its files in `dir`.
pub fn render(dir: &Path, port: u16, algorithms: Algorithms) -> String {
    let mut text = format!(
        "Port {port}\nListenAddress 127.0.0.1\nHostKey {host}\nAuthorizedKeysFile {keys}\n\
PidFile none\nUsePAM no\nStrictModes no\nPasswordAuthentication no\n\
KbdInteractiveAuthentication no\nPubkeyAuthentication yes\nAllowTcpForwarding local\n\
AllowAgentForwarding no\nX11Forwarding no\nPermitTTY no\nPermitTunnel no\nLogLevel ERROR\n",
        host = dir.join("host_key").display(),
        keys = dir.join("authorized_keys").display(),
    );
    if algorithms == Algorithms::SurgeMinimum {
        text.push_str("KexAlgorithms curve25519-sha256\nCiphers aes128-gcm@openssh.com\n");
    }
    text
}

/// A running `sshd`; killed and reaped on drop.
pub struct Sshd(Reference);

impl Sshd {
    /// Writes the host key (owner-only, or `sshd` refuses it), the one
    /// authorized key and the configuration into `dir`, starts `binary` in
    /// the foreground and waits until it accepts connections.
    pub fn spawn(
        binary: &Path,
        dir: &Path,
        host_key: &str,
        authorized_key: &str,
        algorithms: Algorithms,
    ) -> Sshd {
        let port = free_port();
        let host = dir.join("host_key");
        std::fs::write(&host, host_key).expect("write the host key");
        owner_only(&host);
        std::fs::write(dir.join("authorized_keys"), format!("{authorized_key}\n"))
            .expect("write the authorized key");
        let config = dir.join("sshd_config");
        std::fs::write(&config, render(dir, port, algorithms)).expect("write the config");
        let mut command = Command::new(binary);
        // in the foreground, logging to stderr: the log file
        command.arg("-D").arg("-e").arg("-f").arg(&config);
        Sshd(Reference::start(
            "sshd",
            command,
            vec![port],
            dir.join("sshd.log"),
        ))
    }

    pub fn port(&self) -> u16 {
        self.0.port(0)
    }

    pub fn log_text(&self) -> String {
        self.0.log_text()
    }
}

#[cfg(unix)]
fn owner_only(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .expect("make the host key owner-only");
}

#[cfg(not(unix))]
fn owner_only(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The configuration reaches no further than the loopback and the one
    /// key: no password, no PAM, no terminal, no remote forwarding.
    #[test]
    fn the_configuration_stays_on_the_loopback_with_one_key() {
        let dir = Path::new("/tmp/rurge-sshd");
        let text = render(dir, 2222, Algorithms::Default);
        for line in [
            "Port 2222",
            "ListenAddress 127.0.0.1",
            "UsePAM no",
            "PasswordAuthentication no",
            "KbdInteractiveAuthentication no",
            "AllowTcpForwarding local",
            "PermitTTY no",
        ] {
            assert!(text.lines().any(|l| l == line), "{line} missing:\n{text}");
        }
        assert!(!text.contains("Ciphers") && !text.contains("KexAlgorithms"));
        let minimum = render(dir, 2222, Algorithms::SurgeMinimum);
        assert!(
            minimum.ends_with("KexAlgorithms curve25519-sha256\nCiphers aes128-gcm@openssh.com\n"),
            "{minimum}"
        );
    }
}
