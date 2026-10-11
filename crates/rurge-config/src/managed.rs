//! `#!MANAGED-CONFIG` and other header directives.

use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::remote::DEFAULT_INTERVAL;
use crate::text::{Directive, unknown_directive};
use crate::value::parse_bool;
use std::fmt;
use std::time::Duration;

#[derive(Clone, PartialEq, Eq)]
pub struct ManagedConfig {
    pub url: String,
    pub interval: Duration,
    pub strict: bool,
}

/// The URL is left blank: the link a provider issues carries the
/// subscription token.
impl fmt::Debug for ManagedConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ManagedConfig {
            url: _,
            interval,
            strict,
        } = self;
        f.debug_struct("ManagedConfig")
            .field("url", &"***")
            .field("interval", interval)
            .field("strict", strict)
            .finish()
    }
}

/// What a diagnostic says of a parameter it does not know: its name, when that
/// is a lower-case word like the ones the format has (`interval`, `strict`).
/// What follows the `=` can be a token, and so can a word that is no
/// `name=value` at all: a link pasted twice has its `=` somewhere in its
/// query, a Base64 string has it for padding.
fn describe(parameter: &str) -> String {
    match parameter.split_once('=') {
        Some((name, _))
            if (1..=32).contains(&name.len())
                && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') =>
        {
            format!("`{name}`")
        }
        _ => "(not written as a lower-case `name=value`)".to_string(),
    }
}

/// The value of `interval` or `strict` in a diagnostic, when it can be what
/// somebody wrote for that parameter: a short run of `plain` characters. A
/// link or a token that lost its space is glued to it otherwise.
fn quoted(value: &str, plain: impl Fn(char) -> bool) -> String {
    if (1..=20).contains(&value.len()) && value.chars().all(plain) {
        format!(" `{value}`")
    } else {
        String::new()
    }
}

