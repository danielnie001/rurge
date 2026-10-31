//! `[URL Rewrite]`, `[Header Rewrite]` and `[Body Rewrite]` (manual: Features ›
//! URL Rewrite, Features › Header Rewrite and Features › Body Rewrite): the
//! lines as written, read once at load time.
//!
//! A `[URL Rewrite]` line is `<regex> <replacement> [header | 302 | 307 |
//! reject]`; a `[Header Rewrite]` line is `[http-request | http-response]
//! <regex> <action> <name> …` with an action of `header-add`, `header-del`,
//! `header-replace` or `header-replace-regex`; a `[Body Rewrite]` line is
//! `[http-request | http-response] <regex> <body pattern> <replacement>
//! [<body pattern> <replacement> …]`, each pair applied in turn. A jq line
//! (`http-request-jq` / `http-response-jq`) gives `W0042` and is not read. The
//! three sections are read here the way the manual describes them (phase 4 M3
//! design 6.1, 7.1 and 9.1, the phase 4 / M3a plan's P6): the engine only
//! evaluates the rules this module keeps, and each rule keeps the span of its
//! line so that the engine can name it in the request's notes.
//!
//! A `[URL Rewrite]` replacement may hold `{{{GATEWAY_ADDRESS}}}`. Whether it
//! does is recorded here, at load time; the engine puts the gateway in only
//! for the `302` and `307` modes, where the manual allows the placeholder
//! (phase 4 M3 design 6.4).

use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::dialect::{BodyPattern, Template};
use crate::rule::Pattern;
use crate::span::Span;
use crate::text::Entry;
use crate::value::split_words;

/// `[URL Rewrite]`, `[Header Rewrite]` and `[Body Rewrite]`: the rules as
/// written, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RewriteConfig {
    pub url: Vec<UrlRewrite>,
    pub header: Vec<HeaderRewrite>,
    /// The lines of `[Body Rewrite]` that can be read, in the order written
    /// (phase 4 M3 design 9.1); a jq line gives `W0042` and is not one of
    /// them.
    pub body: Vec<BodyRewrite>,
}

impl RewriteConfig {
    /// Whether no section has a usable line.
    pub fn is_empty(&self) -> bool {
        self.url.is_empty() && self.header.is_empty() && self.body.is_empty()
    }
}

/// What a `[URL Rewrite]` line does with a request whose URL matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlMode {
    /// Send the request to the replacement's URL, keeping the policy.
    Header,
    /// Answer 302 Found with the replacement as `Location`.
    Found,
    /// Answer 307 Temporary Redirect with the replacement as `Location`.
    Temporary,
    /// Reject the request.
    Reject,
}

/// One `[URL Rewrite]` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UrlRewrite {
    pub pattern: Pattern,
    pub replacement: Template,
    pub mode: UrlMode,
    /// Whether the replacement holds `GATEWAY_ADDRESS`.
    pub gateway: bool,
    pub span: Span,
}

/// Which side of the exchange a `[Header Rewrite]` line acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Request,
    Response,
}

/// One `[Header Rewrite]` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderRewrite {
    pub direction: Direction,
    pub pattern: Pattern,
    pub action: HeaderAction,
    pub span: Span,
}

/// One `[Body Rewrite]` line (phase 4 M3 design 9.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyRewrite {
    pub direction: Direction,
    /// The expression for the URL the line takes effect on.
    pub pattern: Pattern,
    /// Each pair is a body pattern and the replacement it writes; the pairs
    /// apply in the order written, one after the other.
    pub pairs: Vec<(BodyPattern, Template)>,
    pub span: Span,
}

/// The placeholder a `[URL Rewrite]` replacement may hold: the IPv4 default
/// gateway, put in by the engine only for the `302` and `307` modes (phase 4
/// M3 design 6.4).
pub const GATEWAY_ADDRESS: &str = "{{{GATEWAY_ADDRESS}}}";

/// A `[URL Rewrite]` line that is not two or three words, or whose mode is
/// none of the four (phase 4 M3 design 6.1, the phase 4 / M3a plan's P7).
const URL_SHAPE: &str = "[URL Rewrite]: a line that is not `<regex> <replacement> [header | 302 | 307 | reject]` is ignored";
/// A `[URL Rewrite]` expression that does not compile; `{reason}` is why.
const URL_REGEX: &str =
    "[URL Rewrite]: the regular expression cannot be used ({reason}); the line is ignored";
