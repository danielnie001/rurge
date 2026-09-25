//! What a `policy-path` resource holds (M3 design 5.2): Surge policy lines,
//! as a plain list or as the `[Proxy]` section of a whole profile. Parsing
//! never fails: a line that cannot be used is skipped and reported by its
//! number — its text never leaves this module, it may carry a credential.

use rurge_config::diagnostic::codes;
use rurge_config::policy::{Builtin, ProxyPolicy, parse_policy};
use rurge_config::span::Span;
use rurge_config::text::{Origin, parse_str, strip_inline_comment};
use rurge_config::value::split_definition;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

/// More policies than this in one subscription are dropped (M3 design §9).
pub const MAX_POLICIES: usize = 10_000;

/// Only this many lines of a subscription are read at all: content written
/// by someone else must be bounded before anything is allocated from it — a
/// local file or a hostile server can otherwise make a subscription of
/// unbounded size.
pub const MAX_LINES: usize = 100_000;

/// `Subscription.skipped` lists at most this many (line, reason) pairs in
/// full; the rest are only counted, in `skipped_more`.
pub const MAX_SKIPPED_LISTED: usize = 20;

/// What the span of an imported line names in place of a file: never the
/// subscription's URL.
pub const SPAN_FILE: &str = "policy-path";

/// A subscription's policies. No `Debug`: the lines carry credentials.
#[derive(Clone, Default)]
pub struct Subscription {
    /// In file order, names unique; each span holds the line number.
    pub policies: Vec<ProxyPolicy>,
    /// Line number and reason of the first `MAX_SKIPPED_LISTED` lines that
    /// were skipped; `skipped_more` counts however many more there were.
    pub skipped: Vec<(u32, String)>,
    /// How many more lines were skipped beyond `skipped`'s first
    /// `MAX_SKIPPED_LISTED`.
    pub skipped_more: usize,
    /// There were more than `MAX_POLICIES` policies or more than
    /// `MAX_LINES` lines: the rest was dropped.
    pub truncated: bool,
}

impl Subscription {
    /// Records that `line` was skipped for `reason`: the first
    /// `MAX_SKIPPED_LISTED` are kept in full, the rest only counted.
    fn skip(&mut self, line: u32, reason: String) {
        if self.skipped.len() < MAX_SKIPPED_LISTED {
            self.skipped.push((line, reason));
        } else {
            self.skipped_more += 1;
        }
    }
}

pub fn parse(text: &str) -> Subscription {
    let file: Arc<Path> = Arc::from(Path::new(SPAN_FILE));
    let mut out = Subscription::default();
    let mut names: HashSet<String> = HashSet::new();
    let (entries, lines_truncated) = lines(text, &file);
    out.truncated = lines_truncated;
    for (line, raw) in entries {
        let Some((name, definition)) = split_definition(&raw) else {
            out.skip(line, "not a policy line (`Name = type, ...`)".to_string());
            continue;
        };
        if Builtin::parse(name).is_some() {
            out.skip(line, format!("`{name}` is the name of a built-in policy"));
            continue;
        }
        let policy = match parse_policy(name, definition, &Span::new(file.clone(), line)) {
            Ok(policy) => policy,
            Err(e) => {
                out.skip(line, reason(e.code));
                continue;
            }
        };
        if !names.insert(policy.name.clone()) {
            out.skip(
                line,
                format!(
                    "duplicate policy name `{}`; the first one is kept",
                    policy.name
                ),
            );
            continue;
        }
        if out.policies.len() == MAX_POLICIES {
            out.truncated = true;
            break;
        }
        out.policies.push(policy);
    }
    out
}

/// Why `parse_policy` refused a line, with nothing of the line in it: its
/// own messages quote the type and the port, and in a line that is not a
/// policy at all (a `vmess://` link) those are pieces of a credential.
fn reason(code: &str) -> String {
    if code == codes::E_UNKNOWN_POLICY_TYPE {
        "unknown policy type".to_string()
    } else {
        "not a valid policy line".to_string()
    }
}

