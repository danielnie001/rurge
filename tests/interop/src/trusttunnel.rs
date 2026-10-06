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
//!
//! Its QUIC listener (`Listen::Http3`) serves the same CONNECTs over HTTP/3:
//! the reference for `trust-tunnel` with `h3=true`.

use crate::{
    REQUIRED_ENV, Reference, TlsFiles, free_port, free_tcp_and_udp_port, toml_path, toml_string,
};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BINARY_ENV: &str = "RURGE_TEST_TRUSTTUNNEL";

/// The endpoint's one host: the SNI a client must send and the name its
/// certificate is for.
pub const HOSTNAME: &str = "tt.test";

/// What the endpoint logs (at `info`) once its QUIC listener is bound.
const LISTENING_UDP: &str = "Listening to UDP";

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

/// What the endpoint serves on its port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Listen {
    /// HTTP/2 over TLS, on the TCP port (`[listen_protocols.http2]`).
    Http2,
    /// HTTP/3 alone, on the UDP port (`[listen_protocols.quic]`). The
    /// endpoint binds the TCP port of the same number too, and serves
    /// nothing there.
    Http3,
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

/// The whole configuration: `listen`'s one protocol on `127.0.0.1:port`,
/// IPv4 only, direct forwarding that may reach the loopback, one client. The
/// endpoint resolves file paths against its working directory, so they must
/// be absolute.
pub fn render(
    port: u16,
    listen: Listen,
    tls: &TlsFiles,
    credentials: &Path,
    username: &str,
    password: &str,
) -> Rendered {
    let protocol = match listen {
        Listen::Http2 => "http2",
        Listen::Http3 => "quic",
    };
    let vpn = format!(
        "listen_address = \"127.0.0.1:{port}\"\n\
         ipv6_available = false\n\
         allow_private_network_connections = true\n\
         credentials_file = {}\n\
         \n\
         [listen_protocols.{protocol}]\n\
         \n\
         [forward_protocol]\n\
         direct = {{}}\n",
        toml_path(credentials)
    );
    let hosts = format!(
        "[[main_hosts]]\n\
         hostname = {}\n\
         cert_chain_path = {}\n\
         private_key_path = {}\n",
        toml_string(HOSTNAME),
        toml_path(&tls.certificate),
        toml_path(&tls.key)
    );
    let credentials = format!(
        "[[client]]\nusername = {}\npassword = {}\n",
        toml_string(username),
        toml_string(password)
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
    /// `binary` there and waits until it serves `listen`: its TCP port
    /// accepts connections — and, over HTTP/3, its log says the QUIC
    /// listener is bound. `tls` is a certificate for [`HOSTNAME`].
    pub fn spawn(
        binary: &Path,
        dir: &Path,
        listen: Listen,
        tls: &TlsFiles,
        username: &str,
        password: &str,
    ) -> TrustTunnel {
        // over HTTP/3 the endpoint binds both on one number
        let port = match listen {
            Listen::Http2 => free_port(),
            Listen::Http3 => free_tcp_and_udp_port(),
        };
        let (vpn, hosts, credentials) = (
            dir.join("vpn.toml"),
            dir.join("hosts.toml"),
            dir.join("credentials.toml"),
        );
        let rendered = render(port, listen, tls, &credentials, username, password);
        for (file, text) in [
            (&vpn, &rendered.vpn),
            (&hosts, &rendered.hosts),
            (&credentials, &rendered.credentials),
        ] {
            std::fs::write(file, text).expect("write the config");
        }
        let mut command = Command::new(binary);
        command.arg(&vpn).arg(&hosts).current_dir(dir);
        let mut endpoint = Reference::start(
            "trusttunnel_endpoint",
            command,
            vec![port],
            dir.join("trusttunnel.log"),
        );
        if listen == Listen::Http3 {
            endpoint.wait_for_output(LISTENING_UDP);
        }
        TrustTunnel(endpoint)
    }

    /// The loopback port: TCP over HTTP/2, UDP over HTTP/3.
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

    /// One protocol, on the loopback: HTTP/2 alone, or QUIC alone.
    #[test]
    fn the_configuration_stays_on_the_loopback() {
        for (listen, protocol) in [
            (Listen::Http2, "[listen_protocols.http2]"),
            (Listen::Http3, "[listen_protocols.quic]"),
        ] {
            let rendered = render(
                4001,
                listen,
                &files(),
                Path::new("/tmp/tt/credentials.toml"),
                "u",
                "pw",
            );
            let text = format!("{}{}{}", rendered.vpn, rendered.hosts, rendered.credentials);
            for forbidden in ["0.0.0.0", "::", "socks5", "http1"] {
                assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
            }
            let addresses: Vec<&str> = rendered
                .vpn
                .lines()
                .filter(|l| l.starts_with("listen_address"))
                .collect();
            assert_eq!(addresses, ["listen_address = \"127.0.0.1:4001\""]);
            let protocols: Vec<&str> = rendered
                .vpn
                .lines()
                .filter(|l| l.starts_with("[listen_protocols."))
                .collect();
            assert_eq!(protocols, [protocol]);
            assert!(rendered.vpn.contains("[forward_protocol]\ndirect = {}\n"));
        }
    }

    #[test]
    fn the_configuration_is_written_as_the_endpoint_reads_it() {
        let rendered = render(
            4001,
            Listen::Http2,
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
        // the QUIC listener with its defaults: an empty table turns it on
        let http3 = render(
            4001,
            Listen::Http3,
            &files(),
            Path::new("/tmp/tt/credentials.toml"),
            "u",
            "p\"w",
        );
        assert_eq!(
            http3.vpn,
            "listen_address = \"127.0.0.1:4001\"\n\
             ipv6_available = false\n\
             allow_private_network_connections = true\n\
             credentials_file = \"/tmp/tt/credentials.toml\"\n\
             \n\
             [listen_protocols.quic]\n\
             \n\
             [forward_protocol]\n\
             direct = {}\n"
        );
        assert_eq!(
            (http3.hosts, http3.credentials),
            (rendered.hosts, rendered.credentials)
        );
    }
}
