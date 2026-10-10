//! Typed reads over one policy's parameters. Remembers which keys were read
//! so that `finish` can warn about the rest.

use crate::diagnostic::{Diagnostic, Severity, codes};
use crate::policy::{ProxyPolicy, is_name, named};
use crate::redact::split_top_level;
use crate::value::{parse_bool, parse_key_value, split_list};
use std::collections::HashSet;
use std::str::FromStr;

/// Whether a diagnostic may quote `value` of a parameter: only when it can be
/// what somebody wrote for that one key. A comma that is missing lets the next
/// field slide into the value (`4 psk=…`), and what does not fit one key — a
/// space, an `=`, more than 64 characters — can be a credential. So can what
/// an input method types for a comma or an `=`, `，` and `＝`: the field after
/// a `，` is in the same value.
pub(crate) fn quotable(value: &str) -> bool {
    value.chars().nth(64).is_none()
        && !value.contains(char::is_whitespace)
        && !value.contains(['=', '＝', '，'])
}

/// Whether a diagnostic may quote the name of the parameter `key`: only when
/// every item of `definition` that the parser reads as that parameter writes
/// it as a parameter name — ASCII lower-case letters, digits, `-` and `_`, at
/// most 32 of them. The key is held lower-cased and unquoted — a Base64
/// password `aHVudGVyMg==` arrives as `ahvudgvymg`, `"pa=ss"` as `pa` — so
/// this is decided on the text of the line, not on the key.
pub(crate) fn written_as_name(definition: &str, key: &str) -> bool {
    split_top_level(definition).into_iter().all(|item| {
        let reads_as_key = split_list(item).pop().is_some_and(|field| {
            parse_key_value(&field).is_some_and(|(name, _)| name.eq_ignore_ascii_case(key))
        });
        let written = item.split('=').next().unwrap_or_default().trim();
        !reads_as_key
            || ((1..=32).contains(&written.len())
                && written.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_')
                }))
    })
}

/// The text of `E0018`, the value quoted when it can be.
pub(crate) fn invalid_value(key: &str, value: &str, expected: &str) -> String {
    if quotable(value) {
        format!("invalid value `{value}` for `{key}` (expected {expected})")
    } else {
        format!("invalid value for `{key}` (expected {expected})")
    }
}

/// What a diagnostic says of a name that `key` refers to and nothing has: the
/// name is quoted when it can be one, as a value is, and when it is a name
/// (`underlying-proxy=https://user:password@host:port` is none).
pub(crate) fn unknown_reference(key: &str, what: &str, name: &str) -> String {
    if quotable(name) && is_name(name) {
        format!("`{key}` references unknown {what} `{name}`")
    } else {
        format!("`{key}` references unknown {what}")
    }
}

/// The keystore item `name` as a diagnostic that found it says it: quoted when
/// its name is one, else "the item".
pub(crate) fn found_item(name: &str) -> String {
    if is_name(name) {
        format!("`{name}`")
    } else {
        "the item".to_string()
    }
}

pub struct ParamReader<'a> {
    policy: &'a ProxyPolicy,
    used: HashSet<String>,
    positional_used: usize,
    /// The parameters whose value was reported as an error.
    rejected: HashSet<String>,
    diags: Vec<Diagnostic>,
}

impl<'a> ParamReader<'a> {
    pub fn new(policy: &'a ProxyPolicy) -> ParamReader<'a> {
        ParamReader {
            policy,
            used: HashSet::new(),
            positional_used: 0,
            rejected: HashSet::new(),
            diags: Vec::new(),
        }
    }

    pub fn policy(&self) -> &'a ProxyPolicy {
        self.policy
    }

    /// `true` when the parameter is written on the policy line. To say of a
    /// parameter that it does not apply or has no effect, ask `has_unrejected`.
    pub fn has(&self, key: &str) -> bool {
        self.policy.params.contains(key)
    }

    /// `true` when `key` is written and its value is not an error already:
    /// the parameters a warning that they do not apply, or have no effect,
    /// may be said of. What is an error already gets that one sentence and
    /// no other.
    pub fn has_unrejected(&self, key: &str) -> bool {
        self.has(key) && !self.rejected(key)
    }

    /// Marks `key` as known without reading it.
    pub fn touch(&mut self, key: &str) {
        self.used.insert(key.to_ascii_lowercase());
    }