/// Line number and content of every line that may hold a policy: the
/// `[Proxy]` section's, by the profile parser's own rules, when the text has
/// one; else every line that is neither blank nor a comment. Nothing is ever
/// included from elsewhere: a `#!include` line is just a line that is no
/// policy. Bounded to `MAX_LINES` lines of `text` before any of it is parsed
/// (the second return value says whether anything followed): a flood of
/// junk must not make the profile parser allocate a diagnostic per line, nor
/// this function a `String` per line, for content nobody asked to see.
fn lines(text: &str, file: &Arc<Path>) -> (Vec<(u32, String)>, bool) {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let (text, truncated) = bound_lines(text, MAX_LINES);
    // `has_proxy_header` is a superset of what `parse_str` recognises (it
    // matches `profile.section` case-insensitively too), so skipping the
    // parse when it is false changes nothing about a real profile.
    if has_proxy_header(text) {
        let (profile, _) = parse_str(text, file.clone(), Origin::Main);
        if let Some(section) = profile.section("Proxy") {
            let entries = section
                .active_entries()
                .map(|e| (e.span.line, e.raw.clone()))
                .collect();
            return (entries, truncated);
        }
    }
    let entries = text
        .lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let line = line.trim();
            if line.is_empty()
                || line.starts_with('#')
                || line.starts_with("//")
                || line.starts_with(';')
            {
                return None;
            }
            let content = strip_inline_comment(line);
            (!content.is_empty()).then(|| (i as u32 + 1, content.to_string()))
        })
        .collect();
    (entries, truncated)
}

/// Whether some line of `text` is a `[Proxy]` section header by the profile
/// parser's own rule: the name inside the brackets trimmed, compared
/// case-insensitively.
fn has_proxy_header(text: &str) -> bool {
    text.lines().any(|line| {
        line.trim()
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
            .is_some_and(|name| name.trim().eq_ignore_ascii_case("Proxy"))
    })
}

