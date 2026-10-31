//! `[URL Rewrite]`, `[Header Rewrite]`, `[Body Rewrite]` and `[Map Local]`
//! (manual: Features › URL Rewrite, Features › Header Rewrite, Features ›
//! Body Rewrite and Features › Map Local): the lines as written, read once at
//! load time.
//!
//! A `[URL Rewrite]` line is `<regex> <replacement> [header | 302 | 307 |
//! reject]`; a `[Header Rewrite]` line is `[http-request | http-response]
//! <regex> <action> <name> …` with an action of `header-add`, `header-del`,
//! `header-replace` or `header-replace-regex`; a `[Body Rewrite]` line is
//! `[http-request | http-response] <regex> <body pattern> <replacement>
//! [<body pattern> <replacement> …]`, each pair applied in turn. A jq line
//! (`http-request-jq` / `http-response-jq`) gives `W0042` and is not read. A
//! `[Map Local]` line is `<regex> [data-type=…] [data=…] [header=…]
//! [status-code=…]`, answered here instead of by an upstream server; a path it
//! names in text rurge downloaded is refused with `W0033`. The four sections
//! are read here the way the manual describes them (phase 4 M3 design 6.1,
//! 7.1, 9.1 and 10.1, the phase 4 / M3a plan's P6): the engine only evaluates
//! the rules this module keeps, and each rule keeps the span of its line so
//! that the engine can name it in the request's notes.
//!
//! A `[URL Rewrite]` replacement may hold `{{{GATEWAY_ADDRESS}}}`. Whether it
//! does is recorded here, at load time; the engine puts the gateway in only
//! for the `302` and `307` modes, where the manual allows the placeholder
//! (phase 4 M3 design 6.4).

use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::dialect::{BodyPattern, Template};
use crate::rule::Pattern;
use crate::span::Span;
use crate::spec::reader::is_plain_name;
use crate::text::Entry;
use crate::value::{split_words, starts_with_ci};
use std::path::{Path, PathBuf};

/// `[URL Rewrite]`, `[Header Rewrite]`, `[Body Rewrite]` and `[Map Local]`:
/// the rules as written, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RewriteConfig {
    pub url: Vec<UrlRewrite>,
    pub header: Vec<HeaderRewrite>,
    /// The lines of `[Body Rewrite]` that can be read, in the order written
    /// (phase 4 M3 design 9.1); a jq line gives `W0042` and is not one of
    /// them.
    pub body: Vec<BodyRewrite>,
    /// The lines of `[Map Local]` that can be read, in the order written
    /// (phase 4 M3 design 10.1); a path written relative joins the directory
    /// of the main profile.
    pub map_local: Vec<MapLocal>,
}

impl RewriteConfig {
    /// Whether no section has a usable line.
    pub fn is_empty(&self) -> bool {
        self.url.is_empty()
            && self.header.is_empty()
            && self.body.is_empty()
            && self.map_local.is_empty()
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

/// What a `[Map Local]` line answers with (manual: Features › Map Local,
/// phase 4 M3 design 10.1 and 10.3). `data-type` picks it, and a `file` line
/// names either a local file or a URL whose data rurge downloads.
#[derive(Clone, PartialEq, Eq)]
pub enum MapData {
    File(PathBuf),
    Url(url::Url),
    Text(String),
    Base64(Vec<u8>),
    TinyGif,
}

impl MapData {
    /// The `data-type` a note names it by: `File` and `Url` are both `file`,
    /// the two forms the manual gives that one name (its `file` type returns
    /// "the content of a specific file or URL").
    pub fn kind(&self) -> &'static str {
        match self {
            MapData::File(_) | MapData::Url(_) => "file",
            MapData::Text(_) => "text",
            MapData::Base64(_) => "base64",
            MapData::TinyGif => "tiny-gif",
        }
    }
}

/// The data as a `Debug` shows it: the bytes and the text a line answers with
/// are the user's traffic, so only a file's path, an origin and the kind are
/// shown (phase 4 M3 design 8.5 and 14).
impl std::fmt::Debug for MapData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MapData::File(path) => write!(f, "File({path:?})"),
            MapData::Url(url) => write!(f, "Url({})", crate::remote::origin(url)),
            MapData::Text(_) => f.write_str("Text(***)"),
            MapData::Base64(_) => f.write_str("Base64(***)"),
            MapData::TinyGif => f.write_str("TinyGif"),
        }
    }
}

