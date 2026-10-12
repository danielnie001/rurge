pub mod include;

use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::reach::Reach;
use crate::requirement;
use crate::span::Span;
use crate::value::starts_with_ci;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Main,
    Include(Arc<Path>),
    Module(String),
    /// A line of a text that was downloaded: the file that holds it.
    Remote(Arc<Path>),
    /// A line of the downloaded copy of the managed profile, and the file that
    /// holds it. While the copy is in effect it is the profile itself, unlike
    /// the text of a remote include, so it may include local files below the
    /// profile's directory.
    Managed(Arc<Path>),
    /// A line of a local file that the downloaded copy of the managed profile
    /// takes in by `#!include`, directly or through other such files, and the
    /// file. Being taken in gives a text no rights: it is read as the copy is.
    ManagedInclude(Arc<Path>),
}

impl Origin {
    /// What a line of this origin may make rurge do on this machine. The one
    /// place that maps an origin to a reach: the variant decides, nothing else.
    pub fn reach(&self) -> Reach {
        match self {
            Origin::Main | Origin::Include(_) => Reach::Anywhere,
            Origin::Managed(_) | Origin::ManagedInclude(_) => Reach::BelowProfile,
            // nothing loads modules yet; the answer that does no harm is the
            // one for text of no known origin
            Origin::Module(_) | Origin::Remote(_) => Reach::Nowhere,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionKind {
    KeyValue,
    Ordered,
    Unknown,
}

/// The line as the redacted profile shows it: a section rurge does not
/// read yet keeps its lines as written, secrets included.
impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Entry {
            raw,
            span,
            origin,
            requirement,
            disabled,
        } = self;
        f.debug_struct("Entry")
            .field("raw", &crate::redact::redact_line(raw))
            .field("span", span)
            .field("origin", origin)
            .field("requirement", requirement)
            .field("disabled", disabled)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
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

#[derive(Clone, PartialEq, Eq)]
pub struct Directive {
    pub raw: String,
    pub span: Span,
}

/// The line as the redacted profile shows it: a link on a `#!` line can
/// carry a token.
impl std::fmt::Debug for Directive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Directive { raw, span } = self;
        f.debug_struct("Directive")
            .field("raw", &crate::redact::redact_line(raw))
            .field("span", span)
            .finish()
    }
}

/// The name of a `#!` line: the `#!` and the run of ASCII letters, digits,
/// `-` and `_` right after it, which can be empty. What follows can be a link
/// that carries a token (a misspelt `#!MANAGED-CONFIG` is an unknown
/// directive), glued to the name or not, so a diagnostic and the redacted
/// profile both go by the name and by nothing more. `""` for a line that does
/// not start with `#!`.
pub(crate) fn directive_name(line: &str) -> &str {
    let Some(rest) = line.strip_prefix("#!") else {
        return "";
    };
    let run = rest
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        .count();
    &line[..2 + run]
}

/// What a diagnostic says of a `#!` line it ignores: the directive's name
/// when that can be one. A run of letters, digits, `-` and `_` longer than
/// 32 characters is no name anyone wrote, and is not quoted.
pub(crate) fn unknown_directive(line: &str) -> String {
    let name = directive_name(line);
    if name.len().saturating_sub(2) <= 32 {
        format!("unknown directive ignored: {name}")
    } else {
        "unknown directive ignored".to_string()
    }
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

/// Parse profile text. Never fails; problems are reported as diagnostics.
pub fn parse_str(text: &str, file: Arc<Path>, origin: Origin) -> (Profile, Diagnostics) {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
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
            && !requirement::is_requirement_prefix(trimmed)
        {
            match current.as_mut() {
                None => profile.header.push(Directive {
                    raw: trimmed.to_string(),
                    span,
                }),
                Some(_) => diags.push(
                    Diagnostic::warning(codes::W_UNKNOWN_DIRECTIVE, unknown_directive(trimmed))
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

    /// Inside a double-quoted value `\"` is a quote of the value and `\\` a
    /// backslash of it: a comment delimiter after either, still inside the
    /// quotes, starts nothing, and the one after the closing quote does.
    #[test]
    fn an_escaped_quote_does_not_end_a_quoted_value() {
        assert_eq!(
            strip_inline_comment("x = \"a \\\" // b\" // c"),
            "x = \"a \\\" // b\""
        );
        assert_eq!(
            strip_inline_comment("x = \"a \\\\\" // c"),
            "x = \"a \\\\\""
        );
        assert_eq!(
            strip_inline_comment("x = \"a \\\\ // b\" ; c"),
            "x = \"a \\\\ // b\""
        );
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
    fn utf8_bom_is_stripped_before_parsing() {
        let (p, d) =
            parse("\u{FEFF}[General]\r\nloglevel = notify\r\n\r\n[Rule]\r\nFINAL,DIRECT\r\n");
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert_eq!(p.sections.len(), 2);
        assert_eq!(p.sections[0].name, "General");
        assert_eq!(p.sections[0].entries.len(), 1);
        assert_eq!(p.sections[0].entries[0].raw, "loglevel = notify");
        assert_eq!(p.sections[1].name, "Rule");
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

    /// The name is the `#!` and the run of letters, digits, `-` and `_`
    /// right after it: a link or a token glued to it, or a space in between,
    /// does not make it longer.
    #[test]
    fn a_directive_is_named_by_the_run_after_its_hash_bang() {
        for (line, name) in [
            ("#!MANAGED-CONFIG https://x.test/y", "#!MANAGED-CONFIG"),
            ("#!include a.conf", "#!include"),
            ("#!", "#!"),
            ("#! MANAGED-CONFIG https://x.test/y", "#!"),
            ("#!MANAGED_CONFIG=https://x.test/y", "#!MANAGED_CONFIG"),
            ("#!MANAGED-CONFIGhttps://x.test/y", "#!MANAGED-CONFIGhttps"),
            ("#!a_b-9Z\u{200b}x", "#!a_b-9Z"),
            ("#!é", "#!"),
            ("not a directive", ""),
        ] {
            assert_eq!(directive_name(line), name, "{line:?}");
        }
    }

    /// A diagnostic names the directive it ignores, and nothing of what
    /// follows; a run longer than a name can be is no name anyone wrote.
    #[test]
    fn an_unknown_directive_is_named_only_when_that_can_be_a_name() {
        let longest = "A".repeat(32);
        let too_long = "A".repeat(33);
        let (_, d) = parse(&format!(
            "[General]\n#!{longest} x\n#!{too_long} x\n#! x\n#!MANAGED_CONFIG=https://x.test/S3CR3T\n#!MANAGED-CONFIG https://x.test/S3CR3T\n"
        ));
        let messages: Vec<String> = d.into_vec().into_iter().map(|d| d.message).collect();
        assert_eq!(
            messages,
            [
                format!("unknown directive ignored: #!{longest}"),
                "unknown directive ignored".to_string(),
                "unknown directive ignored: #!".to_string(),
                "unknown directive ignored: #!MANAGED_CONFIG".to_string(),
                "unknown directive ignored: #!MANAGED-CONFIG".to_string(),
            ]
        );
    }

    /// `Profile` is public and holds its header: its `Debug` shows a line as
    /// the redacted profile does.
    #[test]
    fn a_directive_shows_its_line_as_the_redacted_profile_would() {
        let (p, _) = parse(
            "#!MANAGED-CONFIG https://x.test/S3CR3T interval=60\n#!MANAGED_CONFIG=https://x.test/S3CR3T\n#!NOTE\n[General]\n",
        );
        let shown = format!("{:?}", p.header);
        assert!(!shown.contains("S3CR3T"), "{shown}");
        assert!(
            shown.contains("#!MANAGED-CONFIG *** interval=60"),
            "{shown}"
        );
        assert!(shown.contains("#!MANAGED_CONFIG***"), "{shown}");
        assert!(shown.contains("#!NOTE"), "{shown}");
        assert!(!format!("{p:?}").contains("S3CR3T"));
    }

    /// What a line may make rurge do on this machine follows from where its
    /// text came from: the user's own text anything, the downloaded copy of a
    /// managed profile and the local files it takes in the local files below
    /// the profile's directory, any other downloaded text nothing.
    #[test]
    fn an_origin_decides_what_its_lines_may_do() {
        let file: Arc<Path> = Arc::from(Path::new("f.conf"));
        for (origin, reach) in [
            (Origin::Main, Reach::Anywhere),
            (Origin::Include(file.clone()), Reach::Anywhere),
            (Origin::Managed(file.clone()), Reach::BelowProfile),
            (Origin::ManagedInclude(file.clone()), Reach::BelowProfile),
            (Origin::Remote(file), Reach::Nowhere),
            (Origin::Module("m".to_string()), Reach::Nowhere),
        ] {
            assert_eq!(origin.reach(), reach, "{origin:?}");
        }
    }
}
