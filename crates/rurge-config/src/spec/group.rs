//! Typed view of `[Proxy Group]` parameters (phase 2 M3 design §4): where a
//! group's members come from, the relay it chains them through, and the
//! testing options the automatic groups act on (M3b / M3c).

use super::{NameKind, Secret};
use crate::diagnostic::{Diagnostic, Severity, codes};
use crate::policy::{Builtin, GroupKind, PolicyGroup};
use crate::rule::Pattern;
use crate::span::Span;
use crate::value::{parse_bool, parse_key_value, split_list};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

/// How long a test result stays valid when `interval` is not written.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(600);
/// `url-test` switch damping when `tolerance` is not written.
pub const DEFAULT_TOLERANCE: Duration = Duration::from_millis(100);

/// Where a group's `policy-path` points.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PolicyPath {
    /// A subscription URL usually carries a token: `Debug` hides it.
    Url(Secret<Url>),
    /// Already resolved against the main profile's directory.
    File(PathBuf),
}

/// What a group takes in besides the members written on its line (manual:
/// Policy Including).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportOpts {
    pub policy_path: Option<PolicyPath>,
    /// Seconds; only a URL `policy-path` is refreshed.
    pub update_interval: Option<u64>,
    /// Applies to every member that is not written on the line.
    pub regex_filter: Option<Pattern>,
    pub name_prefix: Option<String>,
    /// `external-policy-modifier`: parameters that override those of every
    /// `policy-path` line, keys lowercase. A value may be a credential.
    pub modifier: Secret<Vec<(String, String)>>,
    pub include_all_proxies: bool,
    pub include_other_groups: Vec<String>,
}

/// When and how the automatic groups test their members.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestOpts {
    pub interval: Duration,
    /// `url-test` only; an explicit 0 is honoured.
    pub tolerance: Duration,
    /// A member whose score is not below this is not a candidate.
    pub timeout: Option<Duration>,
    pub evaluate_before_use: bool,
    /// `load-balance` only.
    pub persistent: bool,
}

impl Default for TestOpts {
    fn default() -> TestOpts {
        TestOpts {
            interval: DEFAULT_INTERVAL,
            tolerance: DEFAULT_TOLERANCE,
            timeout: None,
            evaluate_before_use: false,
            persistent: false,
        }
    }
}

/// One `policy-priority` pair: the first pattern that matches a member's
/// name scales its `smart` score by `factor`.
#[derive(Clone, Debug, PartialEq)]
pub struct Priority {
    pub pattern: Pattern,
    pub factor: f64,
}

// A factor is finite and above zero (checked at load), so `==` is total.
impl Eq for Priority {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupSpec {
    pub name: String,
    pub kind: GroupKind,
    /// Written on the line, in order; a `subnet` group's is its `default`.
    pub members: Vec<String>,
    pub import: ImportOpts,
    /// The relay every proxy member is chained through (FR-GRP-07); `DIRECT`
    /// is the same as none.
    pub underlying_proxy: Option<String>,
    pub test: TestOpts,
    pub priority: Vec<Priority>,
    pub hidden: bool,
    pub span: Span,
}

#[derive(Debug, Default)]
pub struct GroupOutcome {
    /// `None` when an error was found.
    pub spec: Option<GroupSpec>,
    pub diagnostics: Vec<Diagnostic>,
}

/// The parameters that decide the members; a `subnet` group takes none.
const IMPORT_PARAMS: [&str; 8] = [
    "policy-path",
    "update-interval",
    "policy-regex-filter",
    "external-policy-name-prefix",
    "external-policy-modifier",
    "include-all-proxies",
    "include-other-group",
    "underlying-proxy",
];

/// Typed reads over one group's parameters. Remembers which keys were read
/// so that `finish` can warn about the rest.
struct Reader<'a> {
    group: &'a PolicyGroup,
    used: HashSet<&'static str>,
    diags: Vec<Diagnostic>,
}

impl<'a> Reader<'a> {
    fn has(&self, key: &str) -> bool {
        self.group.params.contains(key)
    }