/// One `[Map Local]` line (phase 4 M3 design 10.1): the request whose URL
/// matches is answered here, without an upstream server.
#[derive(Clone, PartialEq, Eq)]
pub struct MapLocal {
    /// The expression for the URL the line answers.
    pub pattern: Pattern,
    pub data: MapData,
    pub status: u16,
    /// The headers of the answer, in the order written.
    pub headers: Vec<(String, String)>,
    pub span: Span,
}

/// The answer as a `Debug` shows it: the names of the headers, never their
/// values (phase 4 M3 design 8.5 and 14).
impl std::fmt::Debug for MapLocal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let MapLocal {
            pattern,
            data,
            status,
            headers,
            span,
        } = self;
        let names: Vec<&str> = headers.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("MapLocal")
            .field("pattern", pattern)
            .field("data", data)
            .field("status", status)
            .field("headers", &names)
            .field("span", span)
            .finish()
    }
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
/// A `[Map Local]` expression that does not compile; `{reason}` is why.
const MAP_REGEX: &str =
    "[Map Local]: the regular expression cannot be used ({reason}); the line is ignored";
/// A `[Map Local]` item after the expression that is not a parameter.
const MAP_PARAMETER: &str = "[Map Local]: every item after the regular expression is a key=value parameter; the line is ignored";
/// A `data-type` other than the four the manual names.
const MAP_TYPE: &str =
    "[Map Local]: data-type is file, text, tiny-gif or base64; the line is ignored";
/// A `file` line without a data, or with an empty one.
const MAP_FILE: &str =
    "[Map Local]: data-type=file takes a path or a URL in data; the line is ignored";
/// `file` data that is not a URL rurge can download.
const MAP_URL: &str = "[Map Local]: data is not a URL rurge can download; the line is ignored";
/// `base64` data that is not base64.
const MAP_BASE64: &str = "[Map Local]: data is not valid base64; the line is ignored";
/// A `header` value that is neither pairs of a valid name and value nor the
/// base64 of lines that are. The value is not shown.
const MAP_HEADER: &str = "[Map Local]: header is `name:value|name:value` or base64 of header lines, with valid names and values; the line is ignored";
/// A `status-code` that is not a whole number from 200 to 999.
const MAP_STATUS: &str =
    "[Map Local]: status-code is a whole number from 200 to 999; the line is ignored";
/// `W0041`: the two names are dropped and the rest of the line is kept.
const MAP_FRAMING: &str = "[Map Local]: Content-Length and Transfer-Encoding in header are not used; the rest of the line is kept";
/// `W0041`'s hint where a `[Map Local]` `header` names the two.
const MAP_FRAMING_HINT: &str =
    "rurge frames the answer by the length of its data: remove the two names from header";
/// `W0001` where the key is written the way a parameter name is.
const MAP_UNKNOWN_KEY: &str = "[Map Local]: unknown parameter `{key}` ignored";
/// `W0001` where nothing of the line may be repeated.
const MAP_UNKNOWN_KEY_UNNAMED: &str = "[Map Local]: unknown parameter ignored";

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

/// Reads the section's active lines in order. `base_dir` is the directory of
/// the main profile: a path written relative joins it, whichever file the line
/// itself stands in (phase 4 M3 design 10.1).
pub fn parse_map_local<'a>(
    entries: impl IntoIterator<Item = &'a Entry>,
    base_dir: &Path,
    diags: &mut Diagnostics,
) -> Vec<MapLocal> {
    entries
        .into_iter()
        .filter_map(|entry| read_map_local(entry, base_dir, diags))
        .collect()
}

