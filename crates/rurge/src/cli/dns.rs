//! `rurge dns lookup` / `rurge dns cache`: resolve names through the
//! profile's DNS settings without running the daemon (M2 design §10.2).

use super::rule::print_diagnostics;
use super::runtime::{RuntimeArgs, Stack, build_stack_with};
use crate::capabilities;
use clap::{Args, Subcommand, ValueEnum};
use rurge_config::config::{LoadOptions, Platform, load};
use rurge_config::general::{DnsServer, EncryptedDns};
use rurge_config::{Config, Diagnostics};
use rurge_dns::{DnsError, DnsResult, LookupOpts, Resolver};
use serde_json::json;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Args)]
pub struct DnsArgs {
    #[command(subcommand)]
    pub command: DnsCommand,
}

#[derive(Subcommand)]
pub enum DnsCommand {
    /// Resolve a name through the profile's DNS settings
    Lookup(LookupArgs),
    /// Resolve the given names, then print this process's cache snapshot
    Cache(CacheArgs),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum QueryType {
    A,
    Aaaa,
    Both,
}

#[derive(Args)]
pub struct CommonArgs {
    /// Profile to load
    #[arg(short = 'c', long = "config", value_name = "FILE")]
    pub config: PathBuf,
    /// Override the profile's upstreams (dns-server / encrypted-dns-server syntax)
    #[arg(long, value_name = "SPEC")]
    pub server: Vec<String>,
    /// Seconds to wait for external resources ([Host] rule sets) to download
    #[arg(long, default_value = "30")]
    pub wait: u64,
    /// JSON output
    #[arg(long)]
    pub json: bool,
    /// Evaluate the profile as if running on this platform
    #[arg(long, value_parser = super::check::parse_platform)]
    pub platform: Option<Platform>,
    #[command(flatten)]
    pub runtime: RuntimeArgs,
}

#[derive(Args)]
pub struct LookupArgs {
    /// Name to resolve
    pub name: String,
    /// Record types to ask for
    #[arg(long = "type", value_enum, default_value = "both")]
    pub qtype: QueryType,
    /// Bypass the cache
    #[arg(long)]
    pub no_cache: bool,
    /// Print every attempt (upstream, record type, outcome, timing) to stderr
    #[arg(long)]
    pub trace: bool,
    #[command(flatten)]
    pub common: CommonArgs,
}

#[derive(Args)]
pub struct CacheArgs {
    /// Names to resolve before printing the snapshot
    pub names: Vec<String>,
    #[command(flatten)]
    pub common: CommonArgs,
}

pub fn run(args: DnsArgs) -> anyhow::Result<ExitCode> {
    match args.command {
        DnsCommand::Lookup(a) => run_lookup(a),
        DnsCommand::Cache(a) => run_cache(a),
    }
}

/// `--server` values follow the `dns-server` key: `system`, `ip[:port]`
/// (`[v6]:port`), or an encrypted URL (`https://`, `tls://`, `tcp://`).
fn parse_servers(specs: &[String]) -> anyhow::Result<(Vec<DnsServer>, Vec<EncryptedDns>)> {
    let mut servers = Vec::new();
    let mut encrypted = Vec::new();
    for spec in specs {
        let s = spec.trim();
        if s.eq_ignore_ascii_case("system") {
            servers.push(DnsServer::System);
        } else if let Some(enc) = EncryptedDns::parse(s) {
            encrypted.push(enc);
        } else if let Ok(sa) = s.parse::<SocketAddr>() {
            servers.push(DnsServer::Udp(sa));
        } else if let Ok(ip) = s.trim_matches(['[', ']']).parse::<IpAddr>() {
            servers.push(DnsServer::Udp(SocketAddr::new(ip, 53)));
        } else {
            anyhow::bail!(
                "invalid --server `{s}` (expected system, ip[:port] or an encrypted DNS URL)"
            );
        }
    }
    Ok((servers, encrypted))
}

/// Loads the profile; `None` means errors were printed and the caller exits 2.
fn load_profile(common: &CommonArgs) -> anyhow::Result<Option<(Config, Diagnostics)>> {
    let platform = common.platform.unwrap_or_else(Platform::current);
    let opts = LoadOptions {
        environment: super::environment(platform, capabilities::CORE_VERSION),
        platform,
        capabilities: capabilities::current(),
    };
    let loaded = load(&common.config, &opts)?;
    if loaded.diagnostics.has_errors() {
        print_diagnostics(&loaded.diagnostics.sorted());
        return Ok(None);
    }
    Ok(Some((loaded.config, loaded.diagnostics)))
}

async fn build(common: &CommonArgs, cfg: &Config) -> anyhow::Result<Stack> {
    let rt = common.runtime.resolve(cfg)?;
    let overrides = (!common.server.is_empty())
        .then(|| parse_servers(&common.server))
        .transpose()?;
    build_stack_with(cfg, &rt, Duration::from_secs(common.wait), move |rc| {
        if let Some((servers, encrypted)) = overrides {
            rc.servers = servers;
            rc.encrypted = encrypted;
        }
    })
    .await
}

/// `--trace`: rurge-dns debug events (one per attempt) on stderr.
fn install_trace() {
    use tracing_subscriber::filter::{LevelFilter, Targets};
    use tracing_subscriber::prelude::*;
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_target(true)
                .without_time(),
        )
        .with(Targets::new().with_target("rurge_dns", LevelFilter::DEBUG))
        .try_init();
}

