//! `rurge reload | stop | status`: clients of the daemon's HTTP API (M4 §7).

use super::api_client::{ApiClient, ClientError};
use crate::capabilities;
use anyhow::{Context, anyhow};
use clap::Args;
use rurge_config::config::{LoadOptions, Platform, load};
use serde_json::{Value, json};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;

pub const NOT_CONFIGURED: &str = "http-api is not configured; add \"http-api = <key>@127.0.0.1:6171\" to [General] or pass --remote/--key (reload can also be triggered by SIGHUP or --watch)";

#[derive(Args, Clone, Debug)]
pub struct ControlArgs {
    /// Profile whose [General] http-api names the daemon
    #[arg(short = 'c', long = "config", value_name = "FILE")]
    pub config: Option<PathBuf>,
    /// API address, e.g. 127.0.0.1:6171 (overrides the profile)
    #[arg(long, value_name = "HOST:PORT")]
    pub remote: Option<SocketAddr>,
    /// API key (overrides the profile)
    #[arg(long, env = "RURGE_API_KEY", value_name = "KEY")]
    pub key: Option<String>,
    /// Evaluate the profile as if running on this platform
    #[arg(long, value_parser = super::check::parse_platform)]
    pub platform: Option<Platform>,
}

#[derive(Args)]
pub struct StatusArgs {
    #[command(flatten)]
    pub control: ControlArgs,
    /// Print the raw API responses as one JSON object
    #[arg(long)]
    pub json: bool,
}

/// A wildcard bind is reached on the loopback of the same family.
pub(crate) fn connect_addr(addr: SocketAddr) -> SocketAddr {
    match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            SocketAddr::new(Ipv4Addr::LOCALHOST.into(), addr.port())
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(Ipv6Addr::LOCALHOST.into(), addr.port())
        }
        _ => addr,
    }
}

/// `--remote` + `--key` → profile `http-api` (only parsed) → `NOT_CONFIGURED`.
pub fn resolve_endpoint(args: &ControlArgs) -> anyhow::Result<(SocketAddr, String)> {
    if let Some(remote) = args.remote {
        let key = args
            .key
            .clone()
            .ok_or_else(|| anyhow!("--remote needs --key (or RURGE_API_KEY)"))?;
        return Ok((connect_addr(remote), key));
    }
    let Some(config) = &args.config else {
        return Err(anyhow!("{NOT_CONFIGURED}"));
    };
    let platform = args.platform.unwrap_or_else(Platform::current);
    let opts = LoadOptions {
        environment: super::environment(platform, capabilities::CORE_VERSION),
        platform,
        capabilities: capabilities::current(),
    };
    let loaded =
        load(config, &opts).with_context(|| format!("cannot read {}", config.display()))?;
    match &loaded.config.general.http_api {
        Some(api) => Ok((
            connect_addr(api.addr),
            args.key.clone().unwrap_or_else(|| api.key.clone()),
        )),
        None => Err(anyhow!("{NOT_CONFIGURED}")),
    }
}

fn client(args: &ControlArgs) -> anyhow::Result<ApiClient> {
    let (addr, key) = resolve_endpoint(args)?;
    Ok(ApiClient::new(addr, key))
}

/// Maps transport failures and non-2xx answers to the exit codes of §7;
/// `Ok(body)` only for 2xx.
fn expect_2xx(
    client: &ApiClient,
    result: Result<(u16, Value), ClientError>,
) -> Result<Value, ExitCode> {
    match result {
        Ok((status, body)) if (200..300).contains(&status) => Ok(body),
        Ok((status, body)) => {
            let message = body["error"].as_str().unwrap_or("").to_string();
            eprintln!(
                "error: rurge at {} answered {status}: {message}",
                client.base()
            );
            Err(ExitCode::from(if status == 401 || status == 403 {
                2
            } else {
                1
            }))
        }
        Err(e) => {
            eprintln!("error: cannot reach rurge at {}: {e}", client.base());
            Err(ExitCode::from(1))
        }
    }
}

fn block_on<T>(fut: impl std::future::Future<Output = T>) -> anyhow::Result<T> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    Ok(runtime.block_on(fut))
}

pub fn reload(args: ControlArgs) -> anyhow::Result<ExitCode> {
    let client = client(&args)?;
    block_on(async {
        let body = match expect_2xx(&client, client.post("/v1/profiles/reload", json!({})).await) {
            Ok(b) => b,
            Err(code) => return code,
        };
        let errors = body["errors"].as_u64().unwrap_or(0);
        let warnings = body["warnings"].as_u64().unwrap_or(0);
        if body["ok"].as_bool().unwrap_or(false) {
            println!("reloaded: {errors} error(s), {warnings} warning(s)");
            ExitCode::SUCCESS
        } else {
            eprintln!("reload failed: {errors} error(s); the running configuration is unchanged");
            ExitCode::from(1)
        }
    })
}

pub fn stop(args: ControlArgs) -> anyhow::Result<ExitCode> {
    let client = client(&args)?;
    block_on(async {
        let result = client.post("/v1/stop", json!({})).await;
        // §7: the daemon answers `{}` and then exits, so it may close the
        // connection before the body is fully read. Behind a 2xx status that
        // still means the stop was accepted.
        if let Err(ClientError::BodyLost { status }) = &result
            && (200..300).contains(status)
        {
            println!("stop requested");
            return ExitCode::SUCCESS;
        }
        match expect_2xx(&client, result) {
            Ok(_) => {
                println!("stop requested");
                ExitCode::SUCCESS
            }
            Err(code) => code,
        }
    })
}

fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut value = n as f64 / 1024.0;
    let mut unit = UNITS[0];
    for u in &UNITS[1..] {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = u;
    }
    format!("{value:.1} {unit}")
}

