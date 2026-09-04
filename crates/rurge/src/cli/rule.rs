//! `rurge rule match`: evaluate the rule engine for a hypothetical session
//! without running the daemon (M2 design §10.1).

use super::runtime::{RuntimeArgs, SystemLazyResolver, build_stack};
use crate::capabilities;
use anyhow::Context;
use clap::{Args, Subcommand};
use rurge_config::config::{LoadOptions, Platform, load};
use rurge_config::diagnostic::Severity;
use rurge_config::rule::PolicyRef;
use rurge_config::rule::ProtocolKind;
use rurge_config::session::{ListenerKind, ProcessInfo, SessionInfo, Transport};
use rurge_config::{Diagnostics, HostName};
use rurge_rules::engine::{FixedResolve, LazyResolver, NoResolve, TraceStep};
use rurge_rules::matcher::ResolvedAddrs;
use rurge_rules::{Decision, OutboundMode, Outcome, RuleEngine};
use serde_json::json;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Args)]
pub struct RuleArgs {
    #[command(subcommand)]
    pub command: RuleCommand,
}

#[derive(Subcommand)]
pub enum RuleCommand {
    /// Evaluate the rules of a profile for a hypothetical session
    Match(MatchArgs),
}

#[derive(Args)]
pub struct MatchArgs {
    /// Profile to load
    #[arg(short = 'c', long = "config", value_name = "FILE")]
    pub config: PathBuf,
    /// Destination host[:port] — a domain, an IPv4, or [IPv6]
    pub target: String,
    /// Full URL of the request (URL-REGEX rules)
    #[arg(long)]
    pub url: Option<String>,
    /// Source address of the client
    #[arg(long, value_name = "IP:PORT")]
    pub src: Option<SocketAddr>,
    /// Listening port that accepted the session (IN-PORT rules)
    #[arg(long)]
    pub in_port: Option<u16>,
    /// Sniffed protocol: http, https, tcp, udp, quic, stun, ...
    #[arg(long, value_parser = parse_protocol)]
    pub protocol: Option<ProtocolKind>,
    /// TLS SNI (extended-matching)
    #[arg(long)]
    pub sni: Option<String>,
    /// HTTP Host header (extended-matching)
    #[arg(long)]
    pub http_host: Option<String>,
    /// User-Agent header
    #[arg(long)]
    pub user_agent: Option<String>,
    /// Originating process name or full path
    #[arg(long, value_name = "NAME|PATH")]
    pub process: Option<String>,
    /// Treat the session as UDP
    #[arg(long)]
    pub udp: bool,
    /// Listener kind: http, socks5, tun, forward
    #[arg(long, value_parser = parse_listener, default_value = "http")]
    pub listener: ListenerKind,
    /// Outbound mode: direct, proxy=<policy>, rule
    #[arg(long, value_parser = parse_mode, default_value = "rule")]
    pub mode: OutboundMode,
    /// Use these addresses instead of resolving the destination
    #[arg(long, value_delimiter = ',', conflicts_with = "no_dns")]
    pub resolve: Vec<IpAddr>,
    /// Make every DNS lookup fail (exercise dns-failed)
    #[arg(long)]
    pub no_dns: bool,
    /// Seconds to wait for external resources to download
    #[arg(long, default_value = "30")]
    pub wait: u64,
    /// Print every evaluated rule
    #[arg(long)]
    pub explain: bool,
    /// JSON output
    #[arg(long)]
    pub json: bool,
    /// Evaluate the profile as if running on this platform
    #[arg(long, value_parser = super::check::parse_platform)]
    pub platform: Option<Platform>,
    #[command(flatten)]
    pub runtime: RuntimeArgs,
}

fn parse_protocol(s: &str) -> Result<ProtocolKind, String> {
    ProtocolKind::parse(s).ok_or_else(|| format!("unknown protocol `{s}`"))
}

fn parse_listener(s: &str) -> Result<ListenerKind, String> {
    match s.to_ascii_lowercase().as_str() {
        "http" => Ok(ListenerKind::Http),
        "socks5" => Ok(ListenerKind::Socks5),
        "tun" => Ok(ListenerKind::Tun),
        "forward" => Ok(ListenerKind::Forward),
        other => Err(format!(
            "unknown listener `{other}` (expected http, socks5, tun or forward)"
        )),
    }
}

