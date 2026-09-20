//! An xray child process on the loopback, for the VMess interoperability
//! tests only: VMess was defined by this family of implementations, and
//! sing-box's is a rewrite, so rurge's hand-written codec is checked against
//! both (M2 design, M2-D5). The same rules as for sing-box apply: nothing is
//! downloaded or installed here, the rendered configuration listens on
//! 127.0.0.1 only, and its single outbound is `freedom`.

use crate::{REQUIRED_ENV, Reference, free_port};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BINARY_ENV: &str = "RURGE_TEST_XRAY";

/// `RURGE_TEST_XRAY`, else the first `xray` on `PATH`.
pub fn locate() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BINARY_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let name = if cfg!(windows) { "xray.exe" } else { "xray" };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure instead.
pub fn xray_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if std::env::var(REQUIRED_ENV).as_deref() == Ok("1") {
        panic!("{REQUIRED_ENV}=1 but no xray was found ({BINARY_ENV} or PATH)");
    }
    eprintln!("skipping {test}: no xray ({BINARY_ENV} or PATH); see tests/interop/README.md");
    None
}

/// A VMess inbound. xray only knows the AEAD handshake.
pub struct XrayInbound {
    pub uuid: String,
    /// A WebSocket transport on this path.
    pub ws_path: Option<String>,
}

/// The whole configuration for `inbounds`, each on its loopback port.
pub fn render(inbounds: &[(XrayInbound, u16)]) -> Value {
    let rendered: Vec<Value> = inbounds
        .iter()
        .enumerate()
        .map(|(i, (inbound, port))| {
            let mut v = json!({
                "tag": format!("in-{i}"),
                "listen": "127.0.0.1",
                "port": port,
                "protocol": "vmess",
                "settings": { "clients": [{ "id": inbound.uuid }] },
            });
            if let Some(path) = &inbound.ws_path {
                v["streamSettings"] = json!({ "network": "ws", "wsSettings": { "path": path } });
            }
            v
        })
        .collect();
    json!({
        "log": { "loglevel": "warning" },
        "inbounds": rendered,
        "outbounds": [{ "protocol": "freedom", "tag": "direct" }],
    })
}

/// A running xray; killed and reaped on drop.
pub struct Xray(Reference);

impl Xray {
    /// Writes the configuration into `dir`, starts `binary` there and waits
    /// until every inbound accepts connections.
    pub fn spawn(binary: &Path, dir: &Path, inbounds: Vec<XrayInbound>) -> Xray {
        let with_ports: Vec<(XrayInbound, u16)> =
            inbounds.into_iter().map(|i| (i, free_port())).collect();
        let ports: Vec<u16> = with_ports.iter().map(|(_, p)| *p).collect();
        let config = dir.join("xray.json");
        std::fs::write(&config, render(&with_ports).to_string()).expect("write the config");
        let mut command = Command::new(binary);
        command.arg("run").arg("-c").arg(&config).current_dir(dir);
        Xray(Reference::start(
            "xray",
            command,
            ports,
            dir.join("xray.log"),
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

    fn both() -> Vec<(XrayInbound, u16)> {
        let uuid = "0233d11c-15a4-47d3-ade3-48ffca0ce119".to_string();
        vec![
            (
                XrayInbound {
                    uuid: uuid.clone(),
                    ws_path: None,
                },
                2001,
            ),
            (
                XrayInbound {
                    uuid,
                    ws_path: Some("/v".into()),
                },
                2002,
            ),
        ]
    }

    #[test]
    fn the_configuration_never_touches_the_machine() {
        let config = render(&both());
        let text = config.to_string();
        for forbidden in [
            "set_system_proxy",
            "tun",
            "auto_route",
            "0.0.0.0",
            "::",
            "dokodemo",
            "sockopt",
        ] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
        }
        let top: Vec<&String> = config.as_object().unwrap().keys().collect();
        assert_eq!(top, ["inbounds", "log", "outbounds"]);
        for inbound in config["inbounds"].as_array().unwrap() {
            assert_eq!(inbound["listen"], "127.0.0.1");
            assert_eq!(inbound["protocol"], "vmess");
        }
        assert_eq!(
            config["outbounds"],
            json!([{ "protocol": "freedom", "tag": "direct" }])
        );
    }

    #[test]
    fn inbounds_are_rendered_as_xray_spells_them() {
        let config = render(&both());
        let plain = &config["inbounds"][0];
        assert_eq!(plain["port"], 2001);
        assert_eq!(
            plain["settings"],
            json!({ "clients": [{ "id": "0233d11c-15a4-47d3-ade3-48ffca0ce119" }] })
        );
        assert!(plain.get("streamSettings").is_none());
        assert_eq!(
            config["inbounds"][1]["streamSettings"],
            json!({ "network": "ws", "wsSettings": { "path": "/v" } })
        );
    }
}
