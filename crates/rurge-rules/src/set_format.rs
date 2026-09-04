//! Text formats of RULE-SET / DOMAIN-SET files (M2 design §6.2) and the
//! built-in `SYSTEM` / `LAN` sets (manual `rules/ruleset.html`, Internal Rule Sets).

use rurge_config::rule::{InternalSet, ParseCtx, SubRule, parse_subrule};

/// Manual: "A set may contain at most 1,000,000 entries." rurge truncates and warns.
pub const MAX_ENTRIES: usize = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SetKind {
    RuleSet,
    DomainSet,
}

impl SetKind {
    pub fn keyword(self) -> &'static str {
        match self {
            SetKind::RuleSet => "RULE-SET",
            SetKind::DomainSet => "DOMAIN-SET",
        }
    }
}

#[derive(Clone, Debug)]
pub enum SetLine {
    Rule(SubRule),
    /// `.example.com` → `suffix = true` (matches the name and all subdomains);
    /// `example.com` → exact.
    Domain {
        name: String,
        suffix: bool,
    },
}

#[derive(Debug, Default)]
pub struct ParsedSet {
    pub lines: Vec<SetLine>,
    /// `(1-based line number, reason)` for every skipped line.
    pub skipped: Vec<(usize, String)>,
    /// Number of valid lines dropped because `limit` was reached.
    pub truncated: usize,
}

pub fn parse_set(kind: SetKind, text: &str, ctx: &ParseCtx) -> ParsedSet {
    parse_set_with_limit(kind, text, ctx, MAX_ENTRIES)
}

pub fn parse_set_with_limit(kind: SetKind, text: &str, ctx: &ParseCtx, limit: usize) -> ParsedSet {
    let mut out = ParsedSet::default();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || is_comment(kind, line) {
            continue;
        }
        let parsed = match kind {
            SetKind::RuleSet => parse_rule_line(line, ctx),
            SetKind::DomainSet => parse_domain_line(line),
        };
        match parsed {
            Ok(l) if out.lines.len() < limit => out.lines.push(l),
            Ok(_) => out.truncated += 1,
            Err(reason) => out.skipped.push((i + 1, reason)),
        }
    }
    out
}

fn is_comment(kind: SetKind, line: &str) -> bool {
    line.starts_with('#')
        || line.starts_with("//")
        || (kind == SetKind::RuleSet && line.starts_with(';'))
}

fn parse_rule_line(line: &str, ctx: &ParseCtx) -> Result<SetLine, String> {
    // parse_subrule rejects FINAL and pre-matching before constructing a SubRule;
    // their errors surface as skipped lines in parse_set_with_limit.
    let sub = parse_subrule(line, ctx).map_err(|e| e.message)?;
    Ok(SetLine::Rule(sub))
}

fn parse_domain_line(line: &str) -> Result<SetLine, String> {
    let (name, suffix) = match line.strip_prefix('.') {
        Some(rest) => (rest, true),
        None => (line, false),
    };
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    let valid = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
        && !name.starts_with('.')
        && !name.contains("..");
    if !valid {
        return Err(format!("invalid DOMAIN-SET line `{line}`"));
    }
    Ok(SetLine::Domain { name, suffix })
}

/// Manual list as of Surge Mac 6.9 / iOS 5.22; the app's own list is authoritative.
const SYSTEM_SET: &str = "\
DOMAIN,api.smoot.apple.com
DOMAIN,captive.apple.com
DOMAIN,xp.apple.com
DOMAIN,configuration.apple.com
DOMAIN,guzzoni.apple.com
DOMAIN,smp-device-content.apple.com
DOMAIN,aod.itunes.apple.com
DOMAIN,mesu.apple.com
DOMAIN,api.smoot.apple.cn
DOMAIN,gs-loc.apple.com
DOMAIN,mvod.itunes.apple.com
DOMAIN,streamingaudio.itunes.apple.com
DOMAIN-SUFFIX,ess.apple.com
DOMAIN-SUFFIX,push-apple.com.akadns.net
DOMAIN-SUFFIX,push.apple.com
DOMAIN-SUFFIX,lcdn-locator.apple.com
DOMAIN-SUFFIX,lcdn-registration.apple.com
DOMAIN-SUFFIX,ls.apple.com
PROCESS-NAME,trustd
PROCESS-NAME,netbiosd
";