/// One `[Map Local]` line, or nothing when it cannot be read; whatever is not
/// kept is said with `W0012`, `W0001`, `W0033` or `W0041` (phase 4 M3 design
/// 10.1, the phase 4 / M3b plan's P5).
fn read_map_local(entry: &Entry, base_dir: &Path, diags: &mut Diagnostics) -> Option<MapLocal> {
    let words = split_words(&entry.raw);
    // the first item is the expression for the URL, whether or not it looks
    // like a parameter: a query holds `=` often enough
    let first = words.first()?;
    let pattern = match Pattern::new(&first.text) {
        Ok(pattern) => pattern,
        Err(reason) => {
            diags.push(
                Diagnostic::warning(
                    codes::W_INVALID_VALUE,
                    MAP_REGEX.replace("{reason}", &reason),
                )
                .at(entry.span.clone()),
            );
            return None;
        }
    };
    // each of the four parameters, the later of two written values winning
    let (mut kind, mut data, mut header, mut status) = (None, None, None, None);
    for word in &words[1..] {
        let Some((key, value)) = &word.param else {
            diags.push(
                Diagnostic::warning(codes::W_INVALID_VALUE, MAP_PARAMETER).at(entry.span.clone()),
            );
            return None;
        };
        if key.eq_ignore_ascii_case("data-type") {
            kind = Some(value.clone());
        } else if key.eq_ignore_ascii_case("data") {
            data = Some(value.clone());
        } else if key.eq_ignore_ascii_case("header") {
            header = Some(value.clone());
        } else if key.eq_ignore_ascii_case("status-code") {
            status = Some(value.clone());
        } else {
            // a key written the way a parameter name is can be named; nothing
            // else of the line is, so the message leaves it out
            let message = if is_plain_name(key) {
                MAP_UNKNOWN_KEY.replace("{key}", key)
            } else {
                MAP_UNKNOWN_KEY_UNNAMED.to_string()
            };
            diags.push(Diagnostic::warning(codes::W_UNKNOWN_KEY, message).at(entry.span.clone()));
        }
    }

    let kind = kind.unwrap_or_else(|| "file".to_string());
    let data = if kind.eq_ignore_ascii_case("file") {
        read_map_file(
            data.filter(|value| !value.is_empty()),
            entry,
            base_dir,
            diags,
        )?
    } else if kind.eq_ignore_ascii_case("text") {
        MapData::Text(data.unwrap_or_default())
    } else if kind.eq_ignore_ascii_case("base64") {
        // no data at all is an empty answer, not a line to refuse
        match data {
            None => MapData::Base64(Vec::new()),
            Some(value) => match decode_base64(&value) {
                Some(bytes) => MapData::Base64(bytes),
                None => {
                    diags.push(
                        Diagnostic::warning(codes::W_INVALID_VALUE, MAP_BASE64)
                            .at(entry.span.clone()),
                    );
                    return None;
                }
            },
        }
    } else if kind.eq_ignore_ascii_case("tiny-gif") {
        // the manual gives this type's data no meaning, so it is not read
        MapData::TinyGif
    } else {
        diags.push(Diagnostic::warning(codes::W_INVALID_VALUE, MAP_TYPE).at(entry.span.clone()));
        return None;
    };

    let headers = match header {
        None => Vec::new(),
        Some(value) => read_map_headers(&value, entry, diags)?,
    };

    let status = match status {
        None => 200,
        Some(value) => match value.parse::<u16>() {
            Ok(code) if (200..=999).contains(&code) => code,
            _ => {
                diags.push(
                    Diagnostic::warning(codes::W_INVALID_VALUE, MAP_STATUS).at(entry.span.clone()),
                );
                return None;
            }
        },
    };

    Some(MapLocal {
        pattern,
        data,
        status,
        headers,
        span: entry.span.clone(),
    })
}