/// A `[Header Rewrite]` line without a direction, expression, action and
/// name, or with an action other than the four (phase 4 M3 design 7.1).
const HEADER_SHAPE: &str = "[Header Rewrite]: a line that is not `[http-request | http-response] <regex> <action> <name> …` with an action of header-add, header-del, header-replace or header-replace-regex is ignored";
/// A name that is not an RFC 9110 token; `{name}` is the name as written.
const HEADER_NAME: &str =
    "[Header Rewrite]: `{name}` is not a header field name; the line is ignored";
/// `header-add` / `header-replace` without a value.
const HEADER_VALUE: &str = "[Header Rewrite]: header-add and header-replace take a value after the field name (write \"\" for an empty one); the line is ignored";
/// A value with a character a header cannot carry. The value is not shown.
const HEADER_CONTROL: &str = "[Header Rewrite]: the value has a control character a header cannot carry; the line is ignored";
/// `header-del` with anything after the name.
const HEADER_DEL_EXTRA: &str =
    "[Header Rewrite]: header-del takes a field name and nothing after it; the line is ignored";
/// `header-replace-regex` without its expression and replacement.
const HEADER_REGEX_SHAPE: &str = "[Header Rewrite]: header-replace-regex takes a field name, a regular expression and a replacement; the line is ignored";
/// A `header-replace-regex` expression that does not compile.
const HEADER_REGEX: &str =
    "[Header Rewrite]: the regular expression cannot be used ({reason}); the line is ignored";
/// `W0041`: the line is kept, and the request whose framing it changes fails.
const FRAMING: &str = "[Header Rewrite]: a rule on Content-Length or Transfer-Encoding fails every request whose framing it changes; the line is kept";
/// `W0041`'s hint: why such a rule is kept but does what it does.
const FRAMING_HINT: &str = "rurge lets no rewrite change how a message is framed: remove the line unless failing those requests is what it is for";
/// A `[Body Rewrite]` line with no direction, fewer than a URL expression and
/// a pair, or an odd number of items after the expression (phase 4 M3 design
/// 9.1).
const BODY_SHAPE: &str = "[Body Rewrite]: a line that is not `http-request | http-response <regex> <pattern> <replacement> [<pattern> <replacement> …]` is ignored";
/// A `[Body Rewrite]` URL expression that does not compile; `{reason}` is why.
const BODY_REGEX: &str =
    "[Body Rewrite]: the regular expression cannot be used ({reason}); the line is ignored";
/// A body pattern that neither engine compiles; `{reason}` is why.
const BODY_PATTERN: &str =
    "[Body Rewrite]: a body pattern cannot be used ({reason}); the line is ignored";
/// `W0042`: a `[Body Rewrite]` line in the jq form (phase 4 M3 design 13).
const JQ_UNAVAILABLE: &str =
    "[Body Rewrite]: jq body rewrite is not available in this build of rurge; the line is ignored";

/// What a `[Header Rewrite]` line does to the headers whose URL matches.
#[derive(Clone, PartialEq, Eq)]
pub enum HeaderAction {
    Add {
        name: String,
        value: String,
    },
    Del {
        name: String,
    },
    Replace {
        name: String,
        value: String,
    },
    ReplaceRegex {
        name: String,
        regex: Pattern,
        template: Template,
    },
}

impl HeaderAction {
    /// The header field name, as the line wrote it.
    pub fn name(&self) -> &str {
        match self {
            HeaderAction::Add { name, .. }
            | HeaderAction::Del { name }
            | HeaderAction::Replace { name, .. }
            | HeaderAction::ReplaceRegex { name, .. } => name,
        }
    }

    /// The action's name as the line writes it.
    pub fn verb(&self) -> &'static str {
        match self {
            HeaderAction::Add { .. } => "header-add",
            HeaderAction::Del { .. } => "header-del",
            HeaderAction::Replace { .. } => "header-replace",
            HeaderAction::ReplaceRegex { .. } => "header-replace-regex",
        }
    }
}

/// The value of an action holds header values: nothing of it is shown.
impl std::fmt::Debug for HeaderAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeaderAction::Add { name, .. } => write!(f, "Add({name}, ***)"),
            HeaderAction::Del { name } => write!(f, "Del({name})"),
            HeaderAction::Replace { name, .. } => write!(f, "Replace({name}, ***)"),
            HeaderAction::ReplaceRegex { name, .. } => write!(f, "ReplaceRegex({name}, ***)"),
        }
    }
}

