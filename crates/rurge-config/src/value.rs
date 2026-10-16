//! Value-level helpers shared by all section parsers.
//!
//! How a list is cut into items (the line-splitting design, 4.1): a quote,
//! `"` or `'`, opens only where a value starts — at the start of the text,
//! after a comma or a `(` outside quotes, or after the first `=` since the
//! last of those, white space between allowed; inside `"` a backslash takes
//! the next character with it, inside `'` nothing escapes, and a quote with
//! no partner runs to the end. A quote anywhere else is a character. `(` and
//! `)` outside quotes group only when they pair up; a parenthesis that pairs
//! with nothing is a character. The items are cut at the commas outside
//! quotes and outside groups.

/// What the first pass of `item_spans` notes outside quotes.
#[derive(Clone, Copy)]
enum Mark {
    /// A comma that may cut: where it stands and how wide it is.
    Comma(usize, usize),
    Open,
    Close,
    /// The first `=` since a value last started.
    Eq(usize),
}

/// One item of a list: where it starts, where it ends, where the separator
/// after it ends, and where its first `=` that starts a value outside any
/// group stands (design 4.2; with nothing in front of it the item is still a
/// value).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ItemSpan {
    pub start: usize,
    pub end: usize,
    pub sep_end: usize,
    pub eq: Option<usize>,
}

/// The items of `text` (design 4.1), in two passes over it: the first finds
/// the quotes and notes the commas, parentheses and value-starting `=` outside
/// them; the second pairs the parentheses on a stack and reads the cuts off
/// the notes. Every item is there, an empty one too. When `wide`, the
/// full-width comma an input method types cuts as well (design LS-D10); it
/// starts no value.
pub(crate) fn item_spans(text: &str, wide: bool) -> Vec<ItemSpan> {
    let mut marks = Vec::new();
    let mut at_start = true;
    let mut seen_eq = false;
    let mut quote: Option<char> = None;
    let mut chars = text.char_indices();
    while let Some((i, c)) = chars.next() {
        if let Some(q) = quote {
            if q == '"' && c == '\\' {
                chars.next();
            } else if c == q {
                quote = None;
            }
            continue;
        }
        if at_start && c.is_whitespace() {
            continue;
        }
        if at_start && matches!(c, '"' | '\'') {
            quote = Some(c);
            at_start = false;
            continue;
        }
        at_start = false;
        match c {
            ',' => {
                marks.push(Mark::Comma(i, 1));
                at_start = true;
                seen_eq = false;
            }
            '，' if wide => marks.push(Mark::Comma(i, '，'.len_utf8())),
            '(' => {
                marks.push(Mark::Open);
                at_start = true;
                seen_eq = false;
            }
            ')' => marks.push(Mark::Close),
            '=' if !seen_eq => {
                marks.push(Mark::Eq(i));
                seen_eq = true;
                at_start = true;
            }
            _ => {}
        }
    }
    let mut paired = vec![false; marks.len()];
    let mut open = Vec::new();
    for (k, mark) in marks.iter().enumerate() {
        match mark {
            Mark::Open => open.push(k),
            Mark::Close => {
                if let Some(o) = open.pop() {
                    paired[o] = true;
                    paired[k] = true;
                }
            }
            _ => {}
        }
    }
    let mut spans = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut eq = None;
    for (k, mark) in marks.iter().enumerate() {
        match *mark {
            Mark::Open if paired[k] => depth += 1,
            Mark::Close if paired[k] => depth -= 1,
            Mark::Eq(at) if depth == 0 && eq.is_none() => eq = Some(at),
            Mark::Comma(at, width) if depth == 0 => {
                spans.push(ItemSpan {
                    start,
                    end: at,
                    sep_end: at + width,
                    eq: eq.take(),
                });
                start = at + width;
            }
            _ => {}
        }
    }
    spans.push(ItemSpan {
        start,
        end: text.len(),
        sep_end: text.len(),
        eq,
    });
    spans
}

/// The items of `text` as they stand in it, blanks around them included
/// (design 4.1).
pub(crate) fn split_items(text: &str) -> Vec<&str> {
    item_spans(text, false)
        .into_iter()
        .map(|span| &text[span.start..span.end])
        .collect()
}

/// One item of a list as every parser reads it (design 4.2).
#[derive(Clone, PartialEq, Eq)]
pub struct Field {
    /// What `split_list` gives for the item.
    pub text: String,
    /// The parameter the item is: its key as written, and its value with its
    /// quotes taken off once. `None` for a value.
    pub param: Option<(String, String)>,
}

