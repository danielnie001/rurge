//! A sing-box child process on the loopback, for interoperability tests
//! (M1 design §8). Nothing here downloads, installs or configures anything
//! outside a temporary directory: the rendered configuration listens on
//! 127.0.0.1 only — but for the UDP port of a WireGuard endpoint, which
//! sing-box opens on every address — its single outbound is `direct`, and it
//! never holds a key that touches the machine (`set_system_proxy`, `tun`,
//! `auto_route`; a WireGuard endpoint runs in user space).

pub mod shadowsocks_rust;
pub mod sshd;
pub mod xray;

use base64::Engine;
use serde_json::{Value, json};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const BINARY_ENV: &str = "RURGE_TEST_SING_BOX";
pub const REQUIRED_ENV: &str = "RURGE_INTEROP_REQUIRED";
const READY_TIMEOUT: Duration = Duration::from_secs(15);

/// `RURGE_TEST_SING_BOX`, else the first `sing-box` on `PATH`.
pub fn locate() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BINARY_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let name = if cfg!(windows) {
        "sing-box.exe"
    } else {
        "sing-box"
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure instead.
pub fn sing_box_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if std::env::var(REQUIRED_ENV).as_deref() == Ok("1") {
        panic!("{REQUIRED_ENV}=1 but no sing-box was found ({BINARY_ENV} or PATH)");
    }
    eprintln!("skipping {test}: no sing-box ({BINARY_ENV} or PATH); see tests/interop/README.md");
    None
}

/// A port that was free a moment ago.
pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("a free loopback port")
        .port()
}

/// A UDP port that was free a moment ago.
pub fn free_udp_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .and_then(|s| s.local_addr())
        .expect("a free loopback port")
        .port()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboundKind {
    Http,
    Socks,
    Mixed,
    Trojan,
    Vmess,
    AnyTls,
    /// Relays the TLS handshake to `127.0.0.1:handshake_port` and hands what
    /// it unwraps to the inbound at index `detour`. `users[0]` holds the
    /// password (version 2 has no user names; the name is ignored).
    ShadowTls {
        version: u8,
        handshake_port: u16,
        detour: usize,
    },
    /// `users[0]` holds the password (the name is ignored); the further
    /// users are 2022 users, told apart by the identity header, and the
    /// password is then the server's identity key.
    Shadowsocks {
        method: &'static str,
    },
}

pub struct TlsFiles {
    /// PEM paths.
    pub certificate: PathBuf,
    pub key: PathBuf,
    /// Require a client certificate signed by this CA (PEM path).
    pub client_ca: Option<PathBuf>,
}

pub struct Inbound {
    pub kind: InboundKind,
    /// Empty = no authentication.
    pub users: Vec<(String, String)>,
    /// sing-box's `http`, `trojan`, `vmess` and `anytls` inbounds speak TLS;
    /// `socks` and `mixed` do not.
    pub tls: Option<TlsFiles>,
    /// A V2Ray WebSocket transport on this path (`trojan` and `vmess` only).
    pub ws_path: Option<String>,
}