    /// The first value of `key`; marks it as read.
    pub fn str(&mut self, key: &str) -> Option<&'a str> {
        self.touch(key);
        let policy = self.policy;
        policy.params.get(key)
    }

    /// Every value of `key`, in the order written; marks it as read.
    pub fn all(&mut self, key: &str) -> Vec<&'a str> {
        self.touch(key);
        let policy = self.policy;
        policy.params.get_all(key)
    }

    /// The positional value at `index` (after `type, server, port`).
    pub fn positional(&mut self, index: usize) -> Option<&'a str> {
        let policy = self.policy;
        let value = policy.positional.get(index).map(String::as_str);
        if value.is_some() {
            self.positional_used = self.positional_used.max(index + 1);
        }
        value
    }

    pub fn bool(&mut self, key: &str) -> Option<bool> {
        let value = self.str(key)?;
        let parsed = parse_bool(value);
        if parsed.is_none() {
            self.invalid(key, value, "true or false");
        }
        parsed
    }

    pub fn number<T: FromStr>(&mut self, key: &str, expected: &str) -> Option<T> {
        let value = self.str(key)?;
        let parsed = value.trim().parse().ok();
        if parsed.is_none() {
            self.invalid(key, value, expected);
        }
        parsed
    }

    /// Case-insensitive lookup of the value in `table`.
    pub fn choice<T: Copy>(&mut self, key: &str, table: &[(&str, T)]) -> Option<T> {
        let value = self.str(key)?;
        let found = table
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(value.trim()))
            .map(|(_, v)| *v);
        if found.is_none() {
            let names: Vec<&str> = table.iter().map(|(name, _)| *name).collect();
            self.invalid(key, value, &names.join(" / "));
        }
        found
    }

    /// `E0018`. Never call this for a parameter whose value is a secret; the
    /// value is quoted only when `quotable`.
    pub fn invalid(&mut self, key: &str, value: &str, expected: &str) {
        self.error_on(
            key,
            codes::E_INVALID_POLICY_PARAM,
            invalid_value(key, value, expected),
        );
    }

    /// An error about the value of `key`, which is then `rejected`. This is
    /// the one way a key becomes rejected, so none is without an error beside
    /// it.
    pub fn error_on(&mut self, key: &str, code: &'static str, message: String) {
        self.rejected.insert(key.to_ascii_lowercase());
        self.error(code, message);
    }

    /// Whether the value of `key` was reported as an error. That error is
    /// the one thing said of the parameter, and of what its value decides: a
    /// warning that it does not apply, or that another parameter has no
    /// effect because of it, is left unsaid.
    pub fn rejected(&self, key: &str) -> bool {
        self.rejected.contains(&key.to_ascii_lowercase())
    }

    pub fn error(&mut self, code: &'static str, message: String) {
        let message = format!("{}: {message}", named("policy", &self.policy.name));
        self.diags
            .push(Diagnostic::error(code, message).at(self.policy.span.clone()));
    }

    pub fn warn(&mut self, code: &'static str, message: String) {
        let message = format!("{}: {message}", named("policy", &self.policy.name));
        self.diags
            .push(Diagnostic::warning(code, message).at(self.policy.span.clone()));
    }

    pub fn has_errors(&self) -> bool {
        self.diags.iter().any(|d| d.severity == Severity::Error)
    }

    /// Warns about every parameter and positional value nobody read.
    pub fn finish(mut self) -> Vec<Diagnostic> {
        let policy = self.policy;
        let mut reported = HashSet::new();
        for (key, _) in policy.params.iter() {
            if !self.used.contains(key) && reported.insert(key.to_string()) {
                let message = if written_as_name(&policy.definition, key) {
                    format!("unknown parameter `{key}` ignored")
                } else {
                    "unknown parameter ignored".to_string()
                };
                self.warn(codes::W_UNKNOWN_KEY, message);
            }
        }
        for index in self.positional_used..policy.positional.len() {
            self.warn(
                codes::W_UNKNOWN_KEY,
                format!("unexpected positional value #{} ignored", index + 1),
            );
        }
        self.diags
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Severity, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use std::path::Path;
    use std::sync::Arc;

    fn policy(def: &str) -> crate::policy::ProxyPolicy {
        parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 7)).unwrap()
    }

    #[test]
    fn typed_reads_validate_and_remember_the_key() {
        let p = policy("http, 1.2.3.4, 80, tfo=true, tos=0x10, ip-version=V4-Only, bad=maybe");
        let mut r = ParamReader::new(&p);
        assert_eq!(r.bool("tfo"), Some(true));
        assert_eq!(r.str("tos"), Some("0x10"));
        let table = [("dual", 0u8), ("v4-only", 1u8)];
        assert_eq!(r.choice("ip-version", &table), Some(1));
        assert_eq!(r.bool("bad"), None);
        assert_eq!(r.bool("absent"), None);
        assert!(r.has_errors());
        let diags = r.finish();
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].code, codes::E_INVALID_POLICY_PARAM);
        assert_eq!(diags[0].severity, Severity::Error);
        assert_eq!(
            diags[0].message,
            "policy `P`: invalid value `maybe` for `bad` (expected true or false)"
        );
        assert_eq!(diags[0].span.as_ref().unwrap().line, 7);
    }

    /// A value is quoted when it can be what somebody wrote for the one key
    /// it stands at. A comma that is missing lets the next field slide into
    /// it (`4 psk=…`); a space, an `=` and a length no value of a parameter
    /// has do not fit one key.
    #[test]
    fn a_value_is_quoted_only_when_it_can_be_what_was_written_for_the_key() {
        let longest = "a".repeat(64);
        let too_long = "a".repeat(65);
        let wide = "é".repeat(64);
        for (value, quoted) in [
            ("maybe", true),
            ("", true),
            (longest.as_str(), true),
            (too_long.as_str(), false),
            // 64 characters, twice as many bytes
            (wide.as_str(), true),
            ("true password", false),
            ("4 psk=S3CR3T", false),
            ("a\u{3000}b", false),
            ("a\tb", false),
            ("a=b", false),
            ("psk=S3CR3T", false),
            // the field after a `，` is in the same value
            ("true\u{ff0c}S3CR3T", false),
            ("true\u{ff0c}psk\u{ff1d}S3CR3T", false),
            ("psk\u{ff1d}S3CR3T", false),
        ] {
            let p = policy("http, 1.2.3.4, 80");
            let mut r = ParamReader::new(&p);
            r.invalid("bad", value, "true or false");
            let diags = r.finish();
            let expected = if quoted {
                format!("policy `P`: invalid value `{value}` for `bad` (expected true or false)")
            } else {
                "policy `P`: invalid value for `bad` (expected true or false)".to_string()
            };
            assert_eq!(diags[0].message, expected, "{value:?}");
        }
        // through the typed reads as well: the value another field slid into
        let p = policy("http, 1.2.3.4, 80, tfo=true password=S3CR3T, test-timeout=5 psk=S3CR3T");
        let mut r = ParamReader::new(&p);
        assert_eq!(r.bool("tfo"), None);
        assert_eq!(r.number::<u32>("test-timeout", "seconds"), None);
        let messages: Vec<String> = r.finish().into_iter().map(|d| d.message).collect();
        assert_eq!(
            messages,
            [
                "policy `P`: invalid value for `tfo` (expected true or false)",
                "policy `P`: invalid value for `test-timeout` (expected seconds)",
            ]
        );
    }

    #[test]
    fn a_value_reported_as_an_error_is_remembered() {
        let p = policy("http, 1.2.3.4, 80, tfo=maybe, tos=1, ip-version=v5");
        let mut r = ParamReader::new(&p);
        assert_eq!(r.bool("tfo"), None);
        assert_eq!(r.choice("ip-version", &[("dual", 0u8)]), None);
        assert_eq!(r.str("tos"), Some("1"));
        assert!(r.rejected("tfo") && r.rejected("TFO") && r.rejected("ip-version"));
        assert!(!r.rejected("tos") && !r.rejected("absent"));
        // a parameter a warning may be said of: written, and not an error
        assert!(r.has_unrejected("tos"));
        assert!(!r.has_unrejected("tfo") && !r.has_unrejected("absent"));
        // an error that is not `invalid`'s is the other way to reject
        r.error_on(
            "Underlying-Proxy",
            codes::E_UNKNOWN_POLICY_REF,
            "x".to_string(),
        );
        assert!(r.rejected("underlying-proxy") && r.has_errors());
    }

    #[test]
    fn unread_parameters_and_extra_positionals_are_warned_about_once() {
        let p = policy("http, 1.2.3.4, 80, user, pass, surplus, mystery=1, mystery=2, known=x");
        let mut r = ParamReader::new(&p);
        assert_eq!(r.positional(0), Some("user"));
        assert_eq!(r.positional(1), Some("pass"));
        assert_eq!(r.positional(5), None);
        r.touch("known");
        assert!(!r.has_errors());
        let messages: Vec<String> = r.finish().into_iter().map(|d| d.message).collect();
        assert_eq!(
            messages,
            [
                "policy `P`: unknown parameter `mystery` ignored",
                // the value itself is never echoed: it may be a secret
                "policy `P`: unexpected positional value #3 ignored",
            ]
        );
    }

    #[test]
    fn a_repeated_parameter_is_read_whole_and_in_order() {
        let p = policy("external, exec=/bin/p, args=-D, args=1080, args=-N");
        let mut r = ParamReader::new(&p);
        assert_eq!(r.all("args"), ["-D", "1080", "-N"]);
        assert!(r.all("absent").is_empty());
        r.touch("exec");
        // read once, not reported as unknown
        assert!(r.finish().is_empty());
    }

    #[test]
    fn numbers_report_what_was_expected() {
        let p = policy("http, 1.2.3.4, 80, test-timeout=soon");
        let mut r = ParamReader::new(&p);
        assert_eq!(r.number::<u32>("test-timeout", "seconds"), None);
        let diags = r.finish();
        assert_eq!(
            diags[0].message,
            "policy `P`: invalid value `soon` for `test-timeout` (expected seconds)"
        );
    }

    /// The name of an unknown parameter is quoted only when the item writes it
    /// as a parameter name — ASCII lower-case letters, digits, `-` and `_`, at
    /// most 32 of them. The reader holds the key lower-cased and unquoted, so
    /// what somebody wrote as a password (`aHVudGVyMg==` reads as
    /// `ahvudgvymg`, the quoted `"pa=ss"` as `pa`) is told by the line.
    #[test]
    fn the_name_of_an_unknown_parameter_is_quoted_only_when_it_is_written_as_one() {
        let longest = "a".repeat(32);
        let too_long = "a".repeat(33);
        let said = |definition: &str| -> Vec<String> {
            let p = policy(definition);
            ParamReader::new(&p)
                .finish()
                .into_iter()
                .map(|d| d.message)
                .collect()
        };
        let named = |name: &str| vec![format!("policy `P`: unknown parameter `{name}` ignored")];
        let nameless = || vec!["policy `P`: unknown parameter ignored".to_string()];
        assert_eq!(said("http, 1.2.3.4, 80, mystery=1"), named("mystery"));
        assert_eq!(said("http, 1.2.3.4, 80, my-key_2=1"), named("my-key_2"));
        // 32 characters is a name, 33 is not
        assert_eq!(
            said(&format!("http, 1.2.3.4, 80, {longest}=1")),
            named(&longest)
        );
        assert_eq!(
            said(&format!("http, 1.2.3.4, 80, {too_long}=1")),
            nameless()
        );
        // upper case is not how a name is written: a Base64 password reads so
        assert_eq!(said("http, 1.2.3.4, 80, aHVudGVyMg=="), nameless());
        assert_eq!(said("http, 1.2.3.4, 80, Mystery=1"), nameless());
        // a whole item in quotes holds a name no more
        assert_eq!(said("http, 1.2.3.4, 80, \"pa=ss\""), nameless());
        assert_eq!(said("http, 1.2.3.4, 80, 'pa=ss'"), nameless());
        // quotes around the value only are no matter; blanks around the name
        // are none of it; other characters are
        assert_eq!(said("http, 1.2.3.4, 80, mystery=\"a,b\""), named("mystery"));
        assert_eq!(
            said("http, 1.2.3.4, 80,\u{3000}mystery\u{3000}=1"),
            named("mystery")
        );
        assert_eq!(said("http, 1.2.3.4, 80, my.key=1"), nameless());
        // a name written two ways: the one that is no name decides
        assert_eq!(said("http, 1.2.3.4, 80, mystery=1, MYSTERY=2"), nameless());
    }
}