/// The first `n` lines of `text` (no copy: line numbers in the slice are
/// unchanged, since it starts at the start), and whether anything followed.
fn bound_lines(text: &str, n: usize) -> (&str, bool) {
    let mut seen = 0usize;
    for (i, _) in text.match_indices('\n') {
        seen += 1;
        if seen == n {
            return (&text[..=i], i + 1 < text.len());
        }
    }
    (text, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::PolicyKind;

    fn names(sub: &Subscription) -> Vec<(&str, u32)> {
        sub.policies
            .iter()
            .map(|p| (p.name.as_str(), p.span.line))
            .collect()
    }

    #[test]
    fn a_plain_list_is_read_line_by_line() {
        let sub = parse(
            "\u{FEFF}#!name=Nodes\r\n\r\n# a comment\r\n// another\r\n; and another\r\n\
HK-1 = http, hk.test, 8080 // inline\r\nUS-1 = trojan, us.test, 443, password=pw\r\n",
        );
        assert_eq!(names(&sub), [("HK-1", 6), ("US-1", 7)]);
        assert!(sub.skipped.is_empty(), "{:?}", sub.skipped);
        assert_eq!(sub.policies[0].kind, PolicyKind::Http);
        assert_eq!(sub.policies[0].definition, "http, hk.test, 8080");
        assert_eq!(sub.policies[0].span.file.as_ref(), Path::new(SPAN_FILE));
        assert!(!sub.truncated);
    }

    #[test]
    fn a_whole_profile_gives_its_proxy_section_only() {
        let sub = parse(
            "[General]\nloglevel = notify\n[Proxy]\nA = http, a.test, 80\n# gone\nB = socks5, b.test, 1080\n\
[Proxy Group]\nG = select, A, B\n[Rule]\nFINAL,G\n",
        );
        assert_eq!(names(&sub), [("A", 4), ("B", 6)]);
        assert!(sub.skipped.is_empty(), "{:?}", sub.skipped);
    }

    /// The reasons name the problem, never a piece of the line: a
    /// `vmess://` link or a stray password must not reach the logs.
    #[test]
    fn a_line_that_cannot_be_used_is_skipped_by_number_without_its_text() {
        let text = "vmess://eyJpZCI6IjAyMzNkMTFjLTE1YTQtNDdkMy1hZGUzLTQ4ZmZjYTBjZTExOSJ9=\n\
Odd = vless, odd.test, 443\nPort = http, p.test, s3cretport\nDIRECT = direct\nA = http, a.test, 80\n\
A = http, other.test, 80\nnot a policy\n";
        let sub = parse(text);
        assert_eq!(names(&sub), [("A", 5)]);
        assert_eq!(
            sub.skipped,
            [
                (1, "not a valid policy line".to_string()),
                (2, "unknown policy type".to_string()),
                (3, "not a valid policy line".to_string()),
                (4, "`DIRECT` is the name of a built-in policy".to_string()),
                (
                    6,
                    "duplicate policy name `A`; the first one is kept".to_string()
                ),
                (7, "not a policy line (`Name = type, ...`)".to_string()),
            ]
        );
        for (_, reason) in &sub.skipped {
            for piece in ["eyJ", "vless", "s3cretport"] {
                assert!(!reason.contains(piece), "{reason}");
            }
        }
    }

    /// A Clash or base64 subscription holds no Surge line: nothing is
    /// imported, and the caller warns (M3-D2).
    #[test]
    fn a_subscription_in_another_format_yields_nothing() {
        for text in [
            "proxies:\n  - name: \"hk\"\n    type: ss\n    server: hk.test\n",
            "c3M6Ly9ZV1Z6TFRJMU5pMW5ZMjA2Y0hjPUBoay50ZXN0Ojg0NDM=\n",
        ] {
            let sub = parse(text);
            assert!(sub.policies.is_empty());
            assert!(!sub.skipped.is_empty());
        }
        assert!(parse("").policies.is_empty());
    }

    /// Subscription content is somebody else's text: it can never make
    /// rurge read a local file.
    #[test]
    fn an_include_line_is_never_followed() {
        let sub = parse("[Proxy]\n#!include /etc/passwd\nA = http, a.test, 80\n");
        assert_eq!(names(&sub), [("A", 3)]);
        assert_eq!(
            sub.skipped,
            [(2, "not a policy line (`Name = type, ...`)".to_string())]
        );
    }

    #[test]
    fn more_than_the_limit_is_dropped() {
        let text: String = (0..MAX_POLICIES + 5)
            .map(|i| format!("N{i} = http, n{i}.test, 80\n"))
            .collect();
        let sub = parse(&text);
        assert_eq!(sub.policies.len(), MAX_POLICIES);
        assert!(sub.truncated);
        assert_eq!(
            sub.policies[MAX_POLICIES - 1].name,
            format!("N{}", MAX_POLICIES - 1)
        );
    }

    /// A local file of a million junk lines must not become a million
    /// diagnostics: only the first `MAX_LINES` are read at all, and only the
    /// first `MAX_SKIPPED_LISTED` of those are kept in full.
    #[test]
    fn a_flood_of_bad_lines_is_counted_not_listed() {
        let text = "x\n".repeat(150_000);
        let sub = parse(&text);
        assert!(sub.policies.is_empty());
        assert_eq!(sub.skipped.len(), MAX_SKIPPED_LISTED);
        assert_eq!(sub.skipped_more, MAX_LINES - MAX_SKIPPED_LISTED);
        assert!(sub.truncated);
    }

    /// `MAX_LINES` lines, the last one ended by its newline, is not "more".
    #[test]
    fn exactly_the_line_limit_is_not_cut() {
        let text = format!("{}A = http, a.test, 80\n", "# c\n".repeat(MAX_LINES - 1));
        let sub = parse(&text);
        assert_eq!(names(&sub), [("A", MAX_LINES as u32)]);
        assert!(!sub.truncated);
        assert!(parse(&format!("{text}B = http, b.test, 80\n")).truncated);
    }

    /// The profile parser trims the name inside the brackets: `[ Proxy ]`
    /// is the `[Proxy]` section there, and so it is here.
    #[test]
    fn a_proxy_header_with_blanks_inside_is_the_proxy_section() {
        let sub = parse(
            "[General]\nloglevel = notify\n[ Proxy ]\nA = http, a.test, 80\n[Rule]\nFINAL,DIRECT\n",
        );
        assert_eq!(names(&sub), [("A", 4)]);
        assert!(sub.skipped.is_empty() && sub.skipped_more == 0);
    }
}