/// The whole configuration for `inbounds`, each on its loopback port.
pub fn render(inbounds: &[(Inbound, u16)]) -> Value {
    let rendered: Vec<Value> = inbounds
        .iter()
        .enumerate()
        .map(|(i, (inbound, port))| {
            let mut v = json!({
                "type": match inbound.kind {
                    InboundKind::Http => "http",
                    InboundKind::Socks => "socks",
                    InboundKind::Mixed => "mixed",
                    InboundKind::Trojan => "trojan",
                    InboundKind::Vmess => "vmess",
                    InboundKind::AnyTls => "anytls",
                    InboundKind::ShadowTls { .. } => "shadowtls",
                    InboundKind::Shadowsocks { .. } => "shadowsocks",
                },
                "tag": format!("in-{i}"),
                "listen": "127.0.0.1",
                "listen_port": port,
            });
            if let InboundKind::ShadowTls {
                version,
                handshake_port,
                detour,
            } = inbound.kind
            {
                let (name, password) = inbound.users.first().expect("a Shadow TLS password");
                v["version"] = json!(version);
                if version == 3 {
                    v["users"] = json!([{ "name": name, "password": password }]);
                    v["strict_mode"] = json!(true);
                } else {
                    v["password"] = json!(password);
                }
                // a loopback IP literal: sing-box resolves nothing
                v["handshake"] = json!({ "server": "127.0.0.1", "server_port": handshake_port });
                v["detour"] = json!(format!("in-{detour}"));
            } else if let InboundKind::Shadowsocks { method } = inbound.kind {
                let (_, password) = inbound.users.first().expect("a Shadowsocks password");
                v["method"] = json!(method);
                v["password"] = json!(password);
                if inbound.users.len() > 1 {
                    v["users"] = inbound.users[1..]
                        .iter()
                        .map(|(u, p)| json!({ "name": u, "password": p }))
                        .collect();
                }
            } else if !inbound.users.is_empty() {
                v["users"] = inbound
                    .users
                    .iter()
                    .map(|(u, p)| match inbound.kind {
                        // sing-box's trojan and anytls users are `name` + `password`
                        InboundKind::Trojan | InboundKind::AnyTls => {
                            json!({ "name": u, "password": p })
                        }
                        // the second half is the id; `alterId: 0` selects the AEAD handshake
                        InboundKind::Vmess => json!({ "name": u, "uuid": p, "alterId": 0 }),
                        _ => json!({ "username": u, "password": p }),
                    })
                    .collect();
            }
            if let Some(tls) = &inbound.tls {
                assert!(
                    matches!(
                        inbound.kind,
                        InboundKind::Http
                            | InboundKind::Trojan
                            | InboundKind::Vmess
                            | InboundKind::AnyTls
                    ),
                    "only sing-box's http, trojan, vmess and anytls inbounds are given tls here"
                );
                let mut t = json!({
                    "enabled": true,
                    "certificate_path": tls.certificate,
                    "key_path": tls.key,
                });
                if let Some(ca) = &tls.client_ca {
                    t["client_authentication"] = json!("require-and-verify");
                    t["client_certificate_path"] = json!([ca]);
                }
                v["tls"] = t;
            }
            if let Some(path) = &inbound.ws_path {
                assert!(
                    matches!(inbound.kind, InboundKind::Trojan | InboundKind::Vmess),
                    "ws is rendered for trojan and vmess only"
                );
                v["transport"] = json!({ "type": "ws", "path": path });
            }
            v
        })
        .collect();
    json!({
        "log": { "level": "warn", "timestamp": false },
        "inbounds": rendered,
        "outbounds": [{ "type": "direct", "tag": "direct" }],
    })
}

/// sing-box's WireGuard endpoint (sing-box 1.11 and later) with one peer:
/// rurge, whose tunnel address must be in `peer_allowed_ips`, and to which
/// every message carries `reserved` (its `client-id`).
pub struct WireGuardEndpoint {
    pub private_key: [u8; 32],
    /// The endpoint's own tunnel address, as a prefix.
    pub address: String,
    pub peer_public_key: [u8; 32],
    pub peer_allowed_ips: Vec<String>,
    pub reserved: Option<[u8; 3]>,
}

/// The whole configuration for `endpoint` on UDP port `port`, in user space
/// (`system: false`: no interface, no route), and a `mixed` inbound on
/// `ready` that says when sing-box is up — a UDP port cannot be probed.
/// What comes out of the tunnel goes `direct`.
pub fn render_wireguard(endpoint: &WireGuardEndpoint, port: u16, ready: u16) -> Value {
    let key = |k: &[u8; 32]| base64::engine::general_purpose::STANDARD.encode(k);
    let mut peer = json!({
        "public_key": key(&endpoint.peer_public_key),
        "allowed_ips": endpoint.peer_allowed_ips,
    });
    if let Some(reserved) = endpoint.reserved {
        peer["reserved"] = json!(reserved);
    }
    json!({
        "log": { "level": "warn", "timestamp": false },
        "endpoints": [{
            "type": "wireguard",
            "tag": "wg",
            "system": false,
            "address": [endpoint.address],
            "private_key": key(&endpoint.private_key),
            "listen_port": port,
            "peers": [peer],
        }],
        "inbounds": [{ "type": "mixed", "tag": "ready", "listen": "127.0.0.1", "listen_port": ready }],
        "outbounds": [{ "type": "direct", "tag": "direct" }],
    })
}

