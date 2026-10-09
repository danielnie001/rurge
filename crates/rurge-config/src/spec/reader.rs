//! Typed reads over one policy's parameters. Remembers which keys were read
//! so that `finish` can warn about the rest.

use crate::diagnostic::{Diagnostic, Severity, codes};
use crate::policy::ProxyPolicy;
use crate::value::parse_bool;
use std::collections::HashSet;
use std::str::FromStr;

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

    /// `E0018`. Never call this for a parameter whose value is a secret.
    pub fn invalid(&mut self, key: &str, value: &str, expected: &str) {
        self.error_on(
            key,
            codes::E_INVALID_POLICY_PARAM,
            format!("invalid value `{value}` for `{key}` (expected {expected})"),
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
        let message = format!("policy `{}`: {message}", self.policy.name);
        self.diags
            .push(Diagnostic::error(code, message).at(self.policy.span.clone()));
    }

    pub fn warn(&mut self, code: &'static str, message: String) {
        let message = format!("policy `{}`: {message}", self.policy.name);
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
                self.warn(
                    codes::W_UNKNOWN_KEY,
                    format!("unknown parameter `{key}` ignored"),
                );
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
}