/// An item can be a password: nothing of it is shown.
impl std::fmt::Debug for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Field(***)")
    }
}

/// The items of a comma-separated list, each read as a value or a parameter
/// (design 4.2). An item that is blank, or wholly in quotes with nothing
/// inside them, is dropped.
pub fn split_fields(text: &str) -> Vec<Field> {
    item_spans(text, false)
        .into_iter()
        .filter_map(|span| {
            read_field(
                &text[span.start..span.end],
                span.eq.map(|at| at - span.start),
            )
        })
        .collect()
}

/// Split a comma-separated list into its items (design 4.1): each is trimmed,
/// one wholly in quotes loses them, a parameter whose value is wholly in
/// quotes is `key=value` with them off; empty items are dropped.
pub fn split_list(text: &str) -> Vec<String> {
    split_fields(text)
        .into_iter()
        .map(|field| field.text)
        .collect()
}

/// `item`, one item of a list, as a value or a parameter; `eq` is where its
/// first `=` that starts a value outside any group stands, if it has one.
fn read_field(item: &str, eq: Option<usize>) -> Option<Field> {
    let trimmed = item.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(inner) = wholly_quoted(trimmed) {
        return (!inner.is_empty()).then_some(Field {
            text: inner,
            param: None,
        });
    }
    if let Some(at) = eq {
        let key = item[..at].trim();
        if !key.is_empty() {
            let raw = item[at + 1..].trim();
            let (value, text) = match wholly_quoted(raw) {
                Some(value) => {
                    let text = format!("{key}={value}");
                    (value, text)
                }
                None => (raw.to_string(), trimmed.to_string()),
            };
            return Some(Field {
                text,
                param: Some((key.to_string(), value)),
            });
        }
    }
    Some(Field {
        text: trimmed.to_string(),
        param: None,
    })
}

/// The text inside the quotes when `text` is wholly in them: it opens with a
/// quote whose partner is its last character (design LS-D4). Inside `"`, `\"`
/// is `"` and `\\` is `\`; any other backslash stays.
fn wholly_quoted(text: &str) -> Option<String> {
    let quote = text.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    if partner(text, quote)? != text.len() - 1 {
        return None;
    }
    let inner = &text[1..text.len() - 1];
    Some(if quote == '"' {
        unescape(inner)
    } else {
        inner.to_string()
    })
}

/// Where the partner of the quote that opens `text` stands: the next `quote`
/// after it, one escaped by a backslash not counted inside `"`.
fn partner(text: &str, quote: char) -> Option<usize> {
    let mut chars = text.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        if quote == '"' && c == '\\' {
            chars.next();
        } else if c == quote {
            return Some(i);
        }
    }
    None
}

/// Strip the quotes of a text that is wholly in them (design LS-D4); any
/// other text is returned as it is. Double quotes support `\"` and `\\`
/// escapes.
pub fn unquote(text: &str) -> String {
    wholly_quoted(text).unwrap_or_else(|| text.to_string())
}

/// The inside of a double-quoted value: `\"` is `"`, `\\` is `\`, any other
/// backslash stays with what follows it, and one at the end stays.
fn unescape(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Every string of up to six characters over the characters the reading
/// looks at — 597,870 of them — for the tests that compare readings.
#[cfg(test)]
pub(crate) fn short_strings() -> impl Iterator<Item = String> {
    const ALPHABET: [char; 9] = ['a', ',', '"', '\'', '(', ')', '=', '\\', ' '];
    (1..=6u32).flat_map(|len| {
        (0..ALPHABET.len().pow(len)).map(move |mut n| {
            (0..len)
                .map(|_| {
                    let c = ALPHABET[n % ALPHABET.len()];
                    n /= ALPHABET.len();
                    c
                })
                .collect::<String>()
        })
    })
}

/// Case-insensitive, boundary-safe prefix test: `s.get(..prefix.len())` fails
/// closed (returns `false`) instead of panicking when `prefix.len()` falls
/// inside a multi-byte character rather than on a char boundary.
pub(crate) fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// Case-insensitive, boundary-safe prefix strip. `None` both when `s` doesn't
/// start with `prefix` and when `prefix.len()` isn't a char boundary of `s`.
pub(crate) fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    match s.get(..prefix.len()) {
        Some(head) if head.eq_ignore_ascii_case(prefix) => Some(&s[prefix.len()..]),
        _ => None,
    }
}