fn parse_mode(s: &str) -> Result<OutboundMode, String> {
    match s.to_ascii_lowercase().as_str() {
        "direct" => Ok(OutboundMode::Direct),
        "rule" => Ok(OutboundMode::Rule),
        other => match other.strip_prefix("proxy=") {
            Some(p) if !p.is_empty() => Ok(OutboundMode::Proxy(PolicyRef::parse(&s[6..]))),
            _ => Err(format!(
                "unknown mode `{s}` (expected direct, rule or proxy=<policy>)"
            )),
        },
    }
}

/// `host[:port]`, with `[v6]:port` for IPv6. The default port follows the URL scheme, else 443.
fn parse_target(target: &str, url: Option<&str>) -> anyhow::Result<(HostName, u16)> {
    let default_port = match url {
        Some(u) if u.starts_with("http://") => 80,
        _ => 443,
    };
    let (host, port) = if let Some(rest) = target.strip_prefix('[') {
        let (h, tail) = rest.split_once(']').context("unterminated IPv6 literal")?;
        (h.to_string(), tail.strip_prefix(':').map(str::to_string))
    } else if target.matches(':').count() == 1 {
        let (h, p) = target.split_once(':').expect("one colon");
        (h.to_string(), Some(p.to_string()))
    } else {
        (target.to_string(), None)
    };
    let port = match port {
        Some(p) => p
            .parse::<u16>()
            .with_context(|| format!("invalid port `{p}`"))?,
        None => default_port,
    };
    Ok((HostName::parse(&host), port))
}

fn build_session(args: &MatchArgs) -> anyhow::Result<SessionInfo> {
    let (host, port) = parse_target(&args.target, args.url.as_deref())?;
    let mut s = SessionInfo::tcp(host, port);
    if let Some(src) = args.src {
        s.src = src;
    }
    s.in_port = args.in_port.unwrap_or(0);
    s.listener = args.listener;
    s.transport = if args.udp {
        Transport::Udp
    } else {
        Transport::Tcp
    };
    s.protocol = args.protocol;
    s.sni = args.sni.as_ref().map(|v| v.to_ascii_lowercase());
    s.http_host = args.http_host.as_ref().map(|v| v.to_ascii_lowercase());
    s.user_agent = args.user_agent.clone();
    s.url = args.url.clone();
    s.process = args.process.as_ref().map(|p| {
        let name = p.rsplit(['/', '\\']).next().unwrap_or(p).to_string();
        let path = (p.contains('/') || p.contains('\\')).then(|| p.clone());
        ProcessInfo { name, path }
    });
    Ok(s)
}

fn print_diagnostics(diags: &Diagnostics) {
    for d in diags.iter() {
        let level = match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "note",
        };
        eprintln!("{level}[{}]: {}", d.code, d.message);
    }
}

pub fn run(args: RuleArgs) -> anyhow::Result<ExitCode> {
    match args.command {
        RuleCommand::Match(m) => run_match(m),
    }
}

fn run_match(args: MatchArgs) -> anyhow::Result<ExitCode> {
    let platform = args.platform.unwrap_or_else(Platform::current);
    let opts = LoadOptions {
        environment: super::environment(platform, capabilities::CORE_VERSION),
        platform,
        capabilities: capabilities::current(),
    };
    let loaded = load(&args.config, &opts)?;
    if loaded.diagnostics.has_errors() {
        print_diagnostics(&loaded.diagnostics.sorted());
        return Ok(ExitCode::from(2));
    }
    let config_diagnostics = loaded.diagnostics;
    let cfg = loaded.config;
    let rt = args.runtime.resolve(&cfg)?;
    let session = build_session(&args)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let stack = build_stack(&cfg, &rt, Duration::from_secs(args.wait)).await?;
        let engine =
            RuleEngine::build_with_registry(&cfg, stack.registry.clone(), stack.geo.clone())?;
        let resolver: Box<dyn LazyResolver> = if args.no_dns {
            Box::new(NoResolve)
        } else if !args.resolve.is_empty() {
            let mut fixed = ResolvedAddrs::default();
            for ip in &args.resolve {
                match ip {
                    IpAddr::V4(v) => fixed.v4.push(*v),
                    IpAddr::V6(v) => fixed.v6.push(*v),
                }
            }
            Box::new(FixedResolve(fixed))
        } else {
            Box::new(SystemLazyResolver)
        };
        let (decision, trace) = if args.explain {
            engine
                .evaluate_traced(&session, args.mode.clone(), resolver.as_ref())
                .await
        } else {
            (
                engine
                    .evaluate(&session, args.mode.clone(), resolver.as_ref())
                    .await,
                Vec::new(),
            )
        };
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&to_json(
                    &engine,
                    &decision,
                    &trace,
                    &config_diagnostics,
                    &stack.diagnostics
                ))?
            );
        } else {
            print_diagnostics(&config_diagnostics.sorted());
            print_diagnostics(&stack.diagnostics);
            print_text(&engine, &decision, &trace);
        }
        drop(stack);
        Ok(match decision.outcome {
            Outcome::Policy(_) => ExitCode::SUCCESS,
            Outcome::DnsFailed => ExitCode::from(1),
        })
    })
}