    fn str(&mut self, key: &'static str) -> Option<&'a str> {
        self.used.insert(key);
        let group = self.group;
        group.params.get(key)
    }

    fn bool(&mut self, key: &'static str) -> Option<bool> {
        let value = self.str(key)?;
        let parsed = parse_bool(value);
        if parsed.is_none() {
            self.invalid(key, value, "true or false");
        }
        parsed
    }

    /// A whole number; `positive` rules out 0.
    fn number(&mut self, key: &'static str, positive: bool, expected: &str) -> Option<u64> {
        let value = self.str(key)?;
        match value.trim().parse::<u64>() {
            Ok(n) if n > 0 || !positive => Some(n),
            _ => {
                self.invalid(key, value, expected);
                None
            }
        }
    }

    /// `E0018`. Never call this for a value that may carry a credential.
    fn invalid(&mut self, key: &str, value: &str, expected: &str) {
        self.error(
            codes::E_INVALID_POLICY_PARAM,
            format!("invalid value `{value}` for `{key}` (expected {expected})"),
        );
    }

    fn error(&mut self, code: &'static str, message: String) {
        let message = format!("policy group `{}`: {message}", self.group.name);
        self.diags
            .push(Diagnostic::error(code, message).at(self.group.span.clone()));
    }

    fn warn(&mut self, code: &'static str, message: String) {
        let message = format!("policy group `{}`: {message}", self.group.name);
        self.diags
            .push(Diagnostic::warning(code, message).at(self.group.span.clone()));
    }

    /// `W0028` when `key` is written although this group type ignores it;
    /// the value is not read.
    fn not_applicable(&mut self, key: &'static str) {
        if self.has(key) {
            self.used.insert(key);
            let kind = self.group.kind.keyword();
            self.warn(
                codes::W_PARAM_NOT_APPLICABLE,
                format!("`{key}` has no effect on a `{kind}` group; ignored"),
            );
        }
    }

    fn has_errors(&self) -> bool {
        self.diags.iter().any(|d| d.severity == Severity::Error)
    }

    /// Warns about every parameter nobody read.
    fn finish(mut self) -> Vec<Diagnostic> {
        let group = self.group;
        let mut reported = HashSet::new();
        for (key, _) in group.params.iter() {
            if !self.used.contains(key) && reported.insert(key) {
                self.warn(
                    codes::W_UNKNOWN_KEY,
                    format!("unknown parameter `{key}` ignored"),
                );
            }
        }
        self.diags
    }
}

/// `lookup` says what a name refers to; `base_dir` is the main profile's
/// directory, which a relative `policy-path` is resolved against.
pub fn to_group_spec(
    group: &PolicyGroup,
    base_dir: &Path,
    lookup: &dyn Fn(&str) -> Option<NameKind>,
) -> GroupOutcome {
    let mut r = Reader {
        group,
        used: HashSet::new(),
        diags: Vec::new(),
    };
    // only the user interface looks at these
    for key in ["no-alert", "icon-url", "category"] {
        r.used.insert(key);
    }
    let hidden = r.bool("hidden").unwrap_or(false);
    if r.has("url") {
        r.used.insert("url");
        r.warn(
            codes::W_VANISHED_KEY,
            "`url` has no effect in current versions; use the policy's `test-url` or `proxy-test-url`"
                .to_string(),
        );
    }
    let (members, import, underlying_proxy) = if group.kind == GroupKind::Subnet {
        // Which network this is only arrives in phase 3; until then the
        // group stands for its `default`.
        for key in ["default", "cellular"] {
            r.used.insert(key);
        }
        for key in IMPORT_PARAMS {
            r.not_applicable(key);
        }
        let default = group.params.get("default").map(str::to_string);
        (default.into_iter().collect(), ImportOpts::default(), None)
    } else {
        let import = read_import(&mut r, base_dir, lookup);
        let underlying_proxy = read_underlying(&mut r, lookup);
        (group.members.clone(), import, underlying_proxy)
    };
    let test = read_test(&mut r, group.kind);
    let priority = if group.kind == GroupKind::Smart {
        read_priority(&mut r)
    } else {
        r.not_applicable("policy-priority");
        Vec::new()
    };
    let failed = r.has_errors();
    let diagnostics = r.finish();
    let spec = (!failed).then(|| GroupSpec {
        name: group.name.clone(),
        kind: group.kind,
        members,
        import,
        underlying_proxy,
        test,
        priority,
        hidden,
        span: group.span.clone(),
    });
    GroupOutcome { spec, diagnostics }
}

fn read_import(
    r: &mut Reader<'_>,
    base_dir: &Path,
    lookup: &dyn Fn(&str) -> Option<NameKind>,
) -> ImportOpts {
    let mut import = ImportOpts::default();
    if let Some(value) = r.str("policy-path") {
        import.policy_path = policy_path(r, value, base_dir);
    }
    import.update_interval = r.number("update-interval", true, "a positive number of seconds");
    if let Some(value) = r.str("policy-regex-filter") {
        match Pattern::new(value) {
            Ok(pattern) => import.regex_filter = Some(pattern),
            Err(e) => r.error(
                codes::E_INVALID_POLICY_PARAM,
                format!("invalid `policy-regex-filter` `{value}`: {e}"),
            ),
        }
    }
    if let Some(value) = r.str("external-policy-name-prefix") {
        if value.contains('=') {
            r.invalid("external-policy-name-prefix", value, "a prefix without `=`");
        } else {
            import.name_prefix = Some(value.to_string());
        }
    }
    if let Some(value) = r.str("external-policy-modifier") {
        match modifier(value) {
            Some(pairs) => import.modifier = Secret::new(pairs),
            // never echoed: a value may be a credential
            None => r.error(
                codes::E_INVALID_POLICY_PARAM,
                "invalid `external-policy-modifier` (expected a quoted list of key=value pairs)"
                    .to_string(),
            ),
        }
    }
    import.include_all_proxies = r.bool("include-all-proxies").unwrap_or(false);
    if let Some(value) = r.str("include-other-group") {
        let names = split_list(value);
        if names.is_empty() {
            r.invalid("include-other-group", value, "a list of policy group names");
        }
        for name in &names {
            if lookup(name) != Some(NameKind::Group) {
                r.error(
                    codes::E_UNKNOWN_GROUP_MEMBER,
                    format!("`include-other-group` references unknown policy group `{name}`"),
                );
            }
        }
        import.include_other_groups = names;
    }
    import
}

/// A URL when the value has a scheme, else a file. The value is never
/// echoed: a subscription URL usually carries a token (M3-D7).
fn policy_path(r: &mut Reader<'_>, value: &str, base_dir: &Path) -> Option<PolicyPath> {
    let value = value.trim();
    if value.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`policy-path` is empty".to_string(),
        );
        return None;
    }
    if value.contains("://") {
        return match Url::parse(value) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => {
                Some(PolicyPath::Url(Secret::new(url)))
            }
            _ => {
                r.error(
                    codes::E_INVALID_POLICY_PARAM,
                    "`policy-path` is not a valid http:// or https:// URL".to_string(),
                );
                None
            }
        };
    }
    let path = Path::new(value);
    Some(PolicyPath::File(if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }))
}

