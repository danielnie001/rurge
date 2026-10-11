//! Text formats of RULE-SET / DOMAIN-SET files (M2 design §6.2) and the
//! built-in `SYSTEM` / `LAN` sets (manual `rules/ruleset.html`, Internal Rule Sets).

use rurge_config::rule::{InternalSet, ParseCtx, SubRule, parse_subrule};
use rurge_config::text::strip_inline_comment;

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

/// Cap on how many `(line, reason)` pairs `ParsedSet::skipped` keeps. Set
/// files are untrusted (downloaded / user-supplied); without a cap a file
/// that is mostly garbage would grow `skipped` without bound. Beyond the
/// cap, lines are still counted in `skipped_total` but their reasons are
/// dropped.
pub const MAX_SKIPPED_REASONS: usize = 20;

#[derive(Debug, Default)]
pub struct ParsedSet {
    pub lines: Vec<SetLine>,
    /// `(1-based line number, reason)` for the first `MAX_SKIPPED_REASONS` skipped lines.
    pub skipped: Vec<(usize, String)>,
    /// Total number of skipped lines, including any beyond `MAX_SKIPPED_REASONS`.
    pub skipped_total: usize,
    /// Number of valid lines dropped because `limit` was reached.
    pub truncated: usize,
}

pub fn parse_set(kind: SetKind, text: &str, ctx: &ParseCtx) -> ParsedSet {
    parse_set_with_limit(kind, text, ctx, MAX_ENTRIES)
}