/// A reference implementation running as a child on the loopback; killed and
/// reaped on drop.
pub(crate) struct Reference {
    what: &'static str,
    child: Child,
    ports: Vec<u16>,
    log: PathBuf,
}

impl Reference {
    /// Starts `command` with its output in `log` and waits until every port
    /// accepts connections.
    pub(crate) fn start(
        what: &'static str,
        mut command: Command,
        ports: Vec<u16>,
        log: PathBuf,
    ) -> Reference {
        let out = std::fs::File::create(&log).expect("create the log");
        let child = command
            .stdin(Stdio::null())
            .stdout(out.try_clone().expect("clone the log handle"))
            .stderr(out)
            .spawn()
            .unwrap_or_else(|e| panic!("cannot start {what}: {e}"));
        let mut running = Reference {
            what,
            child,
            ports,
            log,
        };
        running.wait_ready();
        running
    }

    fn wait_ready(&mut self) {
        let deadline = Instant::now() + READY_TIMEOUT;
        for port in self.ports.clone() {
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            loop {
                // Before the connect, not after: if the child is already dead
                // and an unrelated process happens to hold `port`, a connect
                // that comes first reads as "ready".
                if let Ok(Some(status)) = self.child.try_wait() {
                    panic!(
                        "{} exited early ({status}):\n{}",
                        self.what,
                        self.log_text()
                    );
                }
                if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "{} never listened on {addr}:\n{}",
                    self.what,
                    self.log_text()
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    pub(crate) fn port(&self, index: usize) -> u16 {
        self.ports[index]
    }

    pub(crate) fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for Reference {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A running sing-box; killed and reaped on drop.
pub struct SingBox(Reference);

impl SingBox {
    /// Writes the configuration into `dir`, starts `binary` there and waits
    /// until every inbound accepts connections.
    pub fn spawn(binary: &Path, dir: &Path, inbounds: Vec<Inbound>) -> SingBox {
        let with_ports: Vec<(Inbound, u16)> =
            inbounds.into_iter().map(|i| (i, free_port())).collect();
        let ports: Vec<u16> = with_ports.iter().map(|(_, p)| *p).collect();
        SingBox::launch(binary, dir, &render(&with_ports), ports)
    }

    /// Starts `binary` with `endpoint` on a UDP port of its own; that port,
    /// once sing-box is up.
    pub fn spawn_wireguard(
        binary: &Path,
        dir: &Path,
        endpoint: &WireGuardEndpoint,
    ) -> (SingBox, u16) {
        let (port, ready) = (free_udp_port(), free_port());
        let config = render_wireguard(endpoint, port, ready);
        (SingBox::launch(binary, dir, &config, vec![ready]), port)
    }

    fn launch(binary: &Path, dir: &Path, config: &Value, ports: Vec<u16>) -> SingBox {
        let path = dir.join("sing-box.json");
        std::fs::write(&path, config.to_string()).expect("write the config");
        let mut command = Command::new(binary);
        command.arg("run").arg("-c").arg(&path).arg("-D").arg(dir);
        SingBox(Reference::start(
            "sing-box",
            command,
            ports,
            dir.join("sing-box.log"),
        ))
    }

    /// The loopback port of the `index`-th inbound.
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

    fn every_kind() -> Vec<(Inbound, u16)> {
        vec![
            (
                Inbound {
                    kind: InboundKind::Http,
                    users: vec![("alice".into(), "s3cret".into())],
                    tls: Some(TlsFiles {
                        certificate: "leaf.pem".into(),
                        key: "leaf.key".into(),
                        client_ca: Some("ca.pem".into()),
                    }),
                    ws_path: None,
                },
                1001,
            ),
            (
                Inbound {
                    kind: InboundKind::Socks,
                    users: Vec::new(),
                    tls: None,
                    ws_path: None,
                },
                1002,
            ),
            (
                Inbound {
                    kind: InboundKind::Mixed,
                    users: Vec::new(),
                    tls: None,
                    ws_path: None,
                },
                1003,
            ),
            (
                Inbound {
                    kind: InboundKind::Trojan,
                    users: vec![("u".into(), "pw".into())],
                    tls: Some(TlsFiles {
                        certificate: "leaf.pem".into(),
                        key: "leaf.key".into(),
                        client_ca: None,
                    }),
                    ws_path: Some("/ws".into()),
                },
                1004,
            ),
            (
                Inbound {
                    kind: InboundKind::Vmess,
                    users: vec![("u".into(), "0233d11c-15a4-47d3-ade3-48ffca0ce119".into())],
                    tls: Some(TlsFiles {
                        certificate: "leaf.pem".into(),
                        key: "leaf.key".into(),
                        client_ca: None,
                    }),
                    ws_path: Some("/v".into()),
                },
                1005,
            ),
            (
                Inbound {
                    kind: InboundKind::AnyTls,
                    users: vec![("u".into(), "pw".into())],
                    tls: Some(TlsFiles {
                        certificate: "leaf.pem".into(),
                        key: "leaf.key".into(),
                        client_ca: None,
                    }),
                    ws_path: None,
                },
                1006,
            ),
            (
                Inbound {
                    kind: InboundKind::ShadowTls {
                        version: 3,
                        handshake_port: 1100,
                        detour: 3,
                    },
                    users: vec![("u".into(), "st-pw".into())],
                    tls: None,
                    ws_path: None,
                },
                1007,
            ),
            (
                Inbound {
                    kind: InboundKind::ShadowTls {
                        version: 2,
                        handshake_port: 1100,
                        detour: 3,
                    },
                    users: vec![("ignored".into(), "st-pw".into())],
                    tls: None,
                    ws_path: None,
                },
                1008,
            ),
            (
                Inbound {
                    kind: InboundKind::Shadowsocks {
                        method: "aes-256-gcm",
                    },
                    users: vec![("ignored".into(), "s3same".into())],
                    tls: None,
                    ws_path: None,
                },
                1009,
            ),
            (
                Inbound {
                    kind: InboundKind::Shadowsocks {
                        method: "2022-blake3-aes-128-gcm",
                    },
                    users: vec![
                        ("ignored".into(), "MDEyMzQ1Njc4OWFiY2RlZg==".into()),
                        ("u".into(), "ZmVkY2JhOTg3NjU0MzIxMA==".into()),
                    ],
                    tls: None,
                    ws_path: None,
                },
                1010,
            ),
        ]
    }

    #[test]
    fn the_configuration_never_touches_the_machine() {
        let config = render(&every_kind());
        let text = config.to_string();
        for forbidden in ["set_system_proxy", "tun", "auto_route", "0.0.0.0", "::"] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
        }
        let top: Vec<&String> = config.as_object().unwrap().keys().collect();
        assert_eq!(top, ["inbounds", "log", "outbounds"]);
        for inbound in config["inbounds"].as_array().unwrap() {
            assert_eq!(inbound["listen"], "127.0.0.1");
        }
        assert_eq!(
            config["outbounds"],
            json!([{ "type": "direct", "tag": "direct" }])
        );
    }

    #[test]
    fn inbounds_are_rendered_as_sing_box_spells_them() {
        let config = render(&every_kind());
        let http = &config["inbounds"][0];
        assert_eq!(
            (&http["type"], &http["listen_port"]),
            (&json!("http"), &json!(1001))
        );
        assert_eq!(
            http["users"],
            json!([{ "username": "alice", "password": "s3cret" }])
        );
        assert_eq!(http["tls"]["enabled"], true);
        assert_eq!(http["tls"]["client_authentication"], "require-and-verify");
        assert_eq!(http["tls"]["client_certificate_path"], json!(["ca.pem"]));
        assert_eq!(config["inbounds"][1]["type"], "socks");
        assert!(config["inbounds"][1].get("users").is_none());
        assert_eq!(config["inbounds"][2]["type"], "mixed");
        let trojan = &config["inbounds"][3];
        assert_eq!(trojan["type"], "trojan");
        // trojan users are `name` + `password`, not `username`
        assert_eq!(trojan["users"], json!([{ "name": "u", "password": "pw" }]));
        assert_eq!(trojan["tls"]["enabled"], true);
        assert_eq!(trojan["transport"], json!({ "type": "ws", "path": "/ws" }));
        assert!(config["inbounds"][0].get("transport").is_none());
        let vmess = &config["inbounds"][4];
        assert_eq!(vmess["type"], "vmess");
        // `alterId: 0` is what makes sing-box expect the AEAD handshake
        assert_eq!(
            vmess["users"],
            json!([{ "name": "u", "uuid": "0233d11c-15a4-47d3-ade3-48ffca0ce119", "alterId": 0 }])
        );
        assert_eq!(vmess["transport"], json!({ "type": "ws", "path": "/v" }));
        let anytls = &config["inbounds"][5];
        assert_eq!(anytls["type"], "anytls");
        assert_eq!(anytls["users"], json!([{ "name": "u", "password": "pw" }]));
        assert_eq!(anytls["tls"]["enabled"], true);
        // version 3 has users and a strict mode, version 2 one password
        let v3 = &config["inbounds"][6];
        assert_eq!(
            (&v3["type"], &v3["version"]),
            (&json!("shadowtls"), &json!(3))
        );
        assert_eq!(v3["users"], json!([{ "name": "u", "password": "st-pw" }]));
        assert_eq!(v3["strict_mode"], true);
        assert_eq!(
            v3["handshake"],
            json!({ "server": "127.0.0.1", "server_port": 1100 })
        );
        assert_eq!(v3["detour"], "in-3");
        assert!(v3.get("password").is_none() && v3.get("tls").is_none());
        let v2 = &config["inbounds"][7];
        assert_eq!(
            (&v2["version"], &v2["password"]),
            (&json!(2), &json!("st-pw"))
        );
        assert!(v2.get("users").is_none() && v2.get("strict_mode").is_none());
        assert_eq!(v2["detour"], "in-3");
        // one password; the further users are 2022 users under the server's key
        let ss = &config["inbounds"][8];
        assert_eq!(
            (&ss["type"], &ss["method"], &ss["password"]),
            (
                &json!("shadowsocks"),
                &json!("aes-256-gcm"),
                &json!("s3same")
            )
        );
        assert!(ss.get("users").is_none() && ss.get("network").is_none());
        let multi = &config["inbounds"][9];
        assert_eq!(multi["password"], "MDEyMzQ1Njc4OWFiY2RlZg==");
        assert_eq!(
            multi["users"],
            json!([{ "name": "u", "password": "ZmVkY2JhOTg3NjU0MzIxMA==" }])
        );
    }

    fn endpoint() -> WireGuardEndpoint {
        WireGuardEndpoint {
            private_key: [1; 32],
            address: "10.9.0.1/32".into(),
            peer_public_key: [2; 32],
            peer_allowed_ips: vec!["10.9.0.2/32".into()],
            reserved: Some([1, 2, 3]),
        }
    }

    /// In user space, nothing of the machine's; only the endpoint's UDP port
    /// is on every address.
    #[test]
    fn the_wireguard_configuration_never_touches_the_machine() {
        let config = render_wireguard(&endpoint(), 51820, 1001);
        let text = config.to_string();
        for forbidden in ["set_system_proxy", "tun", "auto_route", "0.0.0.0", "::"] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
        }
        let top: Vec<&String> = config.as_object().unwrap().keys().collect();
        assert_eq!(top, ["endpoints", "inbounds", "log", "outbounds"]);
        assert_eq!(config["endpoints"][0]["system"], false);
        assert_eq!(config["inbounds"][0]["listen"], "127.0.0.1");
        assert_eq!(
            config["outbounds"],
            json!([{ "type": "direct", "tag": "direct" }])
        );
    }

    #[test]
    fn the_wireguard_endpoint_is_rendered_as_sing_box_spells_it() {
        let config = render_wireguard(&endpoint(), 51820, 1001);
        let wg = &config["endpoints"][0];
        assert_eq!(
            (&wg["type"], &wg["listen_port"]),
            (&json!("wireguard"), &json!(51820))
        );
        assert_eq!(wg["address"], json!(["10.9.0.1/32"]));
        assert_eq!(
            wg["private_key"],
            "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE="
        );
        assert_eq!(
            wg["peers"],
            json!([{
                "public_key": "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=",
                "allowed_ips": ["10.9.0.2/32"],
                "reserved": [1, 2, 3],
            }])
        );
        assert_eq!(config["inbounds"][0]["listen_port"], 1001);
    }

    #[test]
    fn free_ports_are_usable() {
        let port = free_port();
        assert!(port > 0);
        TcpListener::bind(("127.0.0.1", port)).expect("the port is free again");
    }
}
