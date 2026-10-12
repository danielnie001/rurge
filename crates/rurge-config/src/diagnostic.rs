use crate::span::Span;
use serde::Serialize;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        })
    }
}

/// One problem found while loading a profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub span: Option<Span>,
    /// What to do about the problem. Static, so it cannot quote the profile.
    pub hint: Option<&'static str>,
}

impl Diagnostic {
    /// The hint is the default one of `code`, if it has one (`codes::hint`).
    pub fn new(severity: Severity, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity,
            code,
            message: message.into(),
            span: None,
            hint: codes::hint(code),
        }
    }
    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(Severity::Error, code, message)
    }
    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, code, message)
    }
    pub fn info(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(Severity::Info, code, message)
    }
    pub fn at(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }
    /// Replaces the default hint of the code, or gives one to a code that has
    /// none.
    pub fn with_hint(mut self, hint: &'static str) -> Self {
        self.hint = Some(hint);
        self
    }
    /// What `Display` writes before the hint: severity, code, position and
    /// message, on one line. A log record stays one line, so it writes this.
    pub fn headline(&self) -> impl fmt::Display + '_ {
        Headline(self)
    }
}

struct Headline<'a>(&'a Diagnostic);

impl fmt::Display for Headline<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = self.0;
        write!(f, "{}[{}]", d.severity, d.code)?;
        if let Some(span) = &d.span {
            write!(f, " {span}")?;
        }
        write!(f, ": {}", d.message)
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", Headline(self))?;
        if let Some(hint) = &self.hint {
            write!(f, "\n  hint: {hint}")?;
        }
        Ok(())
    }
}

/// Ordered collection of diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn push(&mut self, d: Diagnostic) {
        self.items.push(d);
    }
    pub fn extend(&mut self, other: Diagnostics) {
        self.items.extend(other.items);
    }
    pub fn has_errors(&self) -> bool {
        self.items.iter().any(|d| d.severity == Severity::Error)
    }
    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.items.iter()
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.items
    }
    /// Sort by file, then line, then severity (errors first). Spanless items come first.
    pub fn sorted(mut self) -> Self {
        self.items.sort_by(|a, b| {
            a.span
                .cmp(&b.span)
                .then_with(|| b.severity.cmp(&a.severity))
        });
        self
    }
}

/// Error from a section-level parser; the caller attaches the span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub code: &'static str,
    pub message: String,
    /// What to do about this error where the default hint of its code does
    /// not say it. Static, so it cannot quote the profile.
    pub hint: Option<&'static str>,
}

impl ParseError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
        }
    }
    pub fn with_hint(mut self, hint: &'static str) -> Self {
        self.hint = Some(hint);
        self
    }
}

impl Diagnostic {
    pub fn from_parse(err: ParseError, span: Span) -> Diagnostic {
        let d = Diagnostic::error(err.code, err.message).at(span);
        match err.hint {
            Some(hint) => d.with_hint(hint),
            None => d,
        }
    }
}

