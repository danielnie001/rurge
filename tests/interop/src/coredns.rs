//! CoreDNS v1.14.7 (Apache-2.0) as a child process on the loopback: the
//! strict reference for the `quic://` (DoQ) and `h3://` (DoH3) DNS upstreams
//! (phase 2 M7 design 7.4), built for every platform CI runs on. Its DoQ
//! server closes the connection over a message ID other than 0 or an
//! edns-tcp-keepalive option (RFC 9250 4.3.3), and its DoH3 server answers
//! on `/dns-query` alone. The same rules as for sing-box apply: nothing is
//! downloaded or installed here, and the rendered Corefile listens on
//! 127.0.0.1 only. It answers [`DNS_NAME`] from templates and nothing else —
//! CoreDNS runs only the plugins a Corefile names, and this one names no
//! `forward`. It has no TCP port to probe: it is up once it has listed its
//! servers.

use crate::{DNS_NAME, DNS_V4, DNS_V6, REQUIRED_ENV, Reference, TlsFiles, free_udp_port};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BINARY_ENV: &str = "RURGE_TEST_COREDNS";

/// `RURGE_TEST_COREDNS`, else the first `coredns` on `PATH`.
pub fn locate() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BINARY_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let name = if cfg!(windows) {
        "coredns.exe"
    } else {
        "coredns"
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure instead.
pub fn coredns_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if std::env::var(REQUIRED_ENV).as_deref() == Ok("1") {
        panic!("{REQUIRED_ENV}=1 but no coredns was found ({BINARY_ENV} or PATH)");
    }
    eprintln!("skipping {test}: no coredns ({BINARY_ENV} or PATH); see tests/interop/README.md");
    None
}

/// A Corefile token in quotes, which a path with a space needs: a quote is
/// the one character escaped there, a backslash stays as it is.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\\\""))
}

/// What CoreDNS prints once the server of `scheme` on `port` is up.
fn listening(scheme: &str, port: u16) -> String {
    format!("{scheme}://.:{port} on 127.0.0.1")
}

/// The whole Corefile: a DoQ server on UDP port `quic` and a DoH3 server on
/// UDP port `h3`, both bound to 127.0.0.1 with the certificate and key of
/// `tls` (PEM paths), answering [`DNS_NAME`]'s A and AAAA records from
/// templates.
pub fn render(quic: u16, h3: u16, tls: &TlsFiles) -> String {
    assert!(
        tls.client_ca.is_none(),
        "CoreDNS asks for no client certificate here"
    );
    let (certificate, key) = (
        quoted(&tls.certificate.to_string_lossy()),
        quoted(&tls.key.to_string_lossy()),
    );
    let mut text = String::new();
    for (scheme, port) in [("quic", quic), ("https3", h3)] {
        text +=
            &format!("{scheme}://.:{port} {{\n    bind 127.0.0.1\n    tls {certificate} {key}\n");
        for (qtype, address) in [("A", IpAddr::V4(DNS_V4)), ("AAAA", IpAddr::V6(DNS_V6))] {
            text += &format!(
                "    template IN {qtype} {DNS_NAME} {{\n        answer \"{{{{ .Name }}}} 60 IN {qtype} {address}\"\n    }}\n"
            );
        }
        text += "}\n";
    }
    text
}

/// A running CoreDNS; killed and reaped on drop.
pub struct CoreDns {
    server: Reference,
    quic: u16,
    h3: u16,
}

impl CoreDns {
    /// Writes the Corefile into `dir`, starts `binary` there and waits until
    /// it says both servers listen.
    pub fn spawn(binary: &Path, dir: &Path, tls: &TlsFiles) -> CoreDns {
        let (quic, h3) = (free_udp_port(), free_udp_port());
        let corefile = dir.join("Corefile");
        std::fs::write(&corefile, render(quic, h3, tls)).expect("write the Corefile");
        let mut command = Command::new(binary);
        command.arg("-conf").arg(&corefile).current_dir(dir);
        let mut server = Reference::start("coredns", command, Vec::new(), dir.join("coredns.log"));
        server.wait_for_output(&listening("quic", quic));
        server.wait_for_output(&listening("https3", h3));
        CoreDns { server, quic, h3 }
    }

    /// The loopback UDP port of DNS over QUIC.
    pub fn quic_port(&self) -> u16 {
        self.quic
    }

    /// The loopback UDP port of DNS over HTTP/3.
    pub fn h3_port(&self) -> u16 {
        self.h3
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
            certificate: "/tmp/dns/leaf.pem".into(),
            key: "/tmp/dns/leaf.key".into(),
            client_ca: None,
        }
    }

    /// Two servers, each bound to 127.0.0.1; nothing forwarded anywhere.
    #[test]
    fn the_corefile_stays_on_the_loopback() {
        let text = render(4001, 4002, &files());
        let servers: Vec<&str> = text
            .lines()
            .filter(|l| !l.starts_with(' ') && l.ends_with('{'))
            .collect();
        assert_eq!(servers, ["quic://.:4001 {", "https3://.:4002 {"]);
        let binds: Vec<&str> = text
            .lines()
            .filter(|l| l.trim_start().starts_with("bind"))
            .collect();
        assert_eq!(binds, ["    bind 127.0.0.1", "    bind 127.0.0.1"]);
        for forbidden in ["forward", "0.0.0.0", "hosts", "debug"] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
        }
    }

    #[test]
    fn the_corefile_is_written_as_coredns_reads_it() {
        let text = render(4001, 4002, &files());
        let block = |head: &'static str| {
            [
                head,
                "    bind 127.0.0.1",
                r#"    tls "/tmp/dns/leaf.pem" "/tmp/dns/leaf.key""#,
                "    template IN A interop.rurge.test {",
                r#"        answer "{{ .Name }} 60 IN A 192.0.2.10""#,
                "    }",
                "    template IN AAAA interop.rurge.test {",
                r#"        answer "{{ .Name }} 60 IN AAAA 2001:db8::10""#,
                "    }",
                "}",
            ]
        };
        let expected: Vec<&str> = block("quic://.:4001 {")
            .into_iter()
            .chain(block("https3://.:4002 {"))
            .collect();
        assert_eq!(text.lines().collect::<Vec<_>>(), expected);
        assert!(text.ends_with("}\n"));
        assert_eq!(listening("quic", 4001), "quic://.:4001 on 127.0.0.1");
    }

    /// Windows paths keep their backslashes: only a quote is escaped in a
    /// Corefile's quoted token.
    #[test]
    fn paths_are_quoted() {
        assert_eq!(quoted(r"C:\t\x.pem"), r#""C:\t\x.pem""#);
        assert_eq!(quoted("a b\"c"), r#""a b\"c""#);
    }
}
