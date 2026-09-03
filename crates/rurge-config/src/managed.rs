//! `#!MANAGED-CONFIG` and other header directives.

use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::text::Directive;
use crate::value::parse_bool;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedConfig {
    pub url: String,
    pub interval: Duration,
    pub strict: bool,
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
                interval: Duration::from_secs(86400),
                strict: false,
            };
            for p in parts {
                match p.split_once('=') {
                    Some(("interval", v)) => match v.parse::<u64>() {
                        Ok(s) => cfg.interval = Duration::from_secs(s),
                        Err(_) => diags.push(
                            Diagnostic::warning(
                                codes::W_INVALID_VALUE,
                                format!("invalid interval `{v}`"),
                            )
                            .at(d.span.clone()),
                        ),
                    },
                    Some(("strict", v)) => match parse_bool(v) {
                        Some(b) => cfg.strict = b,
                        None => diags.push(
                            Diagnostic::warning(
                                codes::W_INVALID_VALUE,
                                format!("invalid strict `{v}`"),
                            )
                            .at(d.span.clone()),
                        ),
                    },
                    _ => diags.push(
                        Diagnostic::warning(
                            codes::W_INVALID_VALUE,
                            format!("unknown MANAGED-CONFIG parameter `{p}`"),
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
                Diagnostic::warning(
                    codes::W_UNKNOWN_DIRECTIVE,
                    format!("unknown directive ignored: {}", d.raw),
                )
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
}