/// The data of a `file` line: a URL rurge downloads, or a path. A path from
/// downloaded text is refused with `W0033`: such text names nothing on this
/// machine, and the line is dropped so that nothing downstream opens it.
fn read_map_file(
    data: Option<String>,
    entry: &Entry,
    base_dir: &Path,
    diags: &mut Diagnostics,
) -> Option<MapData> {
    let Some(value) = data else {
        diags.push(Diagnostic::warning(codes::W_INVALID_VALUE, MAP_FILE).at(entry.span.clone()));
        return None;
    };
    if starts_with_ci(&value, "http://") || starts_with_ci(&value, "https://") {
        return match url::Url::parse(&value) {
            Ok(url) => Some(MapData::Url(url)),
            Err(_) => {
                diags.push(
                    Diagnostic::warning(codes::W_INVALID_VALUE, MAP_URL).at(entry.span.clone()),
                );
                None
            }
        };
    }
    if let Some(refusal) = entry.origin.reach().map_local_refusal() {
        diags.push(
            Diagnostic::warning(
                codes::W_REMOTE_LOCAL_ACCESS,
                format!("[Map Local]: {}", refusal.message),
            )
            .with_hint(refusal.hint)
            .at(entry.span.clone()),
        );
        return None;
    }
    let path = Path::new(&value);
    Some(MapData::File(if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }))
}

/// The headers of a `[Map Local]` line: `name:value|name:value`, or, when the
/// value holds no `:`, the base64 of newline-separated header lines. A pair
/// with a name or a value a header cannot carry drops the line; the two names
/// that frame a message are dropped from it with one `W0041`.
fn read_map_headers(
    value: &str,
    entry: &Entry,
    diags: &mut Diagnostics,
) -> Option<Vec<(String, String)>> {
    let mut pairs: Vec<String> = Vec::new();
    if value.contains(':') {
        for pair in value.split('|') {
            let pair = pair.trim();
            if !pair.is_empty() {
                pairs.push(pair.to_string());
            }
        }
    } else {
        let Some(text) = decode_base64(value) else {
            diags.push(
                Diagnostic::warning(codes::W_INVALID_VALUE, MAP_HEADER).at(entry.span.clone()),
            );
            return None;
        };
        let Ok(text) = String::from_utf8(text) else {
            diags.push(
                Diagnostic::warning(codes::W_INVALID_VALUE, MAP_HEADER).at(entry.span.clone()),
            );
            return None;
        };
        for line in text.split('\n') {
            let line = line.trim_end_matches('\r').trim();
            if !line.is_empty() {
                pairs.push(line.to_string());
            }
        }
    }
    let mut headers = Vec::new();
    let mut framing = false;
    for pair in &pairs {
        let Some((name, value)) = read_map_header(pair) else {
            diags.push(
                Diagnostic::warning(codes::W_INVALID_VALUE, MAP_HEADER).at(entry.span.clone()),
            );
            return None;
        };
        if name.eq_ignore_ascii_case("content-length")
            || name.eq_ignore_ascii_case("transfer-encoding")
        {
            framing = true;
            continue;
        }
        headers.push((name, value));
    }
    if framing {
        diags.push(
            Diagnostic::warning(codes::W_REWRITE_FRAMING, MAP_FRAMING)
                .with_hint(MAP_FRAMING_HINT)
                .at(entry.span.clone()),
        );
    }
    Some(headers)
}

/// One `name: value` pair, split at its first `:` and trimmed; nothing when
/// either side is not what a header field carries.
fn read_map_header(pair: &str) -> Option<(String, String)> {
    let (name, value) = pair.split_once(':')?;
    let (name, value) = (name.trim(), value.trim());
    if !is_field_name(name) || !value.chars().all(can_be_a_value) {
        return None;
    }
    Some((name.to_string(), value.to_string()))
}