const LAN_SET: &str = "\
DOMAIN-SUFFIX,local
IP-CIDR,0.0.0.0/8
IP-CIDR,10.0.0.0/8
IP-CIDR,100.64.0.0/10
IP-CIDR,127.0.0.0/8
IP-CIDR,169.254.0.0/16
IP-CIDR,172.16.0.0/12
IP-CIDR,192.0.0.0/24
IP-CIDR,192.0.2.0/24
IP-CIDR,192.168.0.0/16
IP-CIDR,224.0.0.0/4
IP-CIDR6,::1/128
IP-CIDR6,fc00::/7
IP-CIDR6,fe80::/10
";

pub fn internal_set_text(set: InternalSet) -> &'static str {
    match set {
        InternalSet::System => SYSTEM_SET,
        InternalSet::Lan => LAN_SET,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::Path;

    fn ctx_with<'a>(names: &'a HashSet<String>) -> ParseCtx<'a> {
        ParseCtx {
            inline_rulesets: names,
            base_dir: Path::new("."),
        }
    }

    #[test]
    fn rule_set_skips_comments_blank_lines_and_keeps_line_params() {
        let names = HashSet::new();
        let text = "# c1\n// c2\n; c3\n\nDOMAIN-SUFFIX,a.com\n  IP-CIDR,10.0.0.0/8,no-resolve  \n";
        let p = parse_set(SetKind::RuleSet, text, &ctx_with(&names));
        assert_eq!(p.lines.len(), 2);
        assert!(p.skipped.is_empty());
        match &p.lines[1] {
            SetLine::Rule(r) => assert!(r.no_resolve),
            _ => panic!("expected rule"),
        }
    }

    #[test]
    fn rule_set_rejects_final_pre_matching_and_garbage() {
        let names = HashSet::new();
        let text = "FINAL,DIRECT\nDOMAIN,a.com,pre-matching\nNOT-A-RULE\nDOMAIN,b.com\n";
        let p = parse_set(SetKind::RuleSet, text, &ctx_with(&names));
        assert_eq!(p.lines.len(), 1);
        let lines: Vec<usize> = p.skipped.iter().map(|(l, _)| *l).collect();
        assert_eq!(lines, vec![1, 2, 3]);
        assert!(p.skipped[0].1.contains("FINAL"));
        assert!(p.skipped[1].1.contains("pre-matching"));
    }

    #[test]
    fn domain_set_forms() {
        let names = HashSet::new();
        let text = "# c\n// c\n.Example.com\nexact.com.\n*.bad.com\nbad domain\n\n";
        let p = parse_set(SetKind::DomainSet, text, &ctx_with(&names));
        assert_eq!(p.lines.len(), 2);
        assert!(
            matches!(&p.lines[0], SetLine::Domain { name, suffix: true } if name == "example.com")
        );
        assert!(
            matches!(&p.lines[1], SetLine::Domain { name, suffix: false } if name == "exact.com")
        );
        assert_eq!(p.skipped.len(), 2);
    }

    #[test]
    fn semicolon_is_a_comment_only_in_rule_sets() {
        let names = HashSet::new();
        let p = parse_set(SetKind::DomainSet, "; not a comment\n", &ctx_with(&names));
        assert_eq!(p.lines.len(), 0);
        assert_eq!(p.skipped.len(), 1);
    }

    #[test]
    fn limit_truncates_and_counts() {
        let names = HashSet::new();
        let text = "a.com\nb.com\nc.com\n";
        let p = parse_set_with_limit(SetKind::DomainSet, text, &ctx_with(&names), 2);
        assert_eq!(p.lines.len(), 2);
        assert_eq!(p.truncated, 1);
    }

    #[test]
    fn internal_sets_parse_completely() {
        let names = HashSet::new();
        let sys = parse_set(
            SetKind::RuleSet,
            internal_set_text(InternalSet::System),
            &ctx_with(&names),
        );
        assert_eq!(sys.lines.len(), 20);
        assert!(sys.skipped.is_empty());
        let lan = parse_set(
            SetKind::RuleSet,
            internal_set_text(InternalSet::Lan),
            &ctx_with(&names),
        );
        assert_eq!(lan.lines.len(), 14);
        assert!(lan.skipped.is_empty());
    }
}