/// Stable diagnostic codes. Never renumber.
pub mod codes {
    pub const E_SYNTAX: &str = "E0001";
    pub const E_INCLUDE_NOT_FOUND: &str = "E0002";
    pub const E_REQUIREMENT_SYNTAX: &str = "E0003";
    pub const E_UNKNOWN_POLICY_TYPE: &str = "E0004";
    pub const E_BUILTIN_REDEFINED: &str = "E0005";
    pub const E_DUPLICATE_NAME: &str = "E0006";
    pub const E_UNKNOWN_POLICY_REF: &str = "E0007";
    pub const E_UNKNOWN_GROUP_MEMBER: &str = "E0008";
    /// Retired in phase 2 M3: a group cycle is `W0030` and no longer stops a load.
    pub const E_GROUP_CYCLE: &str = "E0009";
    pub const E_MISSING_FINAL: &str = "E0010";
    pub const E_INVALID_RULE_VALUE: &str = "E0011";
    pub const E_UNKNOWN_RULE_TYPE: &str = "E0012";
    pub const E_LISTENER_NOT_IP: &str = "E0013";
    pub const E_NOT_ALLOWED_HERE: &str = "E0014";
    pub const E_NESTING_TOO_DEEP: &str = "E0015";
    pub const E_INCLUDE_CYCLE: &str = "E0016";
    pub const E_INVALID_DEFINITION: &str = "E0017";
    /// A known policy parameter has a value that cannot be used.
    pub const E_INVALID_POLICY_PARAM: &str = "E0018";
    /// An `underlying-proxy` chain that leads back to the policy itself.
    pub const E_UNDERLYING_PROXY_CYCLE: &str = "E0019";
    /// A `[Keystore]` reference that names a missing item or one of the wrong type.
    pub const E_KEYSTORE_REF: &str = "E0020";
    /// A `[Keystore]` item whose `base64` does not decode.
    pub const E_KEYSTORE_BASE64: &str = "E0021";
    /// A policy that cannot be built (reported by the engine's dry build, M1b).
    pub const E_POLICY_BUILD: &str = "E0022";
    /// A `[WireGuard <name>]` section that cannot be used, or a
    /// `section-name` that names no usable section.
    pub const E_WIREGUARD_SECTION: &str = "E0023";
    /// One URL or file used as both a `RULE-SET` and a `DOMAIN-SET`: one of
    /// the two reads the file in the wrong format.
    pub const E_SET_KIND_CONFLICT: &str = "E0024";
    pub const W_UNKNOWN_KEY: &str = "W0001";
    pub const W_UNKNOWN_SECTION: &str = "W0002";
    pub const W_UNKNOWN_RULE_PARAM: &str = "W0003";
    pub const W_PLATFORM_IGNORED: &str = "W0004";
    pub const W_INVALID_HOST_LIST_ENTRY: &str = "W0005";
    pub const W_VANISHED_KEY: &str = "W0006";
    pub const W_PROTOCOL_NOT_IMPLEMENTED: &str = "W0007";
    pub const W_GROUP_NOT_IMPLEMENTED: &str = "W0008";
    pub const W_IOS_BUILTIN_AS_DIRECT: &str = "W0009";
    pub const W_DEVICE_POLICY_AS_REJECT: &str = "W0010";
    pub const W_REMOTE_INCLUDE_NOT_READ: &str = "W0011";
    pub const W_INVALID_VALUE: &str = "W0012";
    pub const W_RULE_NEVER_MATCHES: &str = "W0013";
    pub const W_UNKNOWN_DIRECTIVE: &str = "W0014";
    pub const W_LINE_OUTSIDE_SECTION: &str = "W0015";
    pub const W_DEFERRED_SECTION: &str = "W0016";
    pub const W_INCLUDE_SECTION_MISSING: &str = "W0017";
    pub const W_RULESET_LINE_SKIPPED: &str = "W0018";
    pub const W_RULES_AFTER_FINAL: &str = "W0019";
    /// A named section defined twice (`[Ruleset X]`, `[WireGuard X]`): the
    /// first one is used.
    pub const W_DUPLICATE_RULESET: &str = "W0020";
    /// A FINAL before the last one: matching ends at it all the same, but the
    /// policy and the options are the last FINAL's.
    pub const W_DUPLICATE_FINAL: &str = "W0021";
    /// Something the profile names by a URL has no data yet (never downloaded,
    /// or failed without a cache): a set, the text of a remote include, a
    /// managed profile. Also said of sets whose data arrived and was not
    /// compiled yet when a command stopped waiting for them.
    pub const W_RESOURCE_UNAVAILABLE: &str = "W0022";
    /// A set file or a `policy-path` subscription contained lines that were
    /// skipped, or a group had to leave a member out.
    pub const W_SET_LINES_SKIPPED: &str = "W0023";
    /// A set file or a `policy-path` subscription exceeded its limit and was truncated.
    pub const W_SET_TRUNCATED: &str = "W0024";
    /// Nested set reference forms a cycle or exceeds the nesting limit.
    pub const W_SET_NESTING: &str = "W0025";
    /// A DNS upstream that cannot be used: a URL that does not parse, or a
    /// transport whose client could not be built.
    pub const W_DNS_UPSTREAM_UNSUPPORTED: &str = "W0026";
    /// A `[Host]` entry with a `script:` value was skipped (scripts arrive in phase 5).
    pub const W_HOST_SCRIPT_SKIPPED: &str = "W0027";
    /// A parameter that does not apply to this policy type; ignored.
    pub const W_PARAM_NOT_APPLICABLE: &str = "W0028";
    /// A parameter that is parsed but has no effect in this version.
    pub const W_PARAM_NOT_EFFECTIVE: &str = "W0029";
    /// Policy groups that reference each other; they behave as REJECT.
    pub const W_GROUP_CYCLE: &str = "W0030";
    /// An `ss` stream cipher is in use: no integrity protection.
    pub const W_STREAM_CIPHER: &str = "W0031";
    /// A set reference inside a set file that names a resource already used
    /// as the other kind of set; it matches nothing.
    pub const W_SET_KIND_CONFLICT: &str = "W0032";
    /// A downloaded text asks for something on this machine that the user did
    /// not write: it includes a local file it may not, names a local file as
    /// a set or as a group's `policy-path`, or defines an `external` policy.
    /// The line or the parameter is ignored, or the policy is not started.
    pub const W_REMOTE_LOCAL_ACCESS: &str = "W0033";
    /// The downloaded copy of a managed profile is in effect and will not be
    /// updated: it carries no `#!MANAGED-CONFIG` line of its own, or the lines
    /// of the copies lead through too many addresses.
    pub const W_MANAGED_NOT_UPDATED: &str = "W0034";
    pub const I_LEGACY_MIGRATED: &str = "I0001";
    pub const I_LINE_DISABLED: &str = "I0002";
    /// GeoIP / ASN database file not present yet.
    pub const I_GEOIP_DB_MISSING: &str = "I0003";
    /// Which text of a managed profile is in effect when it is not the local
    /// file alone: the downloaded copy, or the local file changed since the
    /// last update.
    pub const I_MANAGED_SOURCE: &str = "I0004";

