//! A shadowsocks-rust `ssserver` child process on the loopback, the reference
//! for the `ss` outbound (phase 2 M6 design, M6-D5). The same rules as for
//! sing-box apply: nothing is downloaded or installed here, and every server
//! of the rendered configuration listens on 127.0.0.1 only — its TCP port and
//! the UDP port of the same number (`tcp_and_udp`).

use crate::{REQUIRED_ENV, Reference, free_port};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BINARY_ENV: &str = "RURGE_TEST_SSSERVER";

/// `RURGE_TEST_SSSERVER`, else the first `ssserver` on `PATH`.
pub fn locate() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BINARY_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let name = if cfg!(windows) {
        "ssserver.exe"
    } else {
        "ssserver"
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure instead.
pub fn ssserver_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if std::env::var(REQUIRED_ENV).as_deref() == Ok("1") {
        panic!("{REQUIRED_ENV}=1 but no ssserver was found ({BINARY_ENV} or PATH)");
    }
    eprintln!("skipping {test}: no ssserver ({BINARY_ENV} or PATH); see tests/interop/README.md");
    None
}

/// One server of the configuration.
pub struct SsInbound {
    pub method: &'static str,
    /// For a 2022 method the Base64 key — with `users`, the server's
    /// identity key (SIP023).
    pub password: String,
    /// 2022 users (name, Base64 key of the method's length), told apart by
    /// the identity header.
    pub users: Vec<(String, String)>,
}

/// The whole configuration for `servers`, each on its loopback port.
pub fn render(servers: &[(SsInbound, u16)]) -> Value {
    let rendered: Vec<Value> = servers
        .iter()
        .map(|(server, port)| {
            let mut v = json!({
                "server": "127.0.0.1",
                "server_port": port,
                "method": server.method,
                "password": server.password,
                "mode": "tcp_and_udp",
            });
            if !server.users.is_empty() {
                v["users"] = server
                    .users
                    .iter()
                    .map(|(name, password)| json!({ "name": name, "password": password }))
                    .collect();
            }
            v
        })
        .collect();
    json!({ "servers": rendered })
}

/// A running ssserver; killed and reaped on drop.
pub struct Ssserver(Reference);

impl Ssserver {
    /// Writes the configuration into `dir`, starts `binary` there and waits
    /// until every server accepts TCP connections.
    pub fn spawn(binary: &Path, dir: &Path, servers: Vec<SsInbound>) -> Ssserver {
        let with_ports: Vec<(SsInbound, u16)> =
            servers.into_iter().map(|s| (s, free_port())).collect();
        let ports: Vec<u16> = with_ports.iter().map(|(_, p)| *p).collect();
        let config = dir.join("ssserver.json");
        std::fs::write(&config, render(&with_ports).to_string()).expect("write the config");
        let mut command = Command::new(binary);
        command.arg("-c").arg(&config).current_dir(dir);
        Ssserver(Reference::start(
            "ssserver",
            command,
            ports,
            dir.join("ssserver.log"),
        ))
    }

    /// The loopback port (TCP and UDP) of the `index`-th server.
    pub fn port(&self, index: usize) -> u16 {
        self.0.port(index)
    }

    pub fn log_text(&self) -> String {
        self.0.log_text()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both() -> Vec<(SsInbound, u16)> {
        vec![
            (
                SsInbound {
                    method: "aes-128-gcm",
                    password: "s3same".into(),
                    users: Vec::new(),
                },
                3001,
            ),
            (
                SsInbound {
                    method: "2022-blake3-aes-128-gcm",
                    password: "MDEyMzQ1Njc4OWFiY2RlZg==".into(),
                    users: vec![("u".into(), "ZmVkY2JhOTg3NjU0MzIxMA==".into())],
                },
                3002,
            ),
        ]
    }

    #[test]
    fn the_configuration_stays_on_the_loopback() {
        let config = render(&both());
        let text = config.to_string();
        for forbidden in [
            "0.0.0.0",
            "::",
            "local_address",
            "locals",
            "manager",
            "plugin",
            "outbound_",
            "acl",
        ] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
        }
        let top: Vec<&String> = config.as_object().unwrap().keys().collect();
        assert_eq!(top, ["servers"]);
        for server in config["servers"].as_array().unwrap() {
            assert_eq!(server["server"], "127.0.0.1");
            assert_eq!(server["mode"], "tcp_and_udp");
        }
    }

    #[test]
    fn servers_are_rendered_as_ssserver_spells_them() {
        let config = render(&both());
        assert_eq!(
            config["servers"][0],
            json!({
                "server": "127.0.0.1",
                "server_port": 3001,
                "method": "aes-128-gcm",
                "password": "s3same",
                "mode": "tcp_and_udp",
            })
        );
        let multi = &config["servers"][1];
        assert_eq!(multi["password"], "MDEyMzQ1Njc4OWFiY2RlZg==");
        assert_eq!(
            multi["users"],
            json!([{ "name": "u", "password": "ZmVkY2JhOTg3NjU0MzIxMA==" }])
        );
    }
}