/// `key=value` items the way `split_list` reads a quoted list; `None` when
/// the list is empty or an item is not a pair.
fn modifier(value: &str) -> Option<Vec<(String, String)>> {
    let items = split_list(value);
    if items.is_empty() {
        return None;
    }
    items
        .iter()
        .map(|item| parse_key_value(item).map(|(k, v)| (k.to_ascii_lowercase(), v.to_string())))
        .collect()
}

fn read_underlying(
    r: &mut Reader<'_>,
    lookup: &dyn Fn(&str) -> Option<NameKind>,
) -> Option<String> {
    let name = r.str("underlying-proxy")?.trim();
    match lookup(name) {
        None => {
            r.error(
                codes::E_UNKNOWN_POLICY_REF,
                format!("`underlying-proxy` references unknown policy `{name}`"),
            );
            None
        }
        // DIRECT is the absence of a chain
        Some(NameKind::Builtin(Builtin::Direct)) => None,
        Some(NameKind::Builtin(_)) => {
            r.invalid("underlying-proxy", name, "a proxy policy or a policy group");
            None
        }
        Some(NameKind::Policy(_) | NameKind::Group) => Some(name.to_string()),
    }
}

fn read_test(r: &mut Reader<'_>, kind: GroupKind) -> TestOpts {
    let mut test = TestOpts::default();
    // a select or subnet group never tests; a smart group tests every 5 minutes
    let tests = !matches!(kind, GroupKind::Select | GroupKind::Subnet);
    if tests && kind != GroupKind::Smart {
        if let Some(secs) = r.number("interval", true, "a positive number of seconds") {
            test.interval = Duration::from_secs(secs);
        }
    } else {
        r.not_applicable("interval");
    }
    if kind == GroupKind::UrlTest {
        if let Some(ms) = r.number("tolerance", false, "a number of milliseconds") {
            test.tolerance = Duration::from_millis(ms);
        }
    } else {
        r.not_applicable("tolerance");
    }
    if tests {
        test.timeout = r
            .number("timeout", false, "a number of seconds")
            .map(Duration::from_secs);
        test.evaluate_before_use = r.bool("evaluate-before-use").unwrap_or(false);
    } else {
        r.not_applicable("timeout");
        r.not_applicable("evaluate-before-use");
    }
    if kind == GroupKind::LoadBalance {
        test.persistent = r.bool("persistent").unwrap_or(false);
    } else {
        r.not_applicable("persistent");
    }
    test
}