/// The four-line status block of design §7.
fn status_text(base: &str, v: &Value) -> String {
    let mode = v["outbound"]["mode"].as_str().unwrap_or("?");
    let global = v["global"]["policy"].as_str().unwrap_or("none");
    let policies = v["policies"]["proxies"].as_array().map_or(0, Vec::len)
        + v["policies"]["policy-groups"]
            .as_array()
            .map_or(0, Vec::len);
    let rules = v["rules"]["rules"].as_array().map_or(0, Vec::len);
    let active = v["requests"]["requests"].as_array().map_or(0, Vec::len);
    let t = &v["traffic"]["total"];
    let n = |k: &str| t[k].as_u64().unwrap_or(0);
    format!(
        "rurge at {base}\nmode: {mode} (global policy: {global})\npolicies: {policies}   rules: {rules}   active requests: {active}\ntraffic: in {}, out {} (in {}/s, out {}/s)\n",
        fmt_bytes(n("in")),
        fmt_bytes(n("out")),
        fmt_bytes(n("inCurrentSpeed")),
        fmt_bytes(n("outCurrentSpeed")),
    )
}

pub fn status(args: StatusArgs) -> anyhow::Result<ExitCode> {
    let client = client(&args.control)?;
    block_on(async {
        let mut combined = serde_json::Map::new();
        for (name, path) in [
            ("outbound", "/v1/outbound"),
            ("global", "/v1/outbound/global"),
            ("policies", "/v1/policies"),
            ("rules", "/v1/rules"),
            ("requests", "/v1/requests/active"),
            ("traffic", "/v1/traffic"),
        ] {
            match expect_2xx(&client, client.get(path).await) {
                Ok(body) => {
                    combined.insert(name.to_string(), body);
                }
                Err(code) => return code,
            }
        }
        let combined = Value::Object(combined);
        if args.json {
            println!("{}", serde_json::to_string_pretty(&combined).expect("json"));
        } else {
            print!("{}", status_text(client.base(), &combined));
        }
        ExitCode::SUCCESS
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_binds_are_reached_on_loopback() {
        assert_eq!(
            connect_addr("0.0.0.0:6171".parse().unwrap()).to_string(),
            "127.0.0.1:6171"
        );
        assert_eq!(
            connect_addr("[::]:6171".parse().unwrap()).to_string(),
            "[::1]:6171"
        );
        assert_eq!(
            connect_addr("10.0.0.2:6171".parse().unwrap()).to_string(),
            "10.0.0.2:6171"
        );
    }

    #[test]
    fn bytes_are_human_readable() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(1023), "1023 B");
        assert_eq!(fmt_bytes(1024), "1.0 KiB");
        assert_eq!(fmt_bytes(12_897_484), "12.3 MiB");
        assert_eq!(fmt_bytes(5 * 1024 * 1024 * 1024), "5.0 GiB");
    }

    #[test]
    fn status_text_matches_the_design() {
        let text = status_text(
            "http://127.0.0.1:6171",
            &serde_json::json!({
                "outbound": {"mode": "rule"},
                "global": {"policy": null},
                "policies": {"proxies": ["DIRECT", "HK"], "policy-groups": ["Pick"]},
                "rules": {"rules": [{}, {}, {}]},
                "requests": {"requests": [{}]},
                "traffic": {"total": {"in": 12_897_484, "out": 1_153_434, "inCurrentSpeed": 8397, "outCurrentSpeed": 614}}
            }),
        );
        assert_eq!(
            text,
            "rurge at http://127.0.0.1:6171\nmode: rule (global policy: none)\npolicies: 3   rules: 3   active requests: 1\ntraffic: in 12.3 MiB, out 1.1 MiB (in 8.2 KiB/s, out 614 B/s)\n"
        );
    }

    #[test]
    fn endpoint_resolution_prefers_flags_then_config() {
        let dir = tempfile::tempdir().unwrap();
        let with = dir.path().join("with.conf");
        std::fs::write(
            &with,
            "[General]\nhttp-api = abc@0.0.0.0:6171\n[Rule]\nFINAL,DIRECT\n",
        )
        .unwrap();
        let without = dir.path().join("without.conf");
        std::fs::write(&without, "[General]\n[Rule]\nFINAL,DIRECT\n").unwrap();
        let args = |config: Option<&std::path::Path>, remote: Option<&str>, key: Option<&str>| {
            ControlArgs {
                config: config.map(|p| p.to_path_buf()),
                remote: remote.map(|r| r.parse().unwrap()),
                key: key.map(str::to_string),
                platform: None,
            }
        };
        let (addr, key) = resolve_endpoint(&args(None, Some("10.0.0.2:1"), Some("k"))).unwrap();
        assert_eq!(
            (addr.to_string().as_str(), key.as_str()),
            ("10.0.0.2:1", "k")
        );
        assert!(
            resolve_endpoint(&args(None, Some("10.0.0.2:1"), None))
                .unwrap_err()
                .to_string()
                .contains("--key")
        );
        let (addr, key) = resolve_endpoint(&args(Some(&with), None, None)).unwrap();
        assert_eq!(
            (addr.to_string().as_str(), key.as_str()),
            ("127.0.0.1:6171", "abc")
        );
        let (_, key) = resolve_endpoint(&args(Some(&with), None, Some("override"))).unwrap();
        assert_eq!(key, "override");
        let e = resolve_endpoint(&args(Some(&without), None, None))
            .unwrap_err()
            .to_string();
        assert_eq!(e, NOT_CONFIGURED);
        assert_eq!(
            resolve_endpoint(&args(None, None, None))
                .unwrap_err()
                .to_string(),
            NOT_CONFIGURED
        );
    }
}
