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
    pub hint: Option<String>,
}

impl Diagnostic {
    pub fn new(severity: Severity, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity,
            code,
            message: message.into(),
            span: None,
            hint: None,
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
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}[{}]", self.severity, self.code)?;
        if let Some(span) = &self.span {
            write!(f, " {span}")?;
        }
        write!(f, ": {}", self.message)?;
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
}

impl ParseError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl Diagnostic {
    pub fn from_parse(err: ParseError, span: Span) -> Diagnostic {
        Diagnostic::error(err.code, err.message).at(span)
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
    pub const E_GROUP_CYCLE: &str = "E0009";
    pub const E_MISSING_FINAL: &str = "E0010";
    pub const E_INVALID_RULE_VALUE: &str = "E0011";
    pub const E_UNKNOWN_RULE_TYPE: &str = "E0012";
    pub const E_LISTENER_NOT_IP: &str = "E0013";
    pub const E_NOT_ALLOWED_HERE: &str = "E0014";
    pub const E_NESTING_TOO_DEEP: &str = "E0015";
    pub const E_INCLUDE_CYCLE: &str = "E0016";
    pub const E_INVALID_DEFINITION: &str = "E0017";
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
    pub const W_REMOTE_INCLUDE_UNSUPPORTED: &str = "W0011";
    pub const W_INVALID_VALUE: &str = "W0012";
    pub const W_RULE_NEVER_MATCHES: &str = "W0013";
    pub const W_UNKNOWN_DIRECTIVE: &str = "W0014";
    pub const W_LINE_OUTSIDE_SECTION: &str = "W0015";
    pub const W_DEFERRED_SECTION: &str = "W0016";
    pub const W_INCLUDE_SECTION_MISSING: &str = "W0017";
    pub const I_LEGACY_MIGRATED: &str = "I0001";
    pub const I_LINE_DISABLED: &str = "I0002";
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;

    fn span(line: u32) -> Span {
        Span::new(Arc::from(Path::new("a.conf")), line)
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
}