/// Reads the section's active lines in order.
pub fn parse_url<'a>(
    entries: impl IntoIterator<Item = &'a Entry>,
    diags: &mut Diagnostics,
) -> Vec<UrlRewrite> {
    let mut rules = Vec::new();
    for entry in entries {
        let words = split_words(&entry.raw);
        let mode = if (2..=3).contains(&words.len()) {
            words.get(2).map_or(Some(UrlMode::Header), |word| {
                match word.text.to_ascii_lowercase().as_str() {
                    "header" => Some(UrlMode::Header),
                    "302" => Some(UrlMode::Found),
                    "307" => Some(UrlMode::Temporary),
                    "reject" => Some(UrlMode::Reject),
                    _ => None,
                }
            })
        } else {
            None
        };
        let Some(mode) = mode else {
            diags.push(
                Diagnostic::warning(codes::W_INVALID_VALUE, URL_SHAPE).at(entry.span.clone()),
            );
            continue;
        };
        let pattern = match Pattern::new(&words[0].text) {
            Ok(pattern) => pattern,
            Err(reason) => {
                diags.push(
                    Diagnostic::warning(
                        codes::W_INVALID_VALUE,
                        URL_REGEX.replace("{reason}", &reason),
                    )
                    .at(entry.span.clone()),
                );
                continue;
            }
        };
        rules.push(UrlRewrite {
            pattern,
            replacement: Template::parse(&words[1].text),
            mode,
            gateway: words[1].text.contains(GATEWAY_ADDRESS),
            span: entry.span.clone(),
        });
    }
    rules
}

/// Reads the section's active lines in order.
pub fn parse_header<'a>(
    entries: impl IntoIterator<Item = &'a Entry>,
    diags: &mut Diagnostics,
) -> Vec<HeaderRewrite> {
    let mut rules = Vec::new();
    for entry in entries {
        let words = split_words(&entry.raw);
        let first = words.first().map(|w| w.text.to_ascii_lowercase());
        let at = match first.as_deref() {
            Some("http-request") | Some("http-response") => 1,
            _ => 0,
        };
        let direction = match first.as_deref() {
            Some("http-response") => Direction::Response,
            _ => Direction::Request,
        };
        if words.len() < at + 3 {
            diags.push(
                Diagnostic::warning(codes::W_INVALID_VALUE, HEADER_SHAPE).at(entry.span.clone()),
            );
            continue;
        }
        let verb = words[at + 1].text.to_ascii_lowercase();
        let known = matches!(
            verb.as_str(),
            "header-add" | "header-del" | "header-replace" | "header-replace-regex"
        );
        if !known {
            diags.push(
                Diagnostic::warning(codes::W_INVALID_VALUE, HEADER_SHAPE).at(entry.span.clone()),
            );
            continue;
        }
        let pattern = match Pattern::new(&words[at].text) {
            Ok(pattern) => pattern,
            Err(reason) => {
                diags.push(
                    Diagnostic::warning(
                        codes::W_INVALID_VALUE,
                        HEADER_REGEX.replace("{reason}", &reason),
                    )
                    .at(entry.span.clone()),
                );
                continue;
            }
        };
        let name = &words[at + 2];
        if !is_field_name(&name.text) {
            diags.push(
                Diagnostic::warning(
                    codes::W_INVALID_VALUE,
                    HEADER_NAME.replace("{name}", &name.text),
                )
                .at(entry.span.clone()),
            );
            continue;
        }
        let action = match verb.as_str() {
            "header-del" => {
                if words.len() != at + 3 {
                    diags.push(
                        Diagnostic::warning(codes::W_INVALID_VALUE, HEADER_DEL_EXTRA)
                            .at(entry.span.clone()),
                    );
                    continue;
                }
                HeaderAction::Del {
                    name: name.text.clone(),
                }
            }
            "header-replace-regex" => {
                if words.len() != at + 5 {
                    diags.push(
                        Diagnostic::warning(codes::W_INVALID_VALUE, HEADER_REGEX_SHAPE)
                            .at(entry.span.clone()),
                    );
                    continue;
                }
                let regex = match Pattern::new(&words[at + 3].text) {
                    Ok(regex) => regex,
                    Err(reason) => {
                        diags.push(
                            Diagnostic::warning(
                                codes::W_INVALID_VALUE,
                                HEADER_REGEX.replace("{reason}", &reason),
                            )
                            .at(entry.span.clone()),
                        );
                        continue;
                    }
                };
                HeaderAction::ReplaceRegex {
                    name: name.text.clone(),
                    regex,
                    template: Template::parse(&words[at + 4].text),
                }
            }
            _ => {
                if words.len() == at + 3 {
                    diags.push(
                        Diagnostic::warning(codes::W_INVALID_VALUE, HEADER_VALUE)
                            .at(entry.span.clone()),
                    );
                    continue;
                }
                let value = if words.len() == at + 4 && words[at + 3].quoted {
                    words[at + 3].text.clone()
                } else {
                    entry.raw[name.end..]
                        .trim_matches(|c| c == ' ' || c == '\t')
                        .to_string()
                };
                if !value.chars().all(can_be_a_value) {
                    diags.push(
                        Diagnostic::warning(codes::W_INVALID_VALUE, HEADER_CONTROL)
                            .at(entry.span.clone()),
                    );
                    continue;
                }
                if verb == "header-add" {
                    HeaderAction::Add {
                        name: name.text.clone(),
                        value,
                    }
                } else {
                    HeaderAction::Replace {
                        name: name.text.clone(),
                        value,
                    }
                }
            }
        };
        if name.text.eq_ignore_ascii_case("content-length")
            || name.text.eq_ignore_ascii_case("transfer-encoding")
        {
            diags.push(
                Diagnostic::warning(codes::W_REWRITE_FRAMING, FRAMING)
                    .at(entry.span.clone())
                    .with_hint(FRAMING_HINT),
            );
        }
        rules.push(HeaderRewrite {
            direction,
            pattern,
            action,
            span: entry.span.clone(),
        });
    }
    rules
}

