pub mod include;

use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::requirement;
use crate::span::Span;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Main,
    Include(Arc<Path>),
    Module(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionKind {
    KeyValue,
    Ordered,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Trimmed line content without inline comment and without requirement directive.
    pub raw: String,
    pub span: Span,
    pub origin: Origin,
    /// Requirement expression source attached to this line, if any.
    pub requirement: Option<String>,
    /// Set by requirement evaluation when the expression is not satisfied.
    pub disabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub kind: SectionKind,
    pub span: Span,
    pub entries: Vec<Entry>,
}

impl Section {
    pub fn active_entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| !e.disabled)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Directive {
    pub raw: String,
    pub span: Span,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub main: Option<Arc<Path>>,
    pub header: Vec<Directive>,
    pub sections: Vec<Section>,
}

impl Profile {
    /// Case-insensitive lookup of the first section with this name.
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    }
    pub fn section_mut(&mut self, name: &str) -> Option<&mut Section> {
        self.sections
            .iter_mut()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    }
    /// Sections whose name starts with `prefix` (e.g. "Ruleset "), in file order.
    pub fn sections_with_prefix<'a>(
        &'a self,
        prefix: &'a str,
    ) -> impl Iterator<Item = &'a Section> + 'a {
        self.sections
            .iter()
            .filter(move |s| starts_with_ci(&s.name, prefix))
    }
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

const KEY_VALUE_SECTIONS: &[&str] = &[
    "General",
    "Proxy",
    "Proxy Group",
    "MITM",
    "Keystore",
    "Ponte",
    "Testing",
    "DHCP",
    "Snell Server",
    "MTProto",
];
const ORDERED_SECTIONS: &[&str] = &[
    "Rule",
    "Host",
    "URL Rewrite",
    "Header Rewrite",
    "Body Rewrite",
    "Map Local",
    "Panel",
    "Port Forwarding",
    "Script",
    "SSID Setting",
];

pub fn section_kind(name: &str) -> SectionKind {
    if KEY_VALUE_SECTIONS
        .iter()
        .any(|k| k.eq_ignore_ascii_case(name))
        || starts_with_ci(name, "WireGuard ")
        || starts_with_ci(name, "Tailscale ")
    {
        SectionKind::KeyValue
    } else if ORDERED_SECTIONS
        .iter()
        .any(|k| k.eq_ignore_ascii_case(name))
        || starts_with_ci(name, "Ruleset ")
    {
        SectionKind::Ordered
    } else {
        SectionKind::Unknown
    }
}

/// A line is a comment if it starts with `#` (not `#!`), `;`, or `//` (not `//!`).
pub fn is_comment(line: &str) -> bool {
    (line.starts_with('#') && !line.starts_with("#!"))
        || line.starts_with(';')
        || (line.starts_with("//") && !line.starts_with("//!"))
}

/// Remove an inline comment (` #`, ` ;`, ` //` preceded by whitespace, outside double quotes).
/// ` #!` and ` //!` are requirement directives, not comments.
pub fn strip_inline_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_quotes = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_quotes {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == b'"' {
                in_quotes = false;
            }
            i += 1;
            continue;
        }
        if c == b'"' {
            in_quotes = true;
            i += 1;
            continue;
        }
        let prev_is_space = i > 0 && bytes[i - 1].is_ascii_whitespace();
        if prev_is_space {
            let next = bytes.get(i + 1).copied();
            let is_directive = |n: Option<u8>| n == Some(b'!');
            if c == b';' {
                return line[..i].trim_end();
            }
            if c == b'#' && !is_directive(next) {
                return line[..i].trim_end();
            }
            if c == b'/' && next == Some(b'/') && !is_directive(bytes.get(i + 2).copied()) {
                return line[..i].trim_end();
            }
        }
        i += 1;
    }
    line
}

fn section_header(line: &str) -> Option<&str> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?.trim();
    (!inner.is_empty()).then_some(inner)
}

fn is_requirement_prefix(line: &str) -> bool {
    ["#!REQUIREMENT", "#!IOS-ONLY", "#!MACOS-ONLY", "#!TVOS-ONLY"]
        .iter()
        .any(|p| line.starts_with(p))
}