fn rule_raw(engine: &RuleEngine, index: usize) -> String {
    engine
        .rules()
        .iter()
        .find(|r| r.index == index)
        .map(|r| r.raw.clone())
        .unwrap_or_default()
}

fn print_text(engine: &RuleEngine, d: &Decision, trace: &[TraceStep]) {
    match &d.outcome {
        Outcome::Policy(p) => println!("policy: {p}"),
        Outcome::DnsFailed => println!("policy: (none) DNS lookup failed"),
    }
    println!("reason: {}", d.reason.as_str());
    if let Some(i) = d.matched {
        println!("rule #{i}: {}", rule_raw(engine, i));
    }
    if let Some(hit) = &d.sub_rule {
        println!("sub-rule: {} (in {})", hit.entry, hit.set);
    }
    if let Some(r) = &d.resolved {
        let mut all: Vec<String> = r.v4.iter().map(|a| a.to_string()).collect();
        all.extend(r.v6.iter().map(|a| a.to_string()));
        println!("resolved: {}", all.join(", "));
    }
    for n in &d.notes {
        println!("note: {n}");
    }
    if !trace.is_empty() {
        println!("trace:");
        for t in trace {
            println!(
                "  #{} {:<13} {}",
                t.rule,
                t.verdict,
                rule_raw(engine, t.rule)
            );
        }
    }
}

fn to_json(
    engine: &RuleEngine,
    d: &Decision,
    trace: &[TraceStep],
    config_diags: &Diagnostics,
    stack_diags: &Diagnostics,
) -> serde_json::Value {
    let warnings: Vec<_> = config_diags
        .iter()
        .chain(stack_diags.iter())
        .map(|x| json!({ "code": x.code, "message": x.message }))
        .collect();
    json!({
        "policy": d.policy().map(|p| p.to_string()),
        "reason": d.reason.as_str(),
        "matched": d.matched.map(|i| json!({ "index": i, "raw": rule_raw(engine, i) })),
        "sub_rule": d.sub_rule.as_ref().map(|h| json!({ "set": h.set, "entry": h.entry })),
        "resolved": d.resolved.as_ref().map(|r| json!({
            "v4": r.v4.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
            "v6": r.v6.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
        })),
        "notes": d.notes,
        "warnings": warnings,
        "trace": trace.iter().map(|t| json!({
            "rule": t.rule,
            "verdict": t.verdict,
            "raw": rule_raw(engine, t.rule),
            "elapsed_us": t.elapsed.as_micros(),
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_parsing_handles_ports_and_ipv6() {
        assert_eq!(
            parse_target("example.com", None).unwrap(),
            (HostName::parse("example.com"), 443)
        );
        assert_eq!(
            parse_target("example.com:8080", None).unwrap(),
            (HostName::parse("example.com"), 8080)
        );
        assert_eq!(
            parse_target("example.com", Some("http://example.com/"))
                .unwrap()
                .1,
            80
        );
        assert_eq!(
            parse_target("[::1]:53", None).unwrap(),
            (HostName::parse("::1"), 53)
        );
        assert_eq!(
            parse_target("::1", None).unwrap(),
            (HostName::parse("::1"), 443)
        );
        assert!(parse_target("example.com:99999", None).is_err());
    }

    #[test]
    fn mode_parsing() {
        assert_eq!(parse_mode("direct").unwrap(), OutboundMode::Direct);
        assert_eq!(
            parse_mode("proxy=MyGroup").unwrap(),
            OutboundMode::Proxy(PolicyRef::Named("MyGroup".into()))
        );
        assert!(parse_mode("proxy=").is_err());
        assert!(parse_mode("global").is_err());
    }
}