/// Reads the section's active lines in order.
pub fn parse_body<'a>(
    entries: impl IntoIterator<Item = &'a Entry>,
    diags: &mut Diagnostics,
) -> Vec<BodyRewrite> {
    let mut rules = Vec::new();
    for entry in entries {
        let words = split_words(&entry.raw);
        let first = words.first().map(|word| word.text.as_str());
        let direction = if first.is_some_and(|word| word.eq_ignore_ascii_case("http-request")) {
            Direction::Request
        } else if first.is_some_and(|word| word.eq_ignore_ascii_case("http-response")) {
            Direction::Response
        } else if first.is_some_and(|word| word.eq_ignore_ascii_case("http-request-jq"))
            || first.is_some_and(|word| word.eq_ignore_ascii_case("http-response-jq"))
        {
            diags.push(
                Diagnostic::warning(codes::W_JQ_REWRITE_UNAVAILABLE, JQ_UNAVAILABLE)
                    .at(entry.span.clone()),
            );
            continue;
        } else {
            diags.push(
                Diagnostic::warning(codes::W_INVALID_VALUE, BODY_SHAPE).at(entry.span.clone()),
            );
            continue;
        };
        if words.len() < 4 || (words.len() - 2) % 2 != 0 {
            diags.push(
                Diagnostic::warning(codes::W_INVALID_VALUE, BODY_SHAPE).at(entry.span.clone()),
            );
            continue;
        }
        let pattern = match Pattern::new(&words[1].text) {
            Ok(pattern) => pattern,
            Err(reason) => {
                diags.push(
                    Diagnostic::warning(
                        codes::W_INVALID_VALUE,
                        BODY_REGEX.replace("{reason}", &reason),
                    )
                    .at(entry.span.clone()),
                );
                continue;
            }
        };
        let mut pairs = Vec::new();
        let mut ok = true;
        for pair in words[2..].chunks(2) {
            match BodyPattern::new(&pair[0].text) {
                Ok(body) => pairs.push((body, Template::parse(&pair[1].text))),
                Err(reason) => {
                    diags.push(
                        Diagnostic::warning(
                            codes::W_INVALID_VALUE,
                            BODY_PATTERN.replace("{reason}", &reason),
                        )
                        .at(entry.span.clone()),
                    );
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        rules.push(BodyRewrite {
            direction,
            pattern,
            pairs,
            span: entry.span.clone(),
        });
    }
    rules
}

/// RFC 9110's token: ASCII letters, digits and the fifteen punctuation
/// characters the field-name grammar lists (`!`, `#`, `$`, `%`, `&`, `'`, `*`,
/// `+`, `-`, `.`, `^`, `_`, the backtick, `|`, `~`).
fn is_field_name(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    '!' | '#'
                        | '$'
                        | '%'
                        | '&'
                        | '\''
                        | '*'
                        | '+'
                        | '-'
                        | '.'
                        | '^'
                        | '_'
                        | '`'
                        | '|'
                        | '~'
                )
        })
}