pub fn parse_managed(header: &[Directive], diags: &mut Diagnostics) -> Option<ManagedConfig> {
    let mut managed = None;
    for d in header {
        if let Some(rest) = d.raw.strip_prefix("#!MANAGED-CONFIG") {
            let mut parts = rest.split_whitespace();
            let Some(url) = parts.next() else {
                diags.push(
                    Diagnostic::warning(
                        codes::W_INVALID_VALUE,
                        "#!MANAGED-CONFIG without a URL is ignored",
                    )
                    .at(d.span.clone()),
                );
                continue;
            };
            let mut cfg = ManagedConfig {
                url: url.to_string(),
                interval: DEFAULT_INTERVAL,
                strict: false,
            };
            for p in parts {
                match p.split_once('=') {
                    Some(("interval", v)) => match v.parse::<u64>() {
                        Ok(s) => cfg.interval = Duration::from_secs(s),
                        Err(_) => diags.push(
                            Diagnostic::warning(
                                codes::W_INVALID_VALUE,
                                format!("invalid interval{}", quoted(v, |c| c.is_ascii_digit())),
                            )
                            .at(d.span.clone()),
                        ),
                    },
                    Some(("strict", v)) => match parse_bool(v) {
                        Some(b) => cfg.strict = b,
                        None => diags.push(
                            Diagnostic::warning(
                                codes::W_INVALID_VALUE,
                                format!("invalid strict{}", quoted(v, |c| c.is_ascii_alphabetic())),
                            )
                            .at(d.span.clone()),
                        ),
                    },
                    _ => diags.push(
                        Diagnostic::warning(
                            codes::W_INVALID_VALUE,
                            format!("unknown MANAGED-CONFIG parameter {}", describe(p)),
                        )
                        .at(d.span.clone()),
                    ),
                }
            }
            managed = Some(cfg);
        } else if d.raw.starts_with("#!FORBIDDEN-AUTO-UPGRADE") {
            // rurge never auto-upgrades profiles; nothing to do.
        } else {
            diags.push(
                Diagnostic::warning(codes::W_UNKNOWN_DIRECTIVE, unknown_directive(&d.raw))
                    .at(d.span.clone()),
            );
        }
    }
    managed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{Origin, parse_str};
    use std::path::Path;
    use std::sync::Arc;

    fn header(text: &str) -> (Option<ManagedConfig>, Diagnostics) {
        let (p, mut d) = parse_str(text, Arc::from(Path::new("m.conf")), Origin::Main);
        let m = parse_managed(&p.header, &mut d);
        (m, d)
    }

    #[test]
    fn managed_directive() {
        let (m, d) = header(
            "#!MANAGED-CONFIG http://test.com/surge.conf interval=60 strict=true\n#!FORBIDDEN-AUTO-UPGRADE smart-group\n[General]\n",
        );
        assert!(d.is_empty(), "{:?}", d.into_vec());
        let m = m.unwrap();
        assert_eq!(m.url, "http://test.com/surge.conf");
        assert_eq!(m.interval, Duration::from_secs(60));
        assert!(m.strict);
        let (m, _) = header("#!MANAGED-CONFIG https://x/y\n[General]\n");
        let m = m.unwrap();
        assert_eq!(m.interval, Duration::from_secs(86400));
        assert!(!m.strict);
        let (m, d) = header("#!SOMETHING-ELSE\n[General]\n");
        assert!(m.is_none());
        assert_eq!(d.iter().next().unwrap().code, codes::W_UNKNOWN_DIRECTIVE);
    }

    fn messages(d: Diagnostics) -> Vec<String> {
        d.into_vec().into_iter().map(|d| d.message).collect()
    }

    /// A directive in the header is named as one in a section is: by its
    /// run of letters, digits, `-` and `_`, if that can be a name.
    #[test]
    fn an_unknown_directive_is_named_only_when_that_can_be_a_name() {
        let longest = "A".repeat(32);
        let too_long = "A".repeat(33);
        let (m, d) = header(&format!(
            "#!{longest} x\n#!{too_long} x\n#!MANAGED_CONFIG=https://x.test/S3CR3T\n#! MANAGED-CONFIG https://x.test/S3CR3T\n[General]\n"
        ));
        assert!(m.is_none());
        assert_eq!(
            messages(d),
            [
                format!("unknown directive ignored: #!{longest}"),
                "unknown directive ignored".to_string(),
                "unknown directive ignored: #!MANAGED_CONFIG".to_string(),
                "unknown directive ignored: #!".to_string(),
            ]
        );
    }

    /// The value of a parameter the line knows is quoted when it is what
    /// somebody could have meant: a number, a word.
    #[test]
    fn an_invalid_interval_or_strictness_is_quoted_only_when_it_is_plain() {
        let (_, d) = header(
            "#!MANAGED-CONFIG https://x.test/y interval=99999999999999999999 interval=1h interval=60https://x.test/S3CR3T strict=maybe strict=1https://x.test/S3CR3T strict=\n[General]\n",
        );
        assert_eq!(
            messages(d),
            [
                "invalid interval `99999999999999999999`",
                "invalid interval",
                "invalid interval",
                "invalid strict `maybe`",
                "invalid strict",
                "invalid strict",
            ]
        );
    }

    /// What a word the line does not know is called: its name, when it is
    /// a lower-case word like the ones the format has (`interval`, `strict`);
    /// a token, a Base64 string whose padding is an `=` or a link pasted twice
    /// is not.
    #[test]
    fn an_unknown_parameter_is_named_only_when_it_is_a_lower_case_word() {
        let longest = "a".repeat(32);
        let too_long = "a".repeat(33);
        let (m, d) = header(&format!(
            "#!MANAGED-CONFIG https://x.test/y foo=1 foo-bar=2 {longest}=3 {too_long}=4 Foo=5 foo1=6 foo_bar=7 aGVsbG8= =8 bare https://x.test/S3CR3T?a=b token=S3CR3T\n[General]\n"
        ));
        assert!(m.is_some());
        let anonymous =
            "unknown MANAGED-CONFIG parameter (not written as a lower-case `name=value`)";
        assert_eq!(
            messages(d),
            [
                "unknown MANAGED-CONFIG parameter `foo`".to_string(),
                "unknown MANAGED-CONFIG parameter `foo-bar`".to_string(),
                format!("unknown MANAGED-CONFIG parameter `{longest}`"),
                anonymous.to_string(),
                anonymous.to_string(),
                anonymous.to_string(),
                anonymous.to_string(),
                anonymous.to_string(),
                anonymous.to_string(),
                anonymous.to_string(),
                anonymous.to_string(),
                "unknown MANAGED-CONFIG parameter `token`".to_string(),
            ]
        );
    }
}