    /// What to do about a diagnostic of `code`, when there is one thing to say
    /// for every place that emits it. The constructors of `Diagnostic` take
    /// it as the hint and `with_hint` replaces it. A sentence is static, so
    /// it never quotes anything of the profile.
    ///
    /// Where the places that emit a code want different fixes, the sentence
    /// says what they share or the code has none. `E0014` has none because
    /// each of its four places carries a hint of its own, `W0033` because each
    /// of its four places (an `#!include`, a set, a `policy-path`, an
    /// `external` policy) carries one for each kind of text, and `W0034`
    /// because each of its two does. The other codes without one:
    ///
    /// - `E0009`: no place emits it any more.
    /// - `E0017`: the message is the shape to write, a different one at each
    ///   of its three places.
    /// - `E0022`: the message holds the reason the policy could not be built.
    /// - `E0023`: the message says what is missing or wrong in the section;
    ///   what to do about a `section-name` that names none depends on whether
    ///   the section exists.
    /// - `W0004`, `W0009`, `W0010`, `W0016`, `W0027`: the line asks for what
    ///   does not apply on this platform or in this version, and there is
    ///   nothing to change in it.
    /// - `W0007`, `W0008`: each text of the message says where the policy or
    ///   the group goes instead.
    /// - `W0011`: only a caller that gave the loader no place to get
    ///   downloaded text from sees it, and the message says so.
    /// - `W0013`: the rule type cannot be matched on this platform in this
    ///   version; nothing in the rule is wrong.
    /// - `W0012`, `W0018`, `W0022`, `W0023`, `W0024`, `W0026`, `W0028`,
    ///   `W0029`: many places for different reasons, and the message says
    ///   what was done about each. `W0024` has two limits, a set's entries
    ///   and a subscription's policies and lines, with different remedies.
    /// - `W0031`: the cipher of an imported line is not the reader's to
    ///   change, and the message of the profile's own line says to use an AEAD
    ///   cipher.
    /// - `I0001`, `I0002`, `I0003`, `I0004`: they inform.
    pub fn hint(code: &str) -> Option<&'static str> {
        Some(match code {
            E_SYNTAX => "compare the line with the syntax the Surge manual gives for this section",
            E_INCLUDE_NOT_FOUND => {
                "check that the file named after `#!include` exists and can be read as text; a relative path starts at the main profile's directory"
            }
            E_REQUIREMENT_SYNTAX => {
                "write the condition as the Surge manual does, for example `#!REQUIREMENT CORE_VERSION>=22`"
            }
            E_UNKNOWN_POLICY_TYPE => {
                "use a type the Surge manual lists for this section, and check its spelling"
            }
            E_BUILTIN_REDEFINED => {
                "pick another name, or remove the line to keep the built-in policy"
            }
            E_DUPLICATE_NAME => "give every policy and policy group a name of its own",
            E_UNKNOWN_POLICY_REF => {
                "define the policy in `[Proxy]` or `[Proxy Group]`, or correct the name"
            }
            E_UNKNOWN_GROUP_MEMBER => {
                "define the missing policy or group in `[Proxy]` or `[Proxy Group]`, or correct the name"
            }
            E_MISSING_FINAL => "add `FINAL,DIRECT` as the last rule",
            E_INVALID_RULE_VALUE => "check the value against what the Surge manual allows for it",
            E_UNKNOWN_RULE_TYPE => "check the rule type's spelling against the Surge manual",
            E_LISTENER_NOT_IP => {
                "write an IP address with an optional port: `127.0.0.1` listens on this machine only, `0.0.0.0` on every interface"
            }
            E_NESTING_TOO_DEEP => "rewrite the logical rule with fewer levels",
            E_INCLUDE_CYCLE => {
                "simplify the `#!include` lines: a file cannot include itself, directly or through other files, and both the nesting and the number of files are limited"
            }
            E_INVALID_POLICY_PARAM => {
                "check the parameter against the Surge manual's page for this policy or group type"
            }
            E_UNDERLYING_PROXY_CYCLE => {
                "remove or change one of the `underlying-proxy` parameters on the loop"
            }
            E_KEYSTORE_REF => {
                "name a `[Keystore]` item that exists and is of the kind this parameter needs"
            }
            E_KEYSTORE_BASE64 => "encode the file as standard Base64 on a single line",
            E_SET_KIND_CONFLICT | W_SET_KIND_CONFLICT => {
                "use the resource as one kind of set only; when both kinds are needed, keep a separate copy for each"
            }
            W_UNKNOWN_KEY => "check the spelling against the Surge manual, or remove it",
            W_UNKNOWN_SECTION => "check the section name's spelling against the Surge manual",
            W_UNKNOWN_RULE_PARAM => {
                "check the parameter's spelling and its value against the Surge manual"
            }
            W_INVALID_HOST_LIST_ENTRY => "correct or remove the entry",
            W_VANISHED_KEY => "remove it",
            W_UNKNOWN_DIRECTIVE => "check the directive's spelling against the Surge manual",
            W_LINE_OUTSIDE_SECTION => "move the line under a `[Section]` header",
            W_INCLUDE_SECTION_MISSING => {
                "move the `#!include` line to a section the file has, or add that section to the file"
            }
            W_RULES_AFTER_FINAL => "move `FINAL` to the end of `[Rule]`",
            W_DUPLICATE_RULESET => "remove one of the two sections, or give it another name",
            W_DUPLICATE_FINAL => "keep a single `FINAL` rule",
            W_SET_NESTING => {
                "remove the reference to this set from the set that nests it, or flatten the sets"
            }
            W_GROUP_CYCLE => "remove one of the group references on the loop",
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;

    fn span(line: u32) -> Span {
        Span::new(Arc::from(Path::new("a.conf")), line)
    }

    /// The codes that have a default hint.
    const WITH_DEFAULT: &[&str] = &[
        codes::E_SYNTAX,
        codes::E_INCLUDE_NOT_FOUND,
        codes::E_REQUIREMENT_SYNTAX,
        codes::E_UNKNOWN_POLICY_TYPE,
        codes::E_BUILTIN_REDEFINED,
        codes::E_DUPLICATE_NAME,
        codes::E_UNKNOWN_POLICY_REF,
        codes::E_UNKNOWN_GROUP_MEMBER,
        codes::E_MISSING_FINAL,
        codes::E_INVALID_RULE_VALUE,
        codes::E_UNKNOWN_RULE_TYPE,
        codes::E_LISTENER_NOT_IP,
        codes::E_NESTING_TOO_DEEP,
        codes::E_INCLUDE_CYCLE,
        codes::E_INVALID_POLICY_PARAM,
        codes::E_UNDERLYING_PROXY_CYCLE,
        codes::E_KEYSTORE_REF,
        codes::E_KEYSTORE_BASE64,
        codes::E_SET_KIND_CONFLICT,
        codes::W_UNKNOWN_KEY,
        codes::W_UNKNOWN_SECTION,
        codes::W_UNKNOWN_RULE_PARAM,
        codes::W_INVALID_HOST_LIST_ENTRY,
        codes::W_VANISHED_KEY,
        codes::W_UNKNOWN_DIRECTIVE,
        codes::W_LINE_OUTSIDE_SECTION,
        codes::W_INCLUDE_SECTION_MISSING,
        codes::W_RULES_AFTER_FINAL,
        codes::W_DUPLICATE_RULESET,
        codes::W_DUPLICATE_FINAL,
        codes::W_SET_NESTING,
        codes::W_GROUP_CYCLE,
        codes::W_SET_KIND_CONFLICT,
    ];

    /// The codes whose every site carries a hint of its own.
    const PER_SITE: &[&str] = &[
        codes::E_NOT_ALLOWED_HERE,
        codes::W_REMOTE_LOCAL_ACCESS,
        codes::W_MANAGED_NOT_UPDATED,
    ];

    /// The codes without a hint; `codes::hint` says why beside its table.
    const WITHOUT: &[&str] = &[
        codes::E_GROUP_CYCLE,
        codes::E_INVALID_DEFINITION,
        codes::E_POLICY_BUILD,
        codes::E_WIREGUARD_SECTION,
        codes::W_PLATFORM_IGNORED,
        codes::W_PROTOCOL_NOT_IMPLEMENTED,
        codes::W_GROUP_NOT_IMPLEMENTED,
        codes::W_IOS_BUILTIN_AS_DIRECT,
        codes::W_DEVICE_POLICY_AS_REJECT,
        codes::W_REMOTE_INCLUDE_NOT_READ,
        codes::W_INVALID_VALUE,
        codes::W_RULE_NEVER_MATCHES,
        codes::W_DEFERRED_SECTION,
        codes::W_RULESET_LINE_SKIPPED,
        codes::W_RESOURCE_UNAVAILABLE,
        codes::W_SET_LINES_SKIPPED,
        codes::W_SET_TRUNCATED,
        codes::W_DNS_UPSTREAM_UNSUPPORTED,
        codes::W_HOST_SCRIPT_SKIPPED,
        codes::W_PARAM_NOT_APPLICABLE,
        codes::W_PARAM_NOT_EFFECTIVE,
        codes::W_STREAM_CIPHER,
        codes::I_LEGACY_MIGRATED,
        codes::I_LINE_DISABLED,
        codes::I_GEOIP_DB_MISSING,
        codes::I_MANAGED_SOURCE,
    ];

    /// The codes of `codes`, read off the source, so that a constant added
    /// later has to be put on one of the lists above.
    fn declared() -> Vec<&'static str> {
        let source = include_str!("diagnostic.rs");
        let from = source.find("pub mod codes {").expect("the codes module");
        let to = from + source[from..].find("#[cfg(test)]").expect("the tests");
        source[from..to]
            .lines()
            .filter(|line| line.trim_start().starts_with("pub const "))
            .map(|line| line.split('"').nth(1).expect("a code in quotes"))
            .collect()
    }

    /// The codes that occur more than once in a sorted list, each named once.
    fn repeated<'a>(sorted: &[&'a str]) -> Vec<&'a str> {
        let mut again: Vec<&str> = sorted
            .windows(2)
            .filter(|pair| pair[0] == pair[1])
            .map(|pair| pair[0])
            .collect();
        again.dedup();
        again
    }

    #[test]
    fn every_code_is_decided_once() {
        let mut listed: Vec<&str> = WITH_DEFAULT
            .iter()
            .chain(PER_SITE)
            .chain(WITHOUT)
            .copied()
            .collect();
        listed.sort();
        let mut declared = declared();
        declared.sort();
        let listed_twice = repeated(&listed);
        let declared_twice = repeated(&declared);
        listed.dedup();
        declared.dedup();
        let undecided: Vec<&str> = declared
            .iter()
            .copied()
            .filter(|code| !listed.contains(code))
            .collect();
        let undeclared: Vec<&str> = listed
            .iter()
            .copied()
            .filter(|code| !declared.contains(code))
            .collect();
        let mut problems = String::new();
        for (what, named, remedy) in [
            (
                "declared in `codes` and on none of WITH_DEFAULT, PER_SITE and WITHOUT",
                undecided,
                "give each an arm in `codes::hint` and list it in WITH_DEFAULT, or list it in WITHOUT and say why in the comment above `codes::hint` (in PER_SITE when every place that reports it carries a hint of its own)",
            ),
            (
                "on a list but not declared in `codes`",
                undeclared,
                "declare each in `codes`, or take it off the list",
            ),
            (
                "on the lists more than once",
                listed_twice,
                "a code is decided once: keep it on one list, once",
            ),
            (
                "declared in `codes` more than once",
                declared_twice,
                "a code is never reused: give each constant a code of its own",
            ),
        ] {
            if !named.is_empty() {
                problems += &format!("\n{what}: {}\n  {remedy}", named.join(", "));
            }
        }
        assert!(problems.is_empty(), "{problems}");
        for code in WITH_DEFAULT {
            assert!(codes::hint(code).is_some(), "{code} has no default hint");
        }
        for code in PER_SITE.iter().chain(WITHOUT) {
            assert_eq!(codes::hint(code), None, "{code} is not to have a default");
        }
        assert_eq!(codes::hint("E9999"), None);
    }

    #[test]
    fn a_default_hint_is_one_plain_sentence() {
        for code in WITH_DEFAULT {
            let hint = codes::hint(code).unwrap_or_default();
            assert!(!hint.is_empty(), "{code}");
            assert!(!hint.ends_with('.'), "{code} ends with a full stop: {hint}");
            assert!(
                !hint.starts_with(|c: char| c.is_uppercase()),
                "{code} starts with a capital: {hint}"
            );
            assert!(
                !hint.contains(['{', '}']),
                "{code} has a placeholder left: {hint}"
            );
            assert!(!hint.contains('\n'), "{code} is not one line: {hint}");
            assert!(hint.is_ascii(), "{code}: {hint}");
        }
    }

    #[test]
    fn a_diagnostic_takes_the_default_hint_of_its_code() {
        let said = codes::hint(codes::W_UNKNOWN_KEY).expect("W0001 has a hint");
        for d in [
            Diagnostic::error(codes::W_UNKNOWN_KEY, "m"),
            Diagnostic::warning(codes::W_UNKNOWN_KEY, "m"),
            Diagnostic::info(codes::W_UNKNOWN_KEY, "m"),
        ] {
            assert_eq!(d.hint, Some(said));
        }
        assert_eq!(Diagnostic::info(codes::I_LEGACY_MIGRATED, "m").hint, None);
    }

    #[test]
    fn a_hint_of_its_own_replaces_the_default() {
        let d = Diagnostic::warning(codes::W_UNKNOWN_KEY, "m").with_hint("say it this way");
        assert_eq!(d.hint, Some("say it this way"));
        assert!(d.to_string().ends_with("\n  hint: say it this way"));
        let d = Diagnostic::warning(codes::W_PARAM_NOT_APPLICABLE, "m").with_hint("and this");
        assert_eq!(d.hint, Some("and this"));
    }

    /// The one hint that existed before the table: its display is the same.
    #[test]
    fn a_missing_final_reads_as_it_always_did() {
        let d = Diagnostic::error(codes::E_MISSING_FINAL, "missing FINAL rule").at(span(12));
        assert_eq!(
            d.to_string(),
            "error[E0010] a.conf:12: missing FINAL rule\n  hint: add `FINAL,DIRECT` as the last rule"
        );
    }

    #[test]
    fn the_headline_is_the_display_without_the_hint() {
        let d = Diagnostic::error(codes::E_MISSING_FINAL, "missing FINAL rule")
            .at(span(12))
            .with_hint("add something");
        assert_eq!(
            d.headline().to_string(),
            "error[E0010] a.conf:12: missing FINAL rule"
        );
        assert_eq!(
            d.to_string(),
            "error[E0010] a.conf:12: missing FINAL rule\n  hint: add something"
        );
        let spanless = Diagnostic::warning(codes::W_UNKNOWN_KEY, "m");
        assert_eq!(spanless.headline().to_string(), "warning[W0001]: m");
    }

    #[test]
    fn a_parse_error_takes_the_default_hint_unless_it_has_its_own() {
        let plain = Diagnostic::from_parse(ParseError::new(codes::E_SYNTAX, "m"), span(1));
        assert_eq!(plain.hint, codes::hint(codes::E_SYNTAX));
        let own = Diagnostic::from_parse(
            ParseError::new(codes::E_NOT_ALLOWED_HERE, "m").with_hint("move it"),
            span(1),
        );
        assert_eq!(own.hint, Some("move it"));
        assert_eq!(own.severity, Severity::Error);
        let none = Diagnostic::from_parse(ParseError::new(codes::E_NOT_ALLOWED_HERE, "m"), span(1));
        assert_eq!(none.hint, None);
    }

    #[test]
    fn builders_and_display() {
        let d = Diagnostic::error(codes::E_MISSING_FINAL, "missing FINAL rule")
            .at(span(12))
            .with_hint("add `FINAL,DIRECT` as the last rule");
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.code, "E0010");
        assert_eq!(
            d.to_string(),
            "error[E0010] a.conf:12: missing FINAL rule\n  hint: add `FINAL,DIRECT` as the last rule"
        );
    }

    #[test]
    fn diagnostics_has_errors_and_sorting() {
        let mut ds = Diagnostics::default();
        ds.push(Diagnostic::warning(codes::W_UNKNOWN_KEY, "w").at(span(5)));
        ds.push(Diagnostic::error(codes::E_SYNTAX, "e").at(span(2)));
        ds.push(Diagnostic::info(codes::I_LEGACY_MIGRATED, "i"));
        assert!(ds.has_errors());
        let sorted = ds.sorted().into_vec();
        assert_eq!(sorted[0].span, None); // spanless first
        assert_eq!(sorted[1].code, "E0001");
        assert_eq!(sorted[2].code, "W0001");
    }

    #[test]
    fn json_shape() {
        let d = Diagnostic::warning(codes::W_UNKNOWN_KEY, "unknown key").at(span(3));
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["severity"], "warning");
        assert_eq!(json["code"], "W0001");
        assert_eq!(json["span"]["line"], 3);
        assert_eq!(json["span"]["file"], "a.conf");
    }

    #[test]
    fn json_carries_the_hint() {
        let with = serde_json::to_value(Diagnostic::warning(codes::W_UNKNOWN_KEY, "m")).unwrap();
        assert_eq!(
            with["hint"],
            codes::hint(codes::W_UNKNOWN_KEY).expect("W0001 has a hint")
        );
        let without =
            serde_json::to_value(Diagnostic::info(codes::I_LEGACY_MIGRATED, "m")).unwrap();
        assert!(without["hint"].is_null());
    }
}