/// Parse profile text. Never fails; problems are reported as diagnostics.
pub fn parse_str(text: &str, file: Arc<Path>, origin: Origin) -> (Profile, Diagnostics) {
    let mut profile = Profile {
        main: Some(file.clone()),
        header: Vec::new(),
        sections: Vec::new(),
    };
    let mut diags = Diagnostics::default();
    let mut current: Option<Section> = None;

    for (idx, line) in text.lines().enumerate() {
        let span = Span::new(file.clone(), idx as u32 + 1);
        let trimmed = line.trim_end_matches('\r').trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(name) = section_header(trimmed) {
            if let Some(sec) = current.take() {
                profile.sections.push(sec);
            }
            current = Some(Section {
                name: name.to_string(),
                kind: section_kind(name),
                span,
                entries: Vec::new(),
            });
            continue;
        }
        if trimmed.starts_with("#!")
            && !trimmed.starts_with("#!include")
            && !is_requirement_prefix(trimmed)
        {
            match current.as_mut() {
                None => profile.header.push(Directive {
                    raw: trimmed.to_string(),
                    span,
                }),
                Some(_) => diags.push(
                    Diagnostic::warning(
                        codes::W_UNKNOWN_DIRECTIVE,
                        format!("unknown directive ignored: {trimmed}"),
                    )
                    .at(span),
                ),
            }
            continue;
        }
        if is_comment(trimmed) {
            continue;
        }
        let Some(sec) = current.as_mut() else {
            diags.push(
                Diagnostic::warning(
                    codes::W_LINE_OUTSIDE_SECTION,
                    "line outside of any section is ignored",
                )
                .at(span),
            );
            continue;
        };
        let content = strip_inline_comment(trimmed);
        match requirement::split_line(content) {
            Ok((requirement, body)) => {
                if body.is_empty() {
                    continue;
                }
                sec.entries.push(Entry {
                    raw: body,
                    span,
                    origin: origin.clone(),
                    requirement,
                    disabled: false,
                });
            }
            Err(e) => {
                diags.push(Diagnostic::error(codes::E_REQUIREMENT_SYNTAX, e.to_string()).at(span))
            }
        }
    }
    if let Some(sec) = current.take() {
        profile.sections.push(sec);
    }
    (profile, diags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn parse(text: &str) -> (Profile, Diagnostics) {
        parse_str(text, Arc::from(Path::new("t.conf")), Origin::Main)
    }

    #[test]
    fn sections_entries_and_comments() {
        let text = "\
#!MANAGED-CONFIG https://x/y interval=60
# a comment
[General]
loglevel = notify // inline
dns-server = 8.8.8.8 # inline
; another comment
[Rule]
DOMAIN,a.com,DIRECT
// comment
FINAL,DIRECT
[Weird Section]
whatever = 1
";
        let (p, d) = parse(text);
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert_eq!(p.header.len(), 1);
        assert_eq!(p.header[0].raw, "#!MANAGED-CONFIG https://x/y interval=60");
        assert_eq!(p.sections.len(), 3);
        let g = p.section("general").unwrap();
        assert_eq!(g.kind, SectionKind::KeyValue);
        assert_eq!(g.entries[0].raw, "loglevel = notify");
        assert_eq!(g.entries[0].span.line, 4);
        assert_eq!(g.entries[1].raw, "dns-server = 8.8.8.8");
        let r = p.section("Rule").unwrap();
        assert_eq!(r.kind, SectionKind::Ordered);
        assert_eq!(r.entries.len(), 2);
        assert_eq!(p.sections[2].kind, SectionKind::Unknown);
        assert_eq!(p.sections[2].entries[0].raw, "whatever = 1");
    }

    #[test]
    fn inline_comment_rules() {
        assert_eq!(
            strip_inline_comment("dns-server = 8.8.8.8 // c"),
            "dns-server = 8.8.8.8"
        );
        assert_eq!(
            strip_inline_comment("dns-server = 8.8.8.8 # c"),
            "dns-server = 8.8.8.8"
        );
        assert_eq!(
            strip_inline_comment("dns-server = 8.8.8.8 ; c"),
            "dns-server = 8.8.8.8"
        );
        assert_eq!(
            strip_inline_comment("url = http://a/b#c"),
            "url = http://a/b#c"
        );
        assert_eq!(
            strip_inline_comment("x = \"a // b\" // c"),
            "x = \"a // b\""
        );
        assert_eq!(
            strip_inline_comment("DOMAIN,a,REJECT #!MACOS-ONLY"),
            "DOMAIN,a,REJECT #!MACOS-ONLY"
        );
        assert_eq!(
            strip_inline_comment("G = url-test, A //!REQUIREMENT CORE_VERSION<22"),
            "G = url-test, A //!REQUIREMENT CORE_VERSION<22"
        );
        assert!(is_comment("# x"));
        assert!(is_comment("; x"));
        assert!(is_comment("// x"));
        assert!(!is_comment("#!include a.dconf"));
        assert!(!is_comment("//!REQUIREMENT x"));
    }

    #[test]
    fn named_sections_and_prefix_lookup() {
        let (p, _) = parse(
            "[Ruleset Streaming]\nDOMAIN-SUFFIX,netflix.com\n[WireGuard home]\nmtu = 1280\n[Ruleset Other]\n",
        );
        let names: Vec<_> = p
            .sections_with_prefix("Ruleset ")
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(names, ["Ruleset Streaming", "Ruleset Other"]);
        assert_eq!(
            p.section("WireGuard home").unwrap().kind,
            SectionKind::KeyValue
        );
        assert_eq!(
            p.section("Ruleset Streaming").unwrap().kind,
            SectionKind::Ordered
        );
    }

    #[test]
    fn requirement_directives_are_attached_to_entries() {
        let (p, d) = parse(
            "[Rule]\n#!MACOS-ONLY DOMAIN,a.com,REJECT\nDOMAIN,b.com,REJECT #!IOS-ONLY\nFINAL,DIRECT\n",
        );
        assert!(d.is_empty());
        let r = p.section("Rule").unwrap();
        assert_eq!(r.entries[0].raw, "DOMAIN,a.com,REJECT");
        assert_eq!(
            r.entries[0].requirement.as_deref(),
            Some("SYSTEM == 'macOS'")
        );
        assert_eq!(r.entries[1].requirement.as_deref(), Some("SYSTEM == 'iOS'"));
        assert_eq!(r.entries[2].requirement, None);
    }

    #[test]
    fn lines_outside_sections_warn_and_crlf_is_stripped() {
        let (p, d) = parse("stray = 1\r\n[General]\r\nipv6 = true\r\n");
        assert_eq!(d.iter().next().unwrap().code, codes::W_LINE_OUTSIDE_SECTION);
        assert_eq!(p.section("General").unwrap().entries[0].raw, "ipv6 = true");
    }

    #[test]
    fn non_ascii_section_names_do_not_panic() {
        assert_eq!(section_kind("中文中文中文"), SectionKind::Unknown);
        assert_eq!(section_kind("Ruleset 流媒体"), SectionKind::Ordered);
        let (p, d) = parse("[中文中文中文]\nkey = value\n");
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert_eq!(p.sections.len(), 1);
        assert_eq!(p.sections[0].kind, SectionKind::Unknown);
        assert_eq!(p.sections[0].entries.len(), 1);
    }
}