/// `regex:factor` pairs separated by `;`; the factor follows the last `:`.
fn read_priority(r: &mut Reader<'_>) -> Vec<Priority> {
    let Some(value) = r.str("policy-priority") else {
        return Vec::new();
    };
    let parsed: Option<Vec<Priority>> = value
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            let (source, factor) = item.rsplit_once(':')?;
            let factor: f64 = factor.trim().parse().ok()?;
            if !(factor.is_finite() && factor > 0.0) {
                return None;
            }
            let pattern = Pattern::new(source).ok()?;
            Some(Priority { pattern, factor })
        })
        .collect();
    match parsed {
        Some(pairs) if !pairs.is_empty() => pairs,
        _ => {
            r.invalid(
                "policy-priority",
                value,
                "`regex:factor` pairs separated by `;`, every factor above 0",
            );
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{PolicyKind, parse_group};
    use std::sync::Arc;

    fn span() -> Span {
        Span::new(Arc::from(Path::new("p.conf")), 4)
    }

    fn lookup(name: &str) -> Option<NameKind> {
        match name {
            "A" | "B" | "Relay" => Some(NameKind::Policy(PolicyKind::Socks5)),
            "g1" | "g2" | "Pick" => Some(NameKind::Group),
            "DIRECT" => Some(NameKind::Builtin(Builtin::Direct)),
            "REJECT" => Some(NameKind::Builtin(Builtin::Reject)),
            _ => None,
        }
    }

    fn outcome(def: &str) -> GroupOutcome {
        let group = parse_group("G", def, &span()).unwrap();
        to_group_spec(&group, Path::new("/profiles"), &lookup)
    }

    fn spec(def: &str) -> GroupSpec {
        let o = outcome(def);
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        o.spec.expect("no errors")
    }

    fn messages(def: &str) -> Vec<(&'static str, String)> {
        outcome(def)
            .diagnostics
            .into_iter()
            .map(|d| (d.code, d.message))
            .collect()
    }

    #[test]
    fn the_manual_examples_read_into_the_spec() {
        let s = spec("select, policy-path=proxies.txt");
        assert_eq!(
            s.import.policy_path,
            Some(PolicyPath::File(Path::new("/profiles").join("proxies.txt")))
        );
        assert!(s.members.is_empty());

        let s = spec(
            "select, A, policy-path=https://sub.test/nodes?token=t0k3n, update-interval=3600, \
policy-regex-filter=^HK, external-policy-name-prefix=Sub-, \
external-policy-modifier=\"test-url=http://apple.com/,tfo=true\", include-all-proxies=true, \
include-other-group=\"g1,g2\", underlying-proxy=Relay, hidden=true, no-alert=true, \
icon-url=https://example.com/i.png, category=Media",
        );
        assert_eq!(s.members, ["A"]);
        let Some(PolicyPath::Url(url)) = &s.import.policy_path else {
            panic!("{:?}", s.import.policy_path)
        };
        assert_eq!(url.expose().as_str(), "https://sub.test/nodes?token=t0k3n");
        assert_eq!(s.import.update_interval, Some(3600));
        assert_eq!(
            s.import.regex_filter.as_ref().map(|p| p.source.as_str()),
            Some("^HK")
        );
        assert_eq!(s.import.name_prefix.as_deref(), Some("Sub-"));
        assert_eq!(
            s.import.modifier.expose(),
            &[
                ("test-url".to_string(), "http://apple.com/".to_string()),
                ("tfo".to_string(), "true".to_string())
            ]
        );
        assert!(s.import.include_all_proxies);
        assert_eq!(s.import.include_other_groups, ["g1", "g2"]);
        assert_eq!(s.underlying_proxy.as_deref(), Some("Relay"));
        assert!(s.hidden);

        let s =
            spec("url-test, A, B, interval=300, tolerance=0, timeout=5, evaluate-before-use=true");
        assert_eq!(
            s.test,
            TestOpts {
                interval: Duration::from_secs(300),
                tolerance: Duration::ZERO,
                timeout: Some(Duration::from_secs(5)),
                evaluate_before_use: true,
                persistent: false,
            }
        );
        assert_eq!(spec("fallback, A, B").test, TestOpts::default());
        assert!(spec("load-balance, A, B, persistent=true").test.persistent);

        let s = spec("smart, A, B, policy-priority=\"Premium:0.9;Backup:1.3\"");
        let pairs: Vec<(&str, f64)> = s
            .priority
            .iter()
            .map(|p| (p.pattern.source.as_str(), p.factor))
            .collect();
        assert_eq!(pairs, [("Premium", 0.9), ("Backup", 1.3)]);
    }

    #[test]
    fn direct_as_the_relay_is_no_relay_at_all() {
        assert_eq!(
            spec("select, A, underlying-proxy=DIRECT").underlying_proxy,
            None
        );
    }

    /// A subscription URL is a credential: no diagnostic repeats it, and
    /// neither does `Debug` — of the path or of the modifier.
    #[test]
    fn a_bad_policy_path_is_an_error_that_never_echoes_it() {
        for def in [
            "select, policy-path=ftp://sub.test/nodes?token=t0k3n",
            "select, policy-path=https://?token=t0k3n",
            "select, policy-path=\"\"",
        ] {
            let found = messages(def);
            assert_eq!(found.len(), 1, "{def}: {found:?}");
            assert_eq!(found[0].0, codes::E_INVALID_POLICY_PARAM);
            assert!(!found[0].1.contains("t0k3n"), "{}", found[0].1);
        }
        let found = messages("select, external-policy-modifier=\"password=hunter2,tfo\"");
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(!found[0].1.contains("hunter2"), "{}", found[0].1);

        let s = spec(
            "select, policy-path=https://sub.test/n?token=t0k3n, external-policy-modifier=\"password=hunter2\"",
        );
        let debug = format!("{s:?}");
        assert!(
            !debug.contains("t0k3n") && !debug.contains("hunter2"),
            "{debug}"
        );
    }

    #[test]
    fn values_that_cannot_be_used_are_errors() {
        for (def, message) in [
            (
                "select, policy-path=a.txt, update-interval=0",
                "policy group `G`: invalid value `0` for `update-interval` (expected a positive number of seconds)",
            ),
            (
                "select, policy-regex-filter=(",
                "policy group `G`: invalid `policy-regex-filter` `(`: ",
            ),
            (
                "select, external-policy-name-prefix=a=b",
                "policy group `G`: invalid value `a=b` for `external-policy-name-prefix` (expected a prefix without `=`)",
            ),
            (
                "select, include-all-proxies=maybe",
                "policy group `G`: invalid value `maybe` for `include-all-proxies` (expected true or false)",
            ),
            (
                "url-test, A, interval=0",
                "policy group `G`: invalid value `0` for `interval` (expected a positive number of seconds)",
            ),
            (
                "url-test, A, tolerance=-1",
                "policy group `G`: invalid value `-1` for `tolerance` (expected a number of milliseconds)",
            ),
            (
                "fallback, A, timeout=soon",
                "policy group `G`: invalid value `soon` for `timeout` (expected a number of seconds)",
            ),
            (
                "smart, A, policy-priority=\"A:0\"",
                "policy group `G`: invalid value `A:0` for `policy-priority` (expected `regex:factor` pairs separated by `;`, every factor above 0)",
            ),
            (
                "smart, A, policy-priority=\"A:-1;B:1\"",
                "policy group `G`: invalid value `A:-1;B:1` for `policy-priority` (expected `regex:factor` pairs separated by `;`, every factor above 0)",
            ),
            (
                "smart, A, policy-priority=Premium",
                "policy group `G`: invalid value `Premium` for `policy-priority` (expected `regex:factor` pairs separated by `;`, every factor above 0)",
            ),
            (
                "smart, A, policy-priority=\"(:1\"",
                "policy group `G`: invalid value `(:1` for `policy-priority` (expected `regex:factor` pairs separated by `;`, every factor above 0)",
            ),
        ] {
            let o = outcome(def);
            assert!(o.spec.is_none(), "{def}");
            let found: Vec<&str> = o.diagnostics.iter().map(|d| d.message.as_str()).collect();
            assert_eq!(found.len(), 1, "{def}: {found:?}");
            assert!(found[0].starts_with(message), "{def}: {}", found[0]);
            assert_eq!(o.diagnostics[0].code, codes::E_INVALID_POLICY_PARAM);
        }
    }

    #[test]
    fn names_are_checked_against_the_profile() {
        assert_eq!(
            messages("select, A, underlying-proxy=Nope"),
            [(
                codes::E_UNKNOWN_POLICY_REF,
                "policy group `G`: `underlying-proxy` references unknown policy `Nope`".to_string()
            )]
        );
        assert_eq!(
            messages("select, A, underlying-proxy=REJECT"),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy group `G`: invalid value `REJECT` for `underlying-proxy` (expected a proxy policy or a policy group)".to_string()
            )]
        );
        // a proxy is not a group whose members could be taken
        assert_eq!(
            messages("select, include-other-group=\"g1, A, missing\""),
            [
                (
                    codes::E_UNKNOWN_GROUP_MEMBER,
                    "policy group `G`: `include-other-group` references unknown policy group `A`"
                        .to_string()
                ),
                (
                    codes::E_UNKNOWN_GROUP_MEMBER,
                    "policy group `G`: `include-other-group` references unknown policy group `missing`"
                        .to_string()
                ),
            ]
        );
    }

    #[test]
    fn parameters_a_group_type_ignores_are_warned_about() {
        for (def, key, kind) in [
            ("fallback, A, tolerance=10", "tolerance", "fallback"),
            ("url-test, A, persistent=true", "persistent", "url-test"),
            (
                "url-test, A, policy-priority=\"A:1\"",
                "policy-priority",
                "url-test",
            ),
            ("smart, A, interval=60", "interval", "smart"),
            ("select, A, interval=60", "interval", "select"),
            ("select, A, timeout=5", "timeout", "select"),
            (
                "select, A, evaluate-before-use=true",
                "evaluate-before-use",
                "select",
            ),
            (
                "subnet, default=A, policy-path=a.txt",
                "policy-path",
                "subnet",
            ),
            (
                "subnet, default=A, underlying-proxy=Relay",
                "underlying-proxy",
                "subnet",
            ),
        ] {
            let found = messages(def);
            assert_eq!(
                found,
                [(
                    codes::W_PARAM_NOT_APPLICABLE,
                    format!("policy group `G`: `{key}` has no effect on a `{kind}` group; ignored")
                )],
                "{def}"
            );
        }
        assert_eq!(
            messages("url-test, A, url=http://bing.com/, mystery=1, default=A"),
            [
                (
                    codes::W_VANISHED_KEY,
                    "policy group `G`: `url` has no effect in current versions; use the policy's `test-url` or `proxy-test-url`".to_string()
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy group `G`: unknown parameter `mystery` ignored".to_string()
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy group `G`: unknown parameter `default` ignored".to_string()
                ),
            ]
        );
    }

    /// Which network this is only arrives in phase 3: until then a subnet
    /// group stands for its `default`, and its user-interface parameters
    /// are no conditions.
    #[test]
    fn a_subnet_group_stands_for_its_default() {
        let group = parse_group(
            "G",
            "subnet, default = A, SSID:Home = DIRECT, category=Home, hidden=true",
            &span(),
        )
        .unwrap();
        assert_eq!(group.conditions.len(), 1);
        let o = to_group_spec(&group, Path::new("/profiles"), &lookup);
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        let s = o.spec.unwrap();
        assert_eq!(s.members, ["A"]);
        assert!(s.hidden);
    }
}
