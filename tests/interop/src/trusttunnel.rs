//! The TrustTunnel endpoint v1.1.0 as a child process on the loopback: the
//! reference for `trust-tunnel` and, through the same TCP mode (a standard
//! HTTP/2 CONNECT with Basic authentication), for `h2-connect`'s plain
//! CONNECT — sing-box 1.14.2's `http` inbound speaks HTTP/1 only. It is
//! built for Linux and macOS only: on Windows a missing binary is always a
//! skip, even with `RURGE_INTEROP_REQUIRED=1`. The same rules as for sing-box
//! apply: nothing is downloaded or installed here, and the rendered
//! configuration listens on 127.0.0.1 only.
//!
//! The endpoint picks its host by the exact SNI, so a client sends
//! `sni=<HOSTNAME>` and trusts a certificate for that name; it refuses
//! private and loopback targets unless `allow_private_network_connections`
//! is set, which the loopback echo needs.

use crate::{REQUIRED_ENV, Reference, TlsFiles, free_port};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BINARY_ENV: &str = "RURGE_TEST_TRUSTTUNNEL";

/// The endpoint's one host: the SNI a client must send and the name its
/// certificate is for.
pub const HOSTNAME: &str = "tt.test";

/// `RURGE_TEST_TRUSTTUNNEL`, else the first `trusttunnel_endpoint` on `PATH`.
pub fn locate() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BINARY_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("trusttunnel_endpoint"))
        .find(|candidate| candidate.is_file())
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure on Linux
/// and macOS, the platforms the endpoint is built for.
pub fn trusttunnel_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if cfg!(any(target_os = "linux", target_os = "macos"))
        && std::env::var(REQUIRED_ENV).as_deref() == Ok("1")
    {
        panic!("{REQUIRED_ENV}=1 but no trusttunnel_endpoint was found ({BINARY_ENV} or PATH)");
    }
    eprintln!(
        "skipping {test}: no trusttunnel_endpoint ({BINARY_ENV} or PATH; Linux and macOS only); see tests/interop/README.md"
    );
    None
}

/// The endpoint's three files.
#[derive(Debug, PartialEq, Eq)]
pub struct Rendered {
    /// `vpn.toml`: the listener, the protocols and the forwarding.
    pub vpn: String,
    /// `hosts.toml`: [`HOSTNAME`] with the certificate and key of `tls`.
    pub hosts: String,
    /// `credentials.toml`: one client.
    pub credentials: String,
}

/// A TOML basic string.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn path(p: &Path) -> String {
    quoted(&p.to_string_lossy())
}

/// The whole configuration: HTTP/2 only on `127.0.0.1:port`, IPv4 only,
/// direct forwarding that may reach the loopback, one client. The endpoint
/// resolves file paths against its working directory, so they must be
/// absolute.
pub fn render(
    port: u16,
    tls: &TlsFiles,
    credentials: &Path,
    username: &str,
    password: &str,
) -> Rendered {
    let vpn = format!(
        "listen_address = \"127.0.0.1:{port}\"\n\
         ipv6_available = false\n\
         allow_private_network_connections = true\n\
         credentials_file = {}\n\
         \n\
         [listen_protocols.http2]\n\
         \n\
         [forward_protocol]\n\
         direct = {{}}\n",
        path(credentials)
    );
    let hosts = format!(
        "[[main_hosts]]\n\
         hostname = {}\n\
         cert_chain_path = {}\n\
         private_key_path = {}\n",
        quoted(HOSTNAME),
        path(&tls.certificate),
        path(&tls.key)
    );
    let credentials = format!(
        "[[client]]\nusername = {}\npassword = {}\n",
        quoted(username),
        quoted(password)
    );
    Rendered {
        vpn,
        hosts,
        credentials,
    }
}

/// A running endpoint; killed and reaped on drop.
pub struct TrustTunnel(Reference);

impl TrustTunnel {
    /// Writes the configuration into `dir` (which must be absolute), starts
    /// `binary` there and waits until it accepts connections. `tls` is a
    /// certificate for [`HOSTNAME`].
    pub fn spawn(
        binary: &Path,
        dir: &Path,
        tls: &TlsFiles,
        username: &str,
        password: &str,
    ) -> TrustTunnel {
        let port = free_port();
        let (vpn, hosts, credentials) = (
            dir.join("vpn.toml"),
            dir.join("hosts.toml"),
            dir.join("credentials.toml"),
        );
        let rendered = render(port, tls, &credentials, username, password);
        for (file, text) in [
            (&vpn, &rendered.vpn),
            (&hosts, &rendered.hosts),
            (&credentials, &rendered.credentials),
        ] {
            std::fs::write(file, text).expect("write the config");
        }
        let mut command = Command::new(binary);
        command.arg(&vpn).arg(&hosts).current_dir(dir);
        TrustTunnel(Reference::start(
            "trusttunnel_endpoint",
            command,
            vec![port],
            dir.join("trusttunnel.log"),
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

    fn files() -> TlsFiles {
        TlsFiles {
            certificate: "/tmp/tt/leaf.pem".into(),
            key: "/tmp/tt/leaf.key".into(),
            client_ca: None,
        }
    }

    #[test]
    fn the_configuration_stays_on_the_loopback() {
        let rendered = render(
            4001,
            &files(),
            Path::new("/tmp/tt/credentials.toml"),
            "u",
            "pw",
        );
        let text = format!("{}{}{}", rendered.vpn, rendered.hosts, rendered.credentials);
        for forbidden in ["0.0.0.0", "::", "socks5", "listen_protocols.quic", "http1"] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
        }
        let listen: Vec<&str> = rendered
            .vpn
            .lines()
            .filter(|l| l.starts_with("listen_address"))
            .collect();
        assert_eq!(listen, ["listen_address = \"127.0.0.1:4001\""]);
        assert!(rendered.vpn.contains("[forward_protocol]\ndirect = {}\n"));
    }

    #[test]
    fn the_configuration_is_written_as_the_endpoint_reads_it() {
        let rendered = render(
            4001,
            &files(),
            Path::new("/tmp/tt/credentials.toml"),
            "u",
            "p\"w",
        );
        assert_eq!(
            rendered,
            Rendered {
                vpn: "listen_address = \"127.0.0.1:4001\"\n\
                      ipv6_available = false\n\
                      allow_private_network_connections = true\n\
                      credentials_file = \"/tmp/tt/credentials.toml\"\n\
                      \n\
                      [listen_protocols.http2]\n\
                      \n\
                      [forward_protocol]\n\
                      direct = {}\n"
                    .into(),
                hosts: "[[main_hosts]]\n\
                        hostname = \"tt.test\"\n\
                        cert_chain_path = \"/tmp/tt/leaf.pem\"\n\
                        private_key_path = \"/tmp/tt/leaf.key\"\n"
                    .into(),
                credentials: "[[client]]\nusername = \"u\"\npassword = \"p\\\"w\"\n".into(),
            }
        );
    }

    #[test]
    fn windows_paths_are_escaped() {
        assert_eq!(path(Path::new(r"C:\t\x.pem")), r#""C:\\t\\x.pem""#);
    }
}