/// The bytes `text` stands for as base64, padded or not: the same two engines
/// a keystore value is decoded with.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
    STANDARD
        .decode(text)
        .or_else(|_| STANDARD_NO_PAD.decode(text))
        .ok()
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
    use base64::Engine as _;
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

    /// The `[Map Local]` rules of `lines`, read the same way. `base_dir` is
    /// the directory of the main profile, which a relative path joins;
    /// nothing read here opens a file.
    fn read_map_local_from(
        lines: &str,
        base_dir: &Path,
        origin: Origin,
    ) -> (Vec<MapLocal>, Vec<Diagnostic>) {
        let text = format!("[Map Local]\n{lines}\n");
        let (profile, mut diags) = parse_str(&text, Arc::from(Path::new("m.conf")), origin);
        let found = parse_map_local(
            profile
                .section("Map Local")
                .into_iter()
                .flat_map(Section::active_entries),
            base_dir,
            &mut diags,
        );
        (found, diags.into_vec())
    }

    /// `read_map_local_from` for the user's own profile, whose directory is a
    /// temporary one.
    fn read_map_local(lines: &str) -> (Vec<MapLocal>, Vec<Diagnostic>) {
        let dir = tempfile::tempdir().unwrap();
        read_map_local_from(lines, dir.path(), Origin::Main)
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

    /// The lines the manual's Map Local page writes (phase 4 M3 design 10.1).
    #[test]
    fn map_local_reads_the_manual_example() {
        let dir = tempfile::tempdir().unwrap();
        let text = r#"^http://surgetest\.com/json data-type=text data="{}" status-code=500
^http://surgetest\.com/gif data-type=tiny-gif status-code=200
^http://surgetest\.com/file data-type=file data="data/map-local.json" header="a:b|foo:bar"
^http://surgetest\.com/base64 data="dGVzdA==" data-type=base64
^http://surgetest\.com/empty data-type=text data=""
"#;
        let (rules, diags) = read_map_local_from(text, dir.path(), Origin::Main);
        assert!(diags.is_empty(), "{:?}", said(&diags));
        assert_eq!(rules.len(), 5);
        let sources: Vec<&str> = rules.iter().map(|r| r.pattern.source.as_str()).collect();
        assert_eq!(
            sources,
            [
                r"^http://surgetest\.com/json",
                r"^http://surgetest\.com/gif",
                r"^http://surgetest\.com/file",
                r"^http://surgetest\.com/base64",
                r"^http://surgetest\.com/empty",
            ]
        );
        assert_eq!(rules[0].data, MapData::Text("{}".to_string()));
        assert_eq!(rules[0].status, 500);
        assert!(rules[0].headers.is_empty());
        assert_eq!(rules[1].data, MapData::TinyGif);
        assert_eq!(rules[1].status, 200);
        assert_eq!(
            rules[2].data,
            MapData::File(dir.path().join("data/map-local.json"))
        );
        assert_eq!(rules[2].status, 200);
        assert_eq!(
            rules[2].headers,
            [
                ("a".to_string(), "b".to_string()),
                ("foo".to_string(), "bar".to_string()),
            ]
        );
        assert_eq!(rules[3].data, MapData::Base64(b"test".to_vec()));
        assert_eq!(rules[3].status, 200);
        assert_eq!(rules[4].data, MapData::Text(String::new()));
        assert_eq!(rules[4].status, 200);
    }

    /// `data-type` is optional and `file` by default; a `file` line may name a
    /// URL instead, an absolute path stays as written, and each of the other
    /// three types fills its data in its own way (phase 4 M3 design 10.1).
    #[test]
    fn map_local_data_types_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        for line in [r#"^a data="x.bin""#, r#"^a data="x.bin" data-type=FILE"#] {
            let (rules, diags) = read_map_local_from(line, base, Origin::Main);
            assert!(diags.is_empty(), "{line}: {:?}", said(&diags));
            assert_eq!(rules.len(), 1, "{line}");
            assert_eq!(rules[0].data, MapData::File(base.join("x.bin")), "{line}");
            assert_eq!(rules[0].status, 200, "{line}");
            assert!(rules[0].headers.is_empty(), "{line}");
        }

        for (line, url) in [
            (
                r#"^a data="https://a.test/x.json""#,
                "https://a.test/x.json",
            ),
            (r#"^a data="HTTP://a.test/x""#, "http://a.test/x"),
        ] {
            let (rules, diags) = read_map_local_from(line, base, Origin::Main);
            assert!(diags.is_empty(), "{line}: {:?}", said(&diags));
            assert_eq!(
                rules[0].data,
                MapData::Url(url::Url::parse(url).unwrap()),
                "{line}"
            );
        }

        // an absolute path is the file itself, not one under `base_dir`
        let absolute = std::env::temp_dir().join("abs.json");
        let line = format!(r#"^a data="{}""#, absolute.display());
        let (rules, diags) = read_map_local_from(&line, base, Origin::Main);
        assert!(diags.is_empty(), "{line}: {:?}", said(&diags));
        assert_eq!(rules[0].data, MapData::File(absolute), "{line}");

        // a type written without its data answers with nothing
        for (line, data) in [
            ("^a data-type=text", MapData::Text(String::new())),
            ("^a data-type=base64", MapData::Base64(Vec::new())),
        ] {
            let (rules, diags) = read_map_local_from(line, base, Origin::Main);
            assert!(diags.is_empty(), "{line}: {:?}", said(&diags));
            assert_eq!(rules[0].data, data, "{line}");
        }

        // `tiny-gif` does not read its data, and does not complain of it
        let (rules, diags) =
            read_map_local_from("^a data-type=tiny-gif data=whatever", base, Origin::Main);
        assert!(diags.is_empty(), "{:?}", said(&diags));
        assert_eq!(rules[0].data, MapData::TinyGif);
    }

    /// A `header` value is `name:value|name:value`, or the base64 of
    /// newline-separated header lines when it holds no `:` (manual: Features ›
    /// Map Local, phase 4 M3 design 10.1).
    #[test]
    fn map_local_reads_both_header_forms() {
        let (rules, diags) = read_map_local(r#"^a data=x header="a: b | c:d|""#);
        assert!(diags.is_empty(), "{:?}", said(&diags));
        assert_eq!(
            rules[0].headers,
            [
                ("a".to_string(), "b".to_string()),
                ("c".to_string(), "d".to_string()),
            ]
        );

        // base64 of "x-a: 1\r\nx-b: 2\n\n", with and without padding
        let lines = "x-a: 1\r\nx-b: 2\n\n";
        for encoded in [
            base64::engine::general_purpose::STANDARD.encode(lines),
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(lines),
        ] {
            assert!(!encoded.contains(':'), "{encoded}");
            let (rules, diags) = read_map_local(&format!(r#"^a data=x header="{encoded}""#));
            assert!(diags.is_empty(), "{encoded}: {:?}", said(&diags));
            assert_eq!(
                rules[0].headers,
                [
                    ("x-a".to_string(), "1".to_string()),
                    ("x-b".to_string(), "2".to_string()),
                ],
                "{encoded}"
            );
        }
    }

    /// Every way a `[Map Local]` line can be wrong gives `W0012` with the
    /// message of its own, and the line is not kept (phase 4 M3 design 10.1,
    /// the phase 4 / M3b plan's P5 and P7).
    #[test]
    fn map_local_refuses() {
        const REGEX: &str = "[Map Local]: the regular expression cannot be used (";
        const PARAMETER: &str = "[Map Local]: every item after the regular expression is a key=value parameter; the line is ignored";
        const TYPE: &str =
            "[Map Local]: data-type is file, text, tiny-gif or base64; the line is ignored";
        const FILE: &str =
            "[Map Local]: data-type=file takes a path or a URL in data; the line is ignored";
        const URL: &str = "[Map Local]: data is not a URL rurge can download; the line is ignored";
        const BASE64: &str = "[Map Local]: data is not valid base64; the line is ignored";
        const HEADER: &str = "[Map Local]: header is `name:value|name:value` or base64 of header lines, with valid names and values; the line is ignored";
        const STATUS: &str =
            "[Map Local]: status-code is a whole number from 200 to 999; the line is ignored";

        // the first item is the URL expression, always, and the rest are
        // parameters: a word that is not one is not read as a value
        for (line, message) in [
            ("^a data", PARAMETER),
            ("^a data-type=text mystery", PARAMETER),
        ] {
            let (rules, diags) = read_map_local(line);
            assert!(rules.is_empty(), "{line}");
            assert_eq!(said(&diags), [("W0012", message)], "{line}");
        }

        let (rules, diags) = read_map_local("( data-type=text");
        assert!(rules.is_empty());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "W0012");
        assert!(diags[0].message.starts_with(REGEX), "{}", diags[0].message);

        for (line, message) in [
            ("^a data-type=xml", TYPE),
            ("^a data-type=file", FILE),
            (r#"^a data="""#, FILE),
            ("data-type=text", FILE),
            (r#"^a data="https://[x/""#, URL),
            ("^a data-type=base64 data=@@@", BASE64),
            ("^a data-type=text status-code=199", STATUS),
            ("^a data-type=text status-code=1000", STATUS),
            ("^a data-type=text status-code=x", STATUS),
            (r#"^a data-type=text header="a b:c""#, HEADER),
            ("^a data-type=text header=@@@", HEADER),
            (r#"^a data-type=text header="a:b|c""#, HEADER),
        ] {
            let (rules, diags) = read_map_local(line);
            assert!(rules.is_empty(), "{line}");
            assert_eq!(said(&diags), [("W0012", message)], "{line}");
        }

        // a header value that is not text a header may carry, base64 encoded
        let encoded = base64::engine::general_purpose::STANDARD.encode("a: b\u{1}c");
        let (rules, diags) = read_map_local(&format!(r#"^a data-type=text header="{encoded}""#));
        assert!(rules.is_empty(), "{encoded}");
        assert_eq!(said(&diags), [("W0012", HEADER)], "{encoded}");
    }

    /// A parameter `[Map Local]` does not know is `W0001`, and the line is
    /// kept; the key is named only when it is written the way a parameter name
    /// is, so that nothing of the line is repeated (phase 4 M3 design 10.1).
    #[test]
    fn map_local_unknown_keys_get_w0001() {
        let (rules, diags) = read_map_local("^a data-type=text mystery=1");
        assert_eq!(rules.len(), 1);
        assert_eq!(
            said(&diags),
            [("W0001", "[Map Local]: unknown parameter `mystery` ignored")]
        );

        let (rules, diags) = read_map_local("^a data-type=text Weird.Key=1");
        assert_eq!(rules.len(), 1);
        assert_eq!(
            said(&diags),
            [("W0001", "[Map Local]: unknown parameter ignored")]
        );
    }

    /// A parameter written twice takes the later value, and its key is read
    /// without regard to case (phase 4 M3 design 10.1).
    #[test]
    fn a_key_written_twice_takes_the_later() {
        let (rules, diags) = read_map_local("^a data-type=text data=a data=b");
        assert!(diags.is_empty(), "{:?}", said(&diags));
        assert_eq!(rules[0].data, MapData::Text("b".to_string()));

        let (rules, diags) = read_map_local("^a DATA-TYPE=base64 data-type=text data=x");
        assert!(diags.is_empty(), "{:?}", said(&diags));
        assert_eq!(rules[0].data, MapData::Text("x".to_string()));
    }

    /// `Content-Length` and `Transfer-Encoding` in a `[Map Local]` `header`
    /// are dropped, the rest of the line is kept, and one `W0041` says so
    /// (phase 4 M3 design 10.1, the phase 4 / M3b plan's P7).
    #[test]
    fn framing_headers_in_map_local_are_dropped_with_w0041() {
        let (rules, diags) = read_map_local(
            r#"^a data-type=text header="Content-Length:5|X-A:1|transfer-encoding:chunked""#,
        );
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].headers, [("X-A".to_string(), "1".to_string())]);
        assert_eq!(diags.len(), 1, "{:?}", said(&diags));
        let d = &diags[0];
        assert_eq!(d.code, "W0041");
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(
            d.message,
            "[Map Local]: Content-Length and Transfer-Encoding in header are not used; the rest of the line is kept"
        );
        assert_eq!(
            d.hint,
            Some(
                "rurge frames the answer by the length of its data: remove the two names from header"
            )
        );
    }

    /// A path in text rurge downloaded is refused with `W0033` and the line is
    /// dropped; a URL in the same text is what to write instead (phase 4 M3
    /// design 10.1, the phase 4 / M3b plan's P6).
    #[test]
    fn a_map_local_path_from_downloaded_text_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        for (origin, message, hint) in [
            (
                Origin::Remote(Arc::from(Path::new("1.conf"))),
                "a downloaded file cannot name a local file for Map Local; the line is ignored",
                "write the line in the local profile to map to a local file",
            ),
            (
                Origin::Managed(Arc::from(Path::new("1.conf"))),
                "the downloaded copy of a managed profile cannot name a local file for Map Local; the line is ignored",
                "give `data` a URL instead",
            ),
        ] {
            let (rules, diags) = read_map_local_from("^a data=x.json", dir.path(), origin.clone());
            assert!(rules.is_empty(), "{origin:?}");
            assert_eq!(diags.len(), 1, "{origin:?}");
            let d = &diags[0];
            assert_eq!(d.code, "W0033", "{origin:?}");
            assert_eq!(d.message, format!("[Map Local]: {message}"), "{origin:?}");
            assert_eq!(d.hint, Some(hint), "{origin:?}");

            let (rules, diags) =
                read_map_local_from(r#"^a data="https://a.test/x""#, dir.path(), origin.clone());
            assert!(diags.is_empty(), "{origin:?}: {:?}", said(&diags));
            assert_eq!(
                rules[0].data,
                MapData::Url(url::Url::parse("https://a.test/x").unwrap()),
                "{origin:?}"
            );
        }
    }

    /// A path written in a local include joins the directory of the main
    /// profile, not that of the file the line stands in (phase 4 M3 design
    /// 10.1).
    #[test]
    fn relative_paths_join_the_main_profile_directory() {
        let dir = tempfile::tempdir().unwrap();
        let origin = Origin::Include(Arc::from(Path::new("other/part.conf")));
        let (rules, diags) = read_map_local_from("^a data=x.json", dir.path(), origin);
        assert!(diags.is_empty(), "{:?}", said(&diags));
        assert_eq!(rules[0].data, MapData::File(dir.path().join("x.json")));
    }

    /// A `Debug` of a `[Map Local]` rule shows neither the text nor the bytes
    /// it answers with, nor a header value, nor the query of a URL (phase 4 M3
    /// design 8.5 and 14).
    #[test]
    fn the_debug_of_map_local_shows_no_data_or_value() {
        let bytes = base64::engine::general_purpose::STANDARD.encode("s3cr3t-b");
        let text = format!(
            "^a data-type=text data=\"s3cr3t-t\"\n\
^a data-type=base64 data={bytes}\n\
^a data=y header=\"X-Token: s3cr3t-h\"\n\
^a data=\"https://a.test/p?tok=q1\""
        );
        let (rules, diags) = read_map_local(&text);
        assert!(diags.is_empty(), "{:?}", said(&diags));
        assert_eq!(rules.len(), 4);
        let shown = format!("{rules:?}");
        for part in ["s3cr3t-t", "s3cr3t-b", "s3cr3t-h", "tok", "q1"] {
            assert!(!shown.contains(part), "{part}: {shown}");
        }
        assert!(shown.contains("X-Token"), "{shown}");
        assert!(shown.contains("https://a.test/…"), "{shown}");
    }
}