pub fn parse_set_with_limit(kind: SetKind, text: &str, ctx: &ParseCtx, limit: usize) -> ParsedSet {
    // Lists saved by some editors start with a byte order mark; it is not part
    // of the first line.
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut out = ParsedSet::default();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || is_comment(kind, line) {
            continue;
        }
        // A rule may be followed by a note, as in the profile; a domain set
        // line is one name, read by its own, stricter rule.
        let parsed = match kind {
            SetKind::RuleSet => parse_rule_line(strip_inline_comment(line), ctx),
            SetKind::DomainSet => parse_domain_line(line),
        };
        match parsed {
            Ok(l) if out.lines.len() < limit => out.lines.push(l),
            Ok(_) => out.truncated += 1,
            Err(reason) => {
                out.skipped_total += 1;
                if out.skipped.len() < MAX_SKIPPED_REASONS {
                    out.skipped.push((i + 1, reason));
                }
            }
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
    use rurge_config::rule::RuleKind;
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
    fn skipped_reasons_are_capped_but_the_total_keeps_counting() {
        let names = HashSet::new();
        let text = "NOT-A-RULE\n".repeat(25);
        let p = parse_set(SetKind::RuleSet, &text, &ctx_with(&names));
        assert_eq!(p.lines.len(), 0);
        assert_eq!(p.skipped.len(), MAX_SKIPPED_REASONS);
        assert_eq!(p.skipped_total, 25);
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

    /// The sub-rules of a parsed set; every line of it must be a rule line.
    fn rules(p: &ParsedSet) -> Vec<&SubRule> {
        p.lines
            .iter()
            .map(|l| match l {
                SetLine::Rule(r) => r,
                other => panic!("expected a rule line, got {other:?}"),
            })
            .collect()
    }

    /// Three lines the way the ASN lists in use write them: a note after the
    /// number, which may hold a comma or a `#` of its own.
    const NOTED_ASNS: &str = "\
IP-ASN,4134 // 中国电信骨干网
IP-ASN,132203 // Tencent Building, Kejizhongyi Avenue
IP-ASN,131326 // 2308 Yinlaige # 22 Jinsui Road
";

    #[test]
    fn a_rule_line_may_end_in_a_note_that_is_not_part_of_the_rule() {
        let names = HashSet::new();
        let p = parse_set(SetKind::RuleSet, NOTED_ASNS, &ctx_with(&names));
        assert_eq!(p.skipped_total, 0, "{:?}", p.skipped);
        let found = rules(&p);
        assert_eq!(found.len(), 3);
        for (rule, (asn, shown)) in found.into_iter().zip([
            (4134, "IP-ASN,4134"),
            (132203, "IP-ASN,132203"),
            (131326, "IP-ASN,131326"),
        ]) {
            assert_eq!(rule.kind, RuleKind::IpAsn(asn));
            // the comma inside a note does not start a parameter
            assert!(rule.unknown.is_empty(), "{:?}", rule.unknown);
            // the note is no part of what is shown as the sub-rule
            assert_eq!(rule.raw, shown);
        }
    }

    #[test]
    fn a_note_may_follow_a_tab_and_start_with_any_of_the_three_markers() {
        let names = HashSet::new();
        for line in [
            "IP-CIDR,10.0.0.0/8,no-resolve\t// lan",
            "IP-CIDR,10.0.0.0/8,no-resolve\t# lan",
            "IP-CIDR,10.0.0.0/8,no-resolve # lan",
            "IP-CIDR,10.0.0.0/8,no-resolve ; lan",
        ] {
            let p = parse_set(SetKind::RuleSet, line, &ctx_with(&names));
            assert_eq!(p.skipped_total, 0, "{line}: {:?}", p.skipped);
            let found = rules(&p);
            assert_eq!(found.len(), 1, "{line}");
            assert!(found[0].no_resolve, "{line}");
            assert!(
                found[0].unknown.is_empty(),
                "{line}: {:?}",
                found[0].unknown
            );
            assert_eq!(found[0].raw, "IP-CIDR,10.0.0.0/8,no-resolve", "{line}");
        }
    }

    /// A marker starts a note only after whitespace and outside double quotes:
    /// these lines keep every character they carry.
    #[test]
    fn a_marker_without_whitespace_before_it_or_inside_quotes_starts_no_note() {
        let names = HashSet::new();
        for (line, value) in [
            // `//` after a colon, `#` inside the pattern
            (
                r"URL-REGEX,^https?://example\.com/a#b",
                r"^https?://example\.com/a#b",
            ),
            // `;` right after a word
            (
                "USER-AGENT,Mozilla/5.0 (iPhone; CPU*",
                "Mozilla/5.0 (iPhone; CPU*",
            ),
            // `//` after whitespace, but inside double quotes
            (r#"URL-REGEX,"^http://a // b""#, "^http://a // b"),
        ] {
            let p = parse_set(SetKind::RuleSet, line, &ctx_with(&names));
            assert_eq!(p.skipped_total, 0, "{line}: {:?}", p.skipped);
            let found = rules(&p);
            assert_eq!(found.len(), 1, "{line}");
            let text = match &found[0].kind {
                RuleKind::UrlRegex(pattern) => pattern.source.as_str(),
                RuleKind::UserAgent(glob) => glob.source(),
                other => panic!("{line}: {other:?}"),
            };
            assert_eq!(text, value, "{line}");
            assert_eq!(found[0].raw, line);
        }
    }

    /// A set file reads a line as the profile always has: a marker inside
    /// single quotes still starts a note; double quotes keep a value whole.
    #[test]
    fn single_quotes_do_not_keep_a_marker_from_starting_a_note() {
        let names = HashSet::new();
        for (line, value, shown) in [
            // cut at the marker, in the middle of the quotes
            (
                "URL-REGEX,'^http://a // b'",
                "'^http://a",
                "URL-REGEX,'^http://a",
            ),
            (
                "URL-REGEX,'^http://a # b'",
                "'^http://a",
                "URL-REGEX,'^http://a",
            ),
            // the same values in double quotes are whole
            (
                r#"URL-REGEX,"^http://a // b""#,
                "^http://a // b",
                r#"URL-REGEX,"^http://a // b""#,
            ),
            (
                r#"URL-REGEX,"^http://a # b""#,
                "^http://a # b",
                r#"URL-REGEX,"^http://a # b""#,
            ),
        ] {
            let p = parse_set(SetKind::RuleSet, line, &ctx_with(&names));
            assert_eq!(p.skipped_total, 0, "{line}: {:?}", p.skipped);
            let found = rules(&p);
            assert_eq!(found.len(), 1, "{line}");
            let RuleKind::UrlRegex(pattern) = &found[0].kind else {
                panic!("{line}: {:?}", found[0].kind);
            };
            assert_eq!(pattern.source, value, "{line}");
            assert_eq!(found[0].raw, shown, "{line}");
        }
    }

    #[test]
    fn a_whole_line_comment_is_still_one_with_any_of_the_three_markers() {
        let names = HashSet::new();
        let text = "\
#!name=ASN.China
# IP-ASN,1 // a note
// IP-ASN,2 # a note
; IP-ASN,3 ; a note
   # IP-ASN,4 // indented
";
        let p = parse_set(SetKind::RuleSet, text, &ctx_with(&names));
        assert!(p.lines.is_empty(), "{:?}", p.lines);
        assert_eq!(p.skipped_total, 0, "{:?}", p.skipped);
    }

    /// A list as it is published: header lines, comments, blank lines, then a
    /// note after every number.
    #[test]
    fn a_file_of_the_usual_shape_yields_its_rule_lines_and_nothing_is_skipped() {
        let names = HashSet::new();
        let text = format!(
            "#!name=ASN.China\n#!desc=AS numbers of networks in China\n# Updated: 2026-10-11\n\
// one line per AS number\n\n{NOTED_ASNS}"
        );
        let p = parse_set(SetKind::RuleSet, &text, &ctx_with(&names));
        assert_eq!(p.skipped_total, 0, "{:?}", p.skipped);
        let kinds: Vec<&RuleKind> = rules(&p).into_iter().map(|r| &r.kind).collect();
        assert_eq!(
            kinds,
            [
                &RuleKind::IpAsn(4134),
                &RuleKind::IpAsn(132203),
                &RuleKind::IpAsn(131326)
            ]
        );
    }

    /// A domain set has its own, stricter line rule: a name followed by a note
    /// is a skipped line.
    #[test]
    fn a_domain_set_line_with_a_note_is_still_a_skipped_line() {
        let names = HashSet::new();
        let text = "example.com // note\nexample.org # note\n.example.net ; note\n";
        let p = parse_set(SetKind::DomainSet, text, &ctx_with(&names));
        assert!(p.lines.is_empty(), "{:?}", p.lines);
        let numbers: Vec<usize> = p.skipped.iter().map(|(n, _)| *n).collect();
        assert_eq!(numbers, [1, 2, 3]);
    }

    /// Lists saved by some editors start with a byte order mark, which is no
    /// part of the first line: the first rule is read like any other.
    #[test]
    fn a_byte_order_mark_at_the_head_of_a_rule_set_is_no_part_of_its_first_rule() {
        let names = HashSet::new();
        let text = "\u{feff}DOMAIN-SUFFIX,example.com\nDOMAIN,b.com\n";
        let p = parse_set(SetKind::RuleSet, text, &ctx_with(&names));
        assert_eq!(p.skipped_total, 0, "{:?}", p.skipped);
        let found = rules(&p);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].kind, RuleKind::DomainSuffix("example.com".into()));
        assert_eq!(found[0].raw, "DOMAIN-SUFFIX,example.com");
    }

    #[test]
    fn a_comment_after_a_byte_order_mark_is_still_a_comment_in_a_rule_set() {
        let names = HashSet::new();
        let text = "\u{feff}# header\nDOMAIN,a.com\n";
        let p = parse_set(SetKind::RuleSet, text, &ctx_with(&names));
        assert_eq!(p.skipped_total, 0, "{:?}", p.skipped);
        assert_eq!(p.lines.len(), 1);
    }

    #[test]
    fn a_comment_after_a_byte_order_mark_is_still_a_comment_in_a_domain_set() {
        let names = HashSet::new();
        let text = "\u{feff}# header\na.com\n";
        let p = parse_set(SetKind::DomainSet, text, &ctx_with(&names));
        assert_eq!(p.skipped_total, 0, "{:?}", p.skipped);
        assert_eq!(p.lines.len(), 1);
    }

    #[test]
    fn a_byte_order_mark_at_the_head_of_a_domain_set_is_no_part_of_its_first_name() {
        let names = HashSet::new();
        let text = "\u{feff}example.com\n.example.net\n";
        let p = parse_set(SetKind::DomainSet, text, &ctx_with(&names));
        assert_eq!(p.skipped_total, 0, "{:?}", p.skipped);
        assert_eq!(p.lines.len(), 2);
        assert!(
            matches!(&p.lines[0], SetLine::Domain { name, suffix: false } if name == "example.com")
        );
        assert!(
            matches!(&p.lines[1], SetLine::Domain { name, suffix: true } if name == "example.net")
        );
    }

    /// The mark is dropped from the very start of the text and nowhere else:
    /// any other is part of its line, which is then a skipped line, and the
    /// numbers of the skipped lines count from the first line as before.
    #[test]
    fn only_one_byte_order_mark_at_the_very_start_of_the_text_is_dropped() {
        let names = HashSet::new();
        for (text, parsed, skipped) in [
            // the start of a later line
            (
                "DOMAIN,a.com\n\u{feff}DOMAIN,b.com\nDOMAIN,c.com\n",
                2,
                vec![2],
            ),
            // a second mark at the head: one is dropped, the other stays
            ("\u{feff}\u{feff}DOMAIN,a.com\nDOMAIN,b.com\n", 1, vec![1]),
        ] {
            let p = parse_set(SetKind::RuleSet, text, &ctx_with(&names));
            assert_eq!(p.lines.len(), parsed, "{text:?}");
            let numbers: Vec<usize> = p.skipped.iter().map(|(n, _)| *n).collect();
            assert_eq!(numbers, skipped, "{text:?}");
        }
    }
}