fn warnings(config_diags: &Diagnostics, stack_diags: &Diagnostics) -> Vec<serde_json::Value> {
    config_diags
        .iter()
        .chain(stack_diags.iter())
        .map(|x| json!({ "code": x.code, "message": x.message }))
        .collect()
}

fn run_lookup(args: LookupArgs) -> anyhow::Result<ExitCode> {
    let Some((cfg, config_diags)) = load_profile(&args.common)? else {
        return Ok(ExitCode::from(2));
    };
    if args.trace {
        install_trace();
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let stack = build(&args.common, &cfg).await?;
        let opts = LookupOpts {
            bypass_cache: args.no_cache,
            want_v6: match args.qtype {
                QueryType::A => Some(false),
                QueryType::Aaaa => Some(true),
                QueryType::Both => None,
            },
        };
        let mut result = stack.resolver.lookup(&args.name, opts).await;
        if args.qtype == QueryType::Aaaa {
            result = match result {
                Ok(r) if r.v6.is_empty() => Err(DnsError::EmptyAnswer),
                Ok(mut r) => {
                    r.v4.clear();
                    Ok(r)
                }
                Err(e) => Err(e),
            };
        }
        let code = match &result {
            Ok(_) => ExitCode::SUCCESS,
            Err(DnsError::EmptyAnswer) => ExitCode::from(1),
            Err(_) => ExitCode::from(2),
        };
        if args.common.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&lookup_json(
                    &args.name,
                    &result,
                    &stack.resolver,
                    &config_diags,
                    &stack.diagnostics
                ))?
            );
        } else {
            print_diagnostics(&config_diags.sorted());
            print_diagnostics(&stack.diagnostics);
            print_lookup(&args.name, &result, &stack.resolver);
        }
        drop(stack);
        Ok(code)
    })
}

fn print_lookup(name: &str, result: &Result<DnsResult, DnsError>, resolver: &Resolver) {
    println!("name: {name}");
    match result {
        Ok(r) => {
            let addrs: Vec<String> = r.addrs().iter().map(ToString::to_string).collect();
            println!(
                "addresses: {}",
                if addrs.is_empty() {
                    "(none)".to_string()
                } else {
                    addrs.join(", ")
                }
            );
            println!("source: {}", r.source);
            println!("ttl: {}s", r.ttl.as_secs());
            println!("elapsed: {:.1}ms", r.elapsed.as_secs_f64() * 1000.0);
        }
        Err(e) => println!("error: {e}"),
    }
    let ups = resolver.primary_upstreams();
    println!(
        "upstreams: {}",
        if ups.is_empty() {
            "(system)".to_string()
        } else {
            ups.join(", ")
        }
    );
}