/// What a header value may hold: a tab, or a character at least as high as a
/// space and not DEL (RFC 9110 field-vchar plus obs-text).
fn can_be_a_value(c: char) -> bool {
    c == '\t' || (c >= ' ' && c != '\x7f')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, Severity};
    use crate::text::{Origin, Section, parse_str};
    use std::path::Path;
    use std::sync::Arc;

    /// The `[URL Rewrite]` rules of `lines`, as the loader reads them, and
    /// what it said about them (the text layer's own diagnostics included).
    fn read_url(lines: &str) -> (Vec<UrlRewrite>, Vec<Diagnostic>) {
        let text = format!("[URL Rewrite]\n{lines}\n");
        let (profile, mut diags) = parse_str(&text, Arc::from(Path::new("r.conf")), Origin::Main);
        let found = parse_url(
            profile
                .section("URL Rewrite")
                .into_iter()
                .flat_map(Section::active_entries),
            &mut diags,
        );
        (found, diags.into_vec())
    }

    /// The `[Header Rewrite]` rules of `lines`, read the same way.
    fn read_header(lines: &str) -> (Vec<HeaderRewrite>, Vec<Diagnostic>) {
        let text = format!("[Header Rewrite]\n{lines}\n");
        let (profile, mut diags) = parse_str(&text, Arc::from(Path::new("h.conf")), Origin::Main);
        let found = parse_header(
            profile
                .section("Header Rewrite")
                .into_iter()
                .flat_map(Section::active_entries),
            &mut diags,
        );
        (found, diags.into_vec())
    }

    fn said(diags: &[Diagnostic]) -> Vec<(&'static str, &str)> {
        diags.iter().map(|d| (d.code, d.message.as_str())).collect()
    }

    /// The `[Body Rewrite]` rules of `lines`, read the same way.
    fn read_body(lines: &str) -> (Vec<BodyRewrite>, Vec<Diagnostic>) {
        let text = format!("[Body Rewrite]\n{lines}\n");
        let (profile, mut diags) = parse_str(&text, Arc::from(Path::new("b.conf")), Origin::Main);
        let found = parse_body(
            profile
                .section("Body Rewrite")
                .into_iter()
                .flat_map(Section::active_entries),
            &mut diags,
        );
        (found, diags.into_vec())
    }

    /// `template` expanded with no groups.
    fn expanded(template: &Template) -> String {
        let mut out = String::new();
        template.expand(&|_| None, 0, &mut out);
        out
    }

    /// The lines the manual's URL Rewrite page writes.
    #[test]
    fn url_rewrite_reads_the_manual_example() {
        let text = "\
^http://www\\.google\\.cn http://www.google.com header
^http://yachen\\.com https://yach.me 302
^http://ad\\.com/ad\\.png _ reject
";
        let (rules, diags) = read_url(text);
        assert!(diags.is_empty(), "{:?}", said(&diags));
        assert_eq!(rules.len(), 3);
        let modes: Vec<UrlMode> = rules.iter().map(|r| r.mode).collect();
        assert_eq!(modes, [UrlMode::Header, UrlMode::Found, UrlMode::Reject]);
        let sources: Vec<&str> = rules.iter().map(|r| r.pattern.source.as_str()).collect();
        assert_eq!(
            sources,
            [
                r"^http://www\.google\.cn",
                r"^http://yachen\.com",
                r"^http://ad\.com/ad\.png",
            ]
        );
    }

    #[test]
    fn url_rewrite_modes_and_shapes() {
        let modes: &[(&str, UrlMode)] = &[
            ("^a b 307", UrlMode::Temporary),
            ("^a b HEADER", UrlMode::Header),
            ("^a b", UrlMode::Header),
        ];
        for (line, mode) in modes {
            let (rules, diags) = read_url(line);
            assert_eq!(rules.len(), 1, "{line}");
            assert_eq!(rules[0].mode, *mode, "{line}");
            assert!(diags.is_empty(), "{line}: {:?}", said(&diags));
        }

        let shape = "[URL Rewrite]: a line that is not `<regex> <replacement> [header | 302 | 307 | reject]` is ignored";
        for line in ["^a", "^a b c d", "^a b 301"] {
            let (rules, diags) = read_url(line);
            assert!(rules.is_empty(), "{line}");
            assert_eq!(said(&diags), [("W0012", shape)], "{line}");
        }

        let (rules, diags) = read_url("( b");
        assert!(rules.is_empty());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "W0012");
        assert!(
            diags[0]
                .message
                .starts_with("[URL Rewrite]: the regular expression cannot be used ("),
            "{}",
            diags[0].message
        );

        let (rules, diags) = read_url("^a \"x y\" 302");
        assert!(diags.is_empty(), "{:?}", said(&diags));
        let mut out = String::new();
        rules[0].replacement.expand(&|_| None, 0, &mut out);
        assert_eq!(out, "x y");

        let (rules, _) = read_url("^a http://{{{GATEWAY_ADDRESS}}}/ 302");
        assert!(rules[0].gateway);
        let (rules, _) = read_url("^a http://b/ 302");
        assert!(!rules[0].gateway);
    }

    /// The lines the manual's Header Rewrite page writes.
    #[test]
    fn header_rewrite_reads_the_manual_examples() {
        let text = "\
http-request ^http://example.com header-add DNT 1
http-request ^http://example.com header-del Cookie
http-request ^http://example.com header-replace User-Agent Unknown
http-response ^http://example.com header-replace-regex Date 2022 2023
^http://example.com header-add DNT 1
";
        let (rules, diags) = read_header(text);
        assert!(diags.is_empty(), "{:?}", said(&diags));
        assert_eq!(rules.len(), 5);
        assert_eq!(rules[0].direction, Direction::Request);
        assert_eq!(rules[0].pattern.source, "^http://example.com");
        assert_eq!(
            rules[0].action,
            HeaderAction::Add {
                name: "DNT".into(),
                value: "1".into(),
            }
        );
        assert_eq!(rules[1].direction, Direction::Request);
        assert_eq!(
            rules[1].action,
            HeaderAction::Del {
                name: "Cookie".into(),
            }
        );
        assert_eq!(rules[2].direction, Direction::Request);
        assert_eq!(
            rules[2].action,
            HeaderAction::Replace {
                name: "User-Agent".into(),
                value: "Unknown".into(),
            }
        );
        assert_eq!(rules[3].direction, Direction::Response);
        match &rules[3].action {
            HeaderAction::ReplaceRegex {
                name,
                regex,
                template,
            } => {
                assert_eq!(name, "Date");
                assert_eq!(regex.source, "2022");
                let mut out = String::new();
                template.expand(&|_| None, 0, &mut out);
                assert_eq!(out, "2023");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(rules[4].direction, Direction::Request);
        assert_eq!(
            rules[4].action,
            HeaderAction::Add {
                name: "DNT".into(),
                value: "1".into(),
            }
        );
    }

    #[test]
    fn a_header_value_is_the_rest_of_the_line() {
        let (rules, diags) = read_header("^a header-add User-Agent Mozilla/5.0 (X; Y) Z");
        assert!(diags.is_empty(), "{:?}", said(&diags));
        assert_eq!(
            rules[0].action,
            HeaderAction::Add {
                name: "User-Agent".into(),
                value: "Mozilla/5.0 (X; Y) Z".into(),
            }
        );

        let (rules, _) = read_header("^a header-add X \"a b\"");
        assert_eq!(
            rules[0].action,
            HeaderAction::Add {
                name: "X".into(),
                value: "a b".into(),
            }
        );
        let (rules, _) = read_header("^a header-add X \"\"");
        assert_eq!(
            rules[0].action,
            HeaderAction::Add {
                name: "X".into(),
                value: String::new(),
            }
        );
        let (rules, _) = read_header("^a header-add X \"a\" \"b\"");
        assert_eq!(
            rules[0].action,
            HeaderAction::Add {
                name: "X".into(),
                value: "\"a\" \"b\"".into(),
            }
        );

        let missing = "[Header Rewrite]: header-add and header-replace take a value after the field name (write \"\" for an empty one); the line is ignored";
        for line in ["^a header-add X", "^a header-replace X"] {
            let (rules, diags) = read_header(line);
            assert!(rules.is_empty(), "{line}");
            assert_eq!(said(&diags), [("W0012", missing)], "{line}");
        }
    }

    #[test]
    fn header_rewrite_refuses() {
        let shape = "[Header Rewrite]: a line that is not `[http-request | http-response] <regex> <action> <name> …` with an action of header-add, header-del, header-replace or header-replace-regex is ignored";
        for line in ["^a header-set X 1", "^a", "^a header-add"] {
            let (rules, diags) = read_header(line);
            assert!(rules.is_empty(), "{line}");
            assert_eq!(said(&diags), [("W0012", shape)], "{line}");
        }

        let not_a_name = |name: &str| {
            format!("[Header Rewrite]: `{name}` is not a header field name; the line is ignored")
        };
        for (line, name) in [
            ("^a header-add X:Y 1", "X:Y"),
            ("^a header-add :authority 1", ":authority"),
        ] {
            let (rules, diags) = read_header(line);
            assert!(rules.is_empty(), "{line}");
            assert_eq!(
                said(&diags),
                [("W0012", not_a_name(name).as_str())],
                "{line}"
            );
        }

        let del = "[Header Rewrite]: header-del takes a field name and nothing after it; the line is ignored";
        let (rules, diags) = read_header("^a header-del X extra");
        assert!(rules.is_empty());
        assert_eq!(said(&diags), [("W0012", del)]);

        let replace_regex = "[Header Rewrite]: header-replace-regex takes a field name, a regular expression and a replacement; the line is ignored";
        for line in [
            "^a header-replace-regex X a",
            "^a header-replace-regex X a b c",
        ] {
            let (rules, diags) = read_header(line);
            assert!(rules.is_empty(), "{line}");
            assert_eq!(said(&diags), [("W0012", replace_regex)], "{line}");
        }

        let (rules, diags) = read_header("^a header-replace-regex X ( b");
        assert!(rules.is_empty());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "W0012");
        assert!(
            diags[0]
                .message
                .starts_with("[Header Rewrite]: the regular expression cannot be used ("),
            "{}",
            diags[0].message
        );

        let control = "[Header Rewrite]: the value has a control character a header cannot carry; the line is ignored";
        let (rules, diags) = read_header("^a header-add X a\u{1}b-s3cr3t");
        assert!(rules.is_empty());
        assert_eq!(said(&diags), [("W0012", control)]);
        assert!(!diags[0].message.contains("s3cr3t"));
    }

    /// A line that changes how the message is framed is kept, with `W0041`.
    #[test]
    fn framing_headers_are_kept_with_w0041() {
        let text = "^a header-del Transfer-Encoding\n^a header-replace content-length 5";
        let (rules, diags) = read_header(text);
        assert_eq!(rules.len(), 2);
        assert_eq!(
            rules[0].action,
            HeaderAction::Del {
                name: "Transfer-Encoding".into(),
            }
        );
        assert_eq!(
            rules[1].action,
            HeaderAction::Replace {
                name: "content-length".into(),
                value: "5".into(),
            }
        );
        assert_eq!(diags.len(), 2);
        for d in &diags {
            assert_eq!(d.code, "W0041");
            assert_eq!(d.severity, Severity::Warning);
            assert_eq!(
                d.message,
                "[Header Rewrite]: a rule on Content-Length or Transfer-Encoding fails every request whose framing it changes; the line is kept"
            );
            assert_eq!(
                d.hint,
                Some(
                    "rurge lets no rewrite change how a message is framed: remove the line unless failing those requests is what it is for"
                )
            );
        }
    }

    #[test]
    fn the_debug_of_header_rewrites_shows_no_value() {
        let text = "^a header-add X-Token s3cr3t-v\n^a header-replace-regex X-Token a t3mpl";
        let (rules, diags) = read_header(text);
        assert!(diags.is_empty(), "{:?}", said(&diags));
        let shown = format!("{rules:?}");
        assert!(!shown.contains("s3cr3t-v"), "{shown}");
        assert!(!shown.contains("t3mpl"), "{shown}");
        assert!(shown.contains("X-Token"), "{shown}");
    }

    /// The lines the manual's Body Rewrite page writes (phase 4 M3 design
    /// 9.1).
    #[test]
    fn body_rewrite_reads_the_manual_examples() {
        let text = r"http-request ^http(s)?://example\.com value abc
http-response ^http(s)?://example\.com documents Surge
http-response ^https?://example\.com/ regex1 replacement1 regex2 replacement2
";
        let (rules, diags) = read_body(text);
        assert!(diags.is_empty(), "{:?}", said(&diags));
        assert_eq!(rules.len(), 3);
        let directions: Vec<Direction> = rules.iter().map(|r| r.direction).collect();
        assert_eq!(
            directions,
            [Direction::Request, Direction::Response, Direction::Response]
        );
        let sources: Vec<&str> = rules.iter().map(|r| r.pattern.source.as_str()).collect();
        assert_eq!(
            sources,
            [
                r"^http(s)?://example\.com",
                r"^http(s)?://example\.com",
                r"^https?://example\.com/",
            ]
        );
        let first: Vec<(&str, String)> = rules[0]
            .pairs
            .iter()
            .map(|(pattern, template)| (pattern.source.as_str(), expanded(template)))
            .collect();
        assert_eq!(first, [("value", "abc".to_string())]);
        let second: Vec<(&str, String)> = rules[1]
            .pairs
            .iter()
            .map(|(pattern, template)| (pattern.source.as_str(), expanded(template)))
            .collect();
        assert_eq!(second, [("documents", "Surge".to_string())]);
        let third: Vec<(&str, String)> = rules[2]
            .pairs
            .iter()
            .map(|(pattern, template)| (pattern.source.as_str(), expanded(template)))
            .collect();
        assert_eq!(
            third,
            [
                ("regex1", "replacement1".to_string()),
                ("regex2", "replacement2".to_string()),
            ]
        );
    }

    /// A body rewrite line is a direction, a URL expression and one or more
    /// pattern and replacement pairs (phase 4 M3 design 9.1).
    #[test]
    fn body_rewrite_shapes() {
        let (rules, diags) = read_body("HTTP-REQUEST ^a b c");
        assert!(diags.is_empty(), "{:?}", said(&diags));
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].direction, Direction::Request);
        assert_eq!(rules[0].pattern.source, "^a");
        assert_eq!(rules[0].pairs.len(), 1);
        assert_eq!(rules[0].pairs[0].0.source, "b");
        assert_eq!(expanded(&rules[0].pairs[0].1), "c");

        let shape = "[Body Rewrite]: a line that is not `http-request | http-response <regex> <pattern> <replacement> [<pattern> <replacement> …]` is ignored";
        for line in [
            "http-response ^a b",
            "http-response ^a",
            "http-put ^a b c",
            "http-response ^a b c d",
        ] {
            let (rules, diags) = read_body(line);
            assert!(rules.is_empty(), "{line}");
            assert_eq!(said(&diags), [("W0012", shape)], "{line}");
        }

        let (rules, diags) = read_body("http-response ^a b c d e");
        assert!(diags.is_empty(), "{:?}", said(&diags));
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pairs.len(), 2);
        assert_eq!(rules[0].pairs[1].0.source, "d");
        assert_eq!(expanded(&rules[0].pairs[1].1), "e");

        let (rules, diags) = read_body("http-request ( b c");
        assert!(rules.is_empty());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "W0012");
        assert!(
            diags[0]
                .message
                .starts_with("[Body Rewrite]: the regular expression cannot be used ("),
            "{}",
            diags[0].message
        );

        let (rules, diags) = read_body("http-request ^a ( c");
        assert!(rules.is_empty());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "W0012");
        assert!(
            diags[0]
                .message
                .starts_with("[Body Rewrite]: a body pattern cannot be used ("),
            "{}",
            diags[0].message
        );

        let (rules, diags) = read_body("http-request ^a \"x y\" \"1 2\"");
        assert!(diags.is_empty(), "{:?}", said(&diags));
        assert_eq!(rules[0].pairs.len(), 1);
        assert_eq!(rules[0].pairs[0].0.source, "x y");
        assert_eq!(expanded(&rules[0].pairs[0].1), "1 2");
    }

    /// A `[Body Rewrite]` line in the jq form is ignored with `W0042`,
    /// whatever it writes after the direction (phase 4 M3 design 13).
    #[test]
    fn jq_lines_are_ignored_with_w0042() {
        let text = r#"http-response-jq ^http://httpbingo.org/anything '.headers |= with_entries(select(.key | test("^X-") | not))'
HTTP-REQUEST-JQ ^a .
"#;
        let (rules, diags) = read_body(text);
        assert!(rules.is_empty());
        assert_eq!(diags.len(), 2, "{:?}", said(&diags));
        for (index, d) in diags.iter().enumerate() {
            assert_eq!(d.code, "W0042");
            assert_eq!(d.severity, Severity::Warning);
            assert_eq!(
                d.message,
                "[Body Rewrite]: jq body rewrite is not available in this build of rurge; the line is ignored"
            );
            assert_eq!(
                d.span.as_ref().map(|span| span.line),
                Some(index as u32 + 2)
            );
        }
    }
}