/// Split `Name = rest` at the first `=`.
pub fn split_definition(raw: &str) -> Option<(&str, &str)> {
    let (name, rest) = raw.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some((name, rest.trim()))
}

/// Split `key=value` at the first `=`.
pub fn parse_key_value(field: &str) -> Option<(&str, &str)> {
    split_definition(field)
}

pub fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

/// Ordered, case-insensitive `key=value` parameters. Duplicate keys are kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParamMap {
    entries: Vec<(String, String)>,
}

impl ParamMap {
    pub fn insert(&mut self, key: &str, value: &str) {
        self.entries
            .push((key.trim().to_ascii_lowercase(), unquote(value.trim())));
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        let key = key.to_ascii_lowercase();
        self.entries
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }
    pub fn get_all(&self, key: &str) -> Vec<&str> {
        let key = key.to_ascii_lowercase();
        self.entries
            .iter()
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
            .collect()
    }
    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
    pub fn bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(parse_bool)
    }
    pub fn u16(&self, key: &str) -> Option<u16> {
        self.get(key).and_then(|v| v.parse().ok())
    }
    pub fn u32(&self, key: &str) -> Option<u32> {
        self.get(key).and_then(|v| v.parse().ok())
    }
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// Split fields into `key=value` parameters and positional values.
    pub fn from_fields(fields: &[String]) -> (ParamMap, Vec<String>) {
        let mut params = ParamMap::default();
        let mut positional = Vec::new();
        for f in fields {
            match parse_key_value(f) {
                Some((k, v)) => params.insert(k, v),
                None => positional.push(f.clone()),
            }
        }
        (params, positional)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_list_cases() {
        let cases: &[(&str, &[&str])] = &[
            ("a, b ,c", &["a", "b", "c"]),
            ("", &[]),
            ("a,,b", &["a", "b"]),
            (
                "ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=\"p,w\"",
                &[
                    "ss",
                    "1.2.3.4",
                    "8388",
                    "encrypt-method=aes-128-gcm",
                    "password=p,w",
                ],
            ),
            (
                "AND,((SRC-IP,1.2.3.4),(DOMAIN-SUFFIX,a.com)),DIRECT",
                &["AND", "((SRC-IP,1.2.3.4),(DOMAIN-SUFFIX,a.com))", "DIRECT"],
            ),
            (
                "URL-REGEX,\"^http://x/(a|b),?c\",Proxy",
                &["URL-REGEX", "^http://x/(a|b),?c", "Proxy"],
            ),
            (
                "peer = (public-key = k, allowed-ips = \"10.0.0.0/8, 192.168.0.0/16\", endpoint = a:1), (public-key = j, allowed-ips = 0.0.0.0/0, endpoint = b:2)",
                &[
                    "peer = (public-key = k, allowed-ips = \"10.0.0.0/8, 192.168.0.0/16\", endpoint = a:1)",
                    "(public-key = j, allowed-ips = 0.0.0.0/0, endpoint = b:2)",
                ],
            ),
            ("x='a,b',c", &["x=a,b", "c"]),
        ];
        for (input, expected) in cases {
            let got = split_list(input);
            assert_eq!(got, *expected, "input: {input}");
        }
    }

    #[test]
    fn unquote_cases() {
        assert_eq!(unquote("\"a \\\"b\\\" c\\\\d\""), "a \"b\" c\\d");
        assert_eq!(unquote("'x'"), "x");
        assert_eq!(unquote("plain"), "plain");
        assert_eq!(unquote("\"unterminated"), "\"unterminated");
        assert_eq!(unquote("\"P1\" \"x\""), "\"P1\" \"x\"");
        assert_eq!(unquote("'it's'"), "'it's'");
        assert_eq!(unquote("\"\\\""), "\"\\\"");
    }

    type Item = (String, Option<(String, String)>);

    fn value(text: &str) -> Item {
        (text.to_string(), None)
    }

    fn param(text: &str, key: &str, val: &str) -> Item {
        (text.to_string(), Some((key.to_string(), val.to_string())))
    }

    fn read(text: &str) -> Vec<Item> {
        split_fields(text)
            .into_iter()
            .map(|f| (f.text, f.param))
            .collect()
    }

    /// Design 4.3, row by row, and the corners of 4.2: where a quote opens,
    /// which parentheses group, what an item reads as, and what `split_list`
    /// gives for it.
    #[test]
    fn a_quote_opens_only_where_a_value_starts_and_a_parenthesis_groups_only_in_a_pair() {
        let peer =
            "peer = (public-key = k, allowed-ips = \"10.0.0.0/8, 192.168.0.0/16\", endpoint = a:1)";
        let cases: Vec<(&str, Vec<Item>)> = vec![
            (
                "select, Tom's HK, Tom's US",
                vec![value("select"), value("Tom's HK"), value("Tom's US")],
            ),
            (
                "USER-AGENT,Bob's App*,DIRECT",
                vec![value("USER-AGENT"), value("Bob's App*"), value("DIRECT")],
            ),
            (
                "DOMAIN,b.example,Proxy (US,extended-matching",
                vec![
                    value("DOMAIN"),
                    value("b.example"),
                    value("Proxy (US"),
                    value("extended-matching"),
                ],
            ),
            (
                r"URL-REGEX,^http://a\.example/[^(]*$,REJECT",
                vec![
                    value("URL-REGEX"),
                    value(r"^http://a\.example/[^(]*$"),
                    value("REJECT"),
                ],
            ),
            (
                "AND,((DOMAIN,a.example),(DEST-PORT,443)),Proxy (US)",
                vec![
                    value("AND"),
                    value("((DOMAIN,a.example),(DEST-PORT,443))"),
                    value("Proxy (US)"),
                ],
            ),
            (
                r#"URL-REGEX,"^http://example\.com/(a|b),?c",Proxy"#,
                vec![
                    value("URL-REGEX"),
                    value(r"^http://example\.com/(a|b),?c"),
                    value("Proxy"),
                ],
            ),
            (
                "password=\"p,w\"",
                vec![param("password=p,w", "password", "p,w")],
            ),
            ("x='a,b'", vec![param("x=a,b", "x", "a,b")]),
            (
                "password=ab\"c,d\"",
                vec![param("password=ab\"c", "password", "ab\"c"), value("d\"")],
            ),
            (
                "a=b=\"c,d\"",
                vec![param("a=b=\"c", "a", "b=\"c"), value("d\"")],
            ),
            ("\"pa=ss\"", vec![value("pa=ss")]),
            (
                r#"sni="\"a.example\"""#,
                vec![param("sni=\"a.example\"", "sni", "\"a.example\"")],
            ),
            (
                "password=\" pw \"",
                vec![param("password= pw ", "password", " pw ")],
            ),
            ("\"P1\" \"x\"", vec![value("\"P1\" \"x\"")]),
            ("\"Fast\" Proxy", vec![value("\"Fast\" Proxy")]),
            ("P1, \"P1", vec![value("P1"), value("\"P1")]),
            ("Proxy (\"US, Other", vec![value("Proxy (\"US, Other")]),
            ("(USER-AGENT,\"a)b\")", vec![value("(USER-AGENT,\"a)b\")")]),
            (
                peer,
                vec![param(
                    peer,
                    "peer",
                    "(public-key = k, allowed-ips = \"10.0.0.0/8, 192.168.0.0/16\", endpoint = a:1)",
                )],
            ),
            ("=(a)=b", vec![value("=(a)=b")]),
            ("(a=b)", vec![value("(a=b)")]),
            ("(a=b)=c", vec![value("(a=b)=c")]),
            ("a(b)=c", vec![param("a(b)=c", "a(b)", "c")]),
            ("(tfo=false", vec![param("(tfo=false", "(tfo", "false")]),
            (
                "peer = (a = b)",
                vec![param("peer = (a = b)", "peer", "(a = b)")],
            ),
            ("a = b", vec![param("a = b", "a", "b")]),
            ("\"a\"=b", vec![param("\"a\"=b", "\"a\"", "b")]),
            ("\"\"", vec![]),
            ("''", vec![]),
            ("\" \"", vec![value(" ")]),
            (", ,", vec![]),
        ];
        for (input, expected) in cases {
            assert_eq!(read(input), expected, "{input:?}");
            let texts: Vec<String> = expected.into_iter().map(|(text, _)| text).collect();
            assert_eq!(split_list(input), texts, "{input:?}");
        }
    }

    fn shift(spans: Vec<ItemSpan>, by: usize) -> Vec<ItemSpan> {
        spans
            .into_iter()
            .map(|s| ItemSpan {
                start: s.start + by,
                end: s.end + by,
                sep_end: s.sep_end + by,
                eq: s.eq.map(|e| e + by),
            })
            .collect()
    }

    /// Design 6.2: read again from any cut, the rest of a text is cut where it
    /// was, and an item read alone is that item — what redaction and
    /// `with_params` rely on when they take an item out and read it again.
    /// Every string of up to six characters the reading looks at.
    #[test]
    fn the_reading_is_the_same_from_any_cut_and_of_any_item_alone() {
        let mut checked = 0usize;
        for text in short_strings() {
            let spans = item_spans(&text, false);
            for (k, span) in spans.iter().enumerate() {
                if k + 1 < spans.len() {
                    assert_eq!(
                        shift(item_spans(&text[span.sep_end..], false), span.sep_end).as_slice(),
                        &spans[k + 1..],
                        "{text:?}"
                    );
                }
                assert_eq!(
                    shift(item_spans(&text[span.start..span.end], false), span.start),
                    [ItemSpan {
                        sep_end: span.end,
                        ..*span
                    }],
                    "{text:?}"
                );
            }
            checked += 1;
        }
        assert!(checked > 500_000, "{checked}");
    }

    /// The full-width comma an input method types cuts only when asked
    /// (`wide`), and it starts no value: a quote right after it is a
    /// character, and a parenthesis after it still groups when it pairs
    /// (line-splitting design LS-D10).
    #[test]
    fn a_full_width_comma_cuts_only_when_asked_and_starts_no_value() {
        let cut = |text: &str, wide: bool| -> Vec<String> {
            item_spans(text, wide)
                .into_iter()
                .map(|span| text[span.start..span.end].to_string())
                .collect()
        };
        assert_eq!(cut("a，b", false), ["a，b"]);
        assert_eq!(cut("a，b", true), ["a", "b"]);
        assert_eq!(cut("a，\"b，c\"", true), ["a", "\"b", "c\""]);
        assert_eq!(cut("a，(b，c)", true), ["a", "(b，c)"]);
    }

    /// A line of one kind of character, three hundred thousand of them, is
    /// read in time that grows with its length: a reading that starts over
    /// after every cut, or looks ahead from every parenthesis, would take
    /// minutes.
    #[test]
    fn a_long_line_of_one_kind_is_read_in_time_that_grows_with_its_length() {
        let n = 300_000usize;
        let lines: Vec<(&str, String)> = vec![
            ("open parentheses", "(".repeat(n)),
            ("close parentheses", ")".repeat(n)),
            ("double quotes", "\"".repeat(n)),
            ("single quotes", "'".repeat(n)),
            ("open parentheses and commas", "(,".repeat(n / 2)),
            ("apostrophes and commas", "'a,".repeat(n / 3)),
            ("first equals signs", "a=".repeat(n / 2)),
            ("quotes before parentheses", "a\"(".repeat(n / 3)),
            ("quotes before equals signs", "a\"=".repeat(n / 3)),
        ];
        for (kind, line) in &lines {
            let started = std::time::Instant::now();
            let fields = split_fields(line);
            let took = started.elapsed();
            std::hint::black_box(fields);
            assert!(
                took < std::time::Duration::from_secs(10),
                "{kind} ({} bytes) took {took:?}",
                line.len()
            );
        }
    }

    #[test]
    fn definitions_and_params() {
        assert_eq!(
            split_definition("Proxy = ss, a, 1"),
            Some(("Proxy", "ss, a, 1"))
        );
        assert_eq!(split_definition("  a.b = c=d "), Some(("a.b", "c=d")));
        assert_eq!(split_definition("= x"), None);
        assert_eq!(split_definition("no equals"), None);
        assert_eq!(parse_key_value("Key = Value"), Some(("Key", "Value")));
        assert_eq!(parse_bool("TRUE"), Some(true));
        assert_eq!(parse_bool("0"), Some(false));
        assert_eq!(parse_bool("maybe"), None);

        let fields = split_list("user, pass, tfo=true, args=a, args=b, Port=443");
        let (params, positional) = ParamMap::from_fields(&fields);
        assert_eq!(positional, ["user", "pass"]);
        assert_eq!(params.get("TFO"), Some("true"));
        assert_eq!(params.bool("tfo"), Some(true));
        assert_eq!(params.get_all("args"), ["a", "b"]);
        assert_eq!(params.u16("port"), Some(443));
        assert_eq!(params.len(), 4);
    }
}