fn lookup_json(
    name: &str,
    result: &Result<DnsResult, DnsError>,
    resolver: &Resolver,
    config_diags: &Diagnostics,
    stack_diags: &Diagnostics,
) -> serde_json::Value {
    let (addresses, v4, v6, source, ttl, elapsed, error) = match result {
        Ok(r) => (
            r.addrs()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            r.v4.iter().map(ToString::to_string).collect::<Vec<_>>(),
            r.v6.iter().map(ToString::to_string).collect::<Vec<_>>(),
            Some(r.source.to_string()),
            Some(r.ttl.as_secs()),
            Some(r.elapsed.as_secs_f64() * 1000.0),
            None,
        ),
        Err(e) => (
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
            Some(e.to_string()),
        ),
    };
    json!({
        "name": name,
        "addresses": addresses,
        "v4": v4,
        "v6": v6,
        "source": source,
        "ttl_secs": ttl,
        "elapsed_ms": elapsed,
        "error": error,
        "upstreams": resolver.primary_upstreams(),
        "warnings": warnings(config_diags, stack_diags),
    })
}

fn run_cache(args: CacheArgs) -> anyhow::Result<ExitCode> {
    let Some((cfg, config_diags)) = load_profile(&args.common)? else {
        return Ok(ExitCode::from(2));
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let stack = build(&args.common, &cfg).await?;
        for name in &args.names {
            if let Err(e) = stack.resolver.lookup(name, LookupOpts::default()).await {
                eprintln!("warning: {name}: {e}");
            }
        }
        let snapshot = stack.resolver.cache_snapshot();
        if args.common.json {
            let entries: Vec<serde_json::Value> = snapshot
                .iter()
                .map(|e| {
                    json!({
                        "name": e.name,
                        "v4": e.v4.iter().map(ToString::to_string).collect::<Vec<_>>(),
                        "v6": e.v6.iter().map(ToString::to_string).collect::<Vec<_>>(),
                        "expires_in_secs": e.expires_in.map(|d| d.as_secs()),
                        "stale": e.stale,
                        "negative": e.negative,
                        "source": e.source,
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "entries": entries,
                    "warnings": warnings(&config_diags, &stack.diagnostics),
                }))?
            );
        } else {
            print_diagnostics(&config_diags.sorted());
            print_diagnostics(&stack.diagnostics);
            println!("entries: {}", snapshot.len());
            for e in &snapshot {
                let state = if e.negative {
                    "negative".to_string()
                } else if e.stale {
                    "stale".to_string()
                } else {
                    match e.expires_in {
                        Some(d) => format!("{}s", d.as_secs()),
                        None => "expired".to_string(),
                    }
                };
                let mut addrs: Vec<String> = e.v4.iter().map(ToString::to_string).collect();
                addrs.extend(e.v6.iter().map(ToString::to_string));
                println!(
                    "{:<40} {:<10} {:<28} {}",
                    e.name,
                    state,
                    e.source,
                    addrs.join(", ")
                );
            }
        }
        drop(stack);
        Ok(ExitCode::SUCCESS)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::general::EncryptedDnsScheme;

    #[test]
    fn server_specs_follow_the_dns_server_syntax() {
        let specs: Vec<String> = [
            "system",
            "1.1.1.1",
            "8.8.8.8:5353",
            "[2001:db8::1]:53",
            "tcp://dns.example",
            "https://dns.example/dns-query",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (servers, encrypted) = parse_servers(&specs).unwrap();
        assert_eq!(
            servers,
            vec![
                DnsServer::System,
                DnsServer::Udp("1.1.1.1:53".parse().unwrap()),
                DnsServer::Udp("8.8.8.8:5353".parse().unwrap()),
                DnsServer::Udp("[2001:db8::1]:53".parse().unwrap()),
            ]
        );
        assert_eq!(encrypted.len(), 2);
        assert_eq!(encrypted[0].scheme, EncryptedDnsScheme::Tcp);
        assert_eq!(encrypted[1].scheme, EncryptedDnsScheme::Https);
        let err = parse_servers(&["nonsense".to_string()]).unwrap_err();
        assert!(err.to_string().contains("invalid --server"), "{err}");
    }
}
