//! The ICU dialect of the regular expressions a profile writes, and the
//! replacement templates that go with them.
//!
//! Surge reads a profile's regular expressions with ICU and expands a
//! replacement the way `NSRegularExpression` does. The two regular-expression
//! crates rurge compiles with (fancy-regex and regex) read several of the same
//! spellings with another meaning, or refuse them; `translate` rewrites those
//! before compiling (phase 4 M3 design 5.2). `Template` reads a replacement
//! after swift-corelibs-foundation's `NSRegularExpression`, so that the
//! groups are numbered the way Surge numbers them (phase 4 M3 design 5.3, the
//! phase 4 / M3a plan's P4).

/// The seven line terminators ICU reads, as the body of a character class.
const LINE_BREAKS: &str = "\\n\\x0B\\f\\r\\x{85}\\x{2028}\\x{2029}";
/// The horizontal blanks ICU reads, as the body of a character class.
const BLANKS: &str = "\\t\\p{Zs}";

/// Rewrites the ICU spellings that fancy-regex and regex read otherwise, so
/// that a compiled pattern means what ICU means (phase 4 M3 design 5.2).
///
/// A backslash and the character after it are read as one; the escapes ICU
/// gives a meaning that differs from the crates' are replaced, `\Q…\E` turns
/// its text into literals, and a character class is followed so that its
/// members are read as such. The result is handed to the regex crate, which
/// reports what is still wrong; a lone backslash at the end is copied for it
/// to report.
pub fn translate(source: &str) -> Result<String, String> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    let mut depth = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            let Some(&next) = chars.get(i + 1) else {
                out.push('\\');
                break;
            };
            i += 2;
            match next {
                'h' => push_member(&mut out, BLANKS, depth > 0),
                'H' if depth > 0 => return Err(refused_in_class(next)),
                'H' => push_member(&mut out, &format!("^{BLANKS}"), false),
                'v' => push_member(&mut out, LINE_BREAKS, depth > 0),
                'V' if depth > 0 => return Err(refused_in_class(next)),
                'V' => push_member(&mut out, &format!("^{LINE_BREAKS}"), false),
                'R' if depth > 0 => return Err(refused_in_class(next)),
                'R' => {
                    out.push_str("(?:\\r\\n|[");
                    out.push_str(LINE_BREAKS);
                    out.push_str("])");
                }
                'Z' if depth == 0 => {
                    out.push_str("(?=(?:\\r\\n|[");
                    out.push_str(LINE_BREAKS);
                    out.push_str("])?\\z)");
                }
                '<' => out.push('<'),
                '>' => out.push('>'),
                'Q' => i = push_quoted(&chars, i, &mut out),
                _ => {
                    out.push('\\');
                    out.push(next);
                }
            }
            continue;
        }
        if c == '[' {
            out.push('[');
            i += 1;
            if depth == 0 {
                depth = 1;
                if chars.get(i) == Some(&'^') {
                    out.push('^');
                    i += 1;
                }
                if chars.get(i) == Some(&']') {
                    out.push(']');
                    i += 1;
                }
            } else {
                depth += 1;
            }
            continue;
        }
        if c == ']' && depth > 0 {
            out.push(']');
            i += 1;
            depth -= 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    Ok(out)
}

/// Writes `body` as a class's member: inside a class it stands as it is,
/// outside it needs a class of its own.
fn push_member(out: &mut String, body: &str, in_class: bool) {
    if !in_class {
        out.push('[');
    }
    out.push_str(body);
    if !in_class {
        out.push(']');
    }
}

fn refused_in_class(letter: char) -> String {
    format!("`\\{letter}` cannot be used inside a character class")
}

/// Writes the text of a `\Q…\E` section as literals, up to the `\E` or the
/// end, and tells where reading goes on.
fn push_quoted(chars: &[char], mut i: usize, out: &mut String) -> usize {
    while i < chars.len() {
        if chars[i] == '\\' && chars.get(i + 1) == Some(&'E') {
            return i + 2;
        }
        push_literal(chars[i], out);
        i += 1;
    }
    i
}

/// Writes one character of a `\Q…\E` section so that it stays a literal: a
/// character of syntax gets a backslash, white space its code point (under
/// `(?x)` it would otherwise pass), and anything else stands as it is.
fn push_literal(c: char, out: &mut String) {
    if matches!(
        c,
        '\\' | '.'
            | '+'
            | '*'
            | '?'
            | '('
            | ')'
            | '|'
            | '['
            | ']'
            | '{'
            | '}'
            | '^'
            | '$'
            | '#'
            | '&'
            | '-'
            | '~'
    ) {
        out.push('\\');
        out.push(c);
    } else if c.is_whitespace() {
        out.push_str(&format!("\\x{{{:x}}}", c as u32));
    } else {
        out.push(c);
    }
}

/// One piece of a replacement template.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Piece {
    /// Text that goes in as written.
    Text(String),
    /// A `$` and the digits after it, all of them.
    Group(String),
}

/// A replacement template, read the way `NSRegularExpression` reads one.
#[derive(Clone, PartialEq, Eq)]
pub struct Template {
    pieces: Vec<Piece>,
}

/// A template holds header values and URL paths: nothing of it is shown.
impl std::fmt::Debug for Template {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Template(***)")
    }
}

impl Template {
    /// Reads `text`: `\` takes the next character as written, a trailing one
    /// is dropped, `$` and the digits after it name a group, and anything
    /// else is text.
    pub fn parse(text: &str) -> Template {
        let chars: Vec<char> = text.chars().collect();
        let mut pieces = Vec::new();
        let mut run = String::new();
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '\\' => {
                    if let Some(&next) = chars.get(i + 1) {
                        run.push(next);
                    }
                    i += 2;
                }
                '$' => {
                    let mut j = i + 1;
                    while j < chars.len() && chars[j].is_ascii_digit() {
                        j += 1;
                    }
                    if j == i + 1 {
                        run.push('$');
                        i += 1;
                    } else {
                        if !run.is_empty() {
                            pieces.push(Piece::Text(std::mem::take(&mut run)));
                        }
                        pieces.push(Piece::Group(chars[i + 1..j].iter().collect()));
                        i = j;
                    }
                }
                c => {
                    run.push(c);
                    i += 1;
                }
            }
        }
        if !run.is_empty() {
            pieces.push(Piece::Text(run));
        }
        Template { pieces }
    }

    /// Writes `text` with the groups `group` gives expanded. `count` is the
    /// number of groups, the whole match not counted: `$` reads at most as
    /// many digits as `count` has, so that `$12` with two groups is the first
    /// group and a `2`.
    pub fn expand<'a>(
        &self,
        group: &dyn Fn(usize) -> Option<&'a str>,
        count: usize,
        out: &mut String,
    ) {
        let digits = count.to_string().len();
        for piece in &self.pieces {
            match piece {
                Piece::Text(text) => out.push_str(text),
                Piece::Group(number) => {
                    let take = digits.min(number.len());
                    let index: usize = number[..take].parse().unwrap_or(0);
                    let text = if index <= count { group(index) } else { None };
                    if let Some(text) = text {
                        out.push_str(text);
                    }
                    out.push_str(&number[take..]);
                }
            }
        }
    }
}

/// The limit on the steps fancy-regex's backtracking engine may take for one
/// pattern of a `[Body Rewrite]` line (`phase 4 M3 design 5.4`).
///
/// fancy-regex 0.14 counts a backtrack for every start that fails, so its
/// default of one million is spent on a body of about a megabyte whatever the
/// pattern is; a hundred million is about three times the largest body a
/// request may have (32 MiB), with room left for an expression that really
/// backtracks.
pub const BODY_BACKTRACK_LIMIT: usize = 100_000_000;

/// A `[Body Rewrite]` expression (`phase 4 M3 design 5.4`).
///
/// Two engines compile it. `regex` reads the start and end of a line the way
/// the manual wants: `\r` and `\n` both end a line, neither is matched in the
/// middle of one, and `.` matches neither. fancy-regex reads the lookarounds
/// and back references `regex` refuses, but its multi-line mode knows only
/// `\n`, so a pattern that lands there counts the `\r` of a CRLF line into the
/// line — a difference rurge registers (phase 4 M3 design 5.4).
#[derive(Clone)]
pub struct BodyPattern {
    /// The expression as the profile wrote it.
    pub source: String,
    engine: Engine,
}

/// The compiled form of a body pattern: which crate compiled it.
#[derive(Clone)]
enum Engine {
    Plain(regex::Regex),
    Fancy(fancy_regex::Regex),
}

impl BodyPattern {
    /// Compiles `source` with the limit a body rewrite uses.
    pub fn new(source: &str) -> Result<BodyPattern, String> {
        BodyPattern::with_backtrack_limit(source, BODY_BACKTRACK_LIMIT)
    }

    /// Compiles `source` with `limit` as fancy-regex's backtracking limit.
    ///
    /// `regex` compiles it first, with the ICU spellings rewritten; what that
    /// crate refuses is handed to fancy-regex with `(?m)` in front, and its
    /// reason is what comes back when that fails too (phase 4 M3 design 5.4).
    pub fn with_backtrack_limit(source: &str, limit: usize) -> Result<BodyPattern, String> {
        let translated = translate(source)?;
        if let Ok(regex) = regex::RegexBuilder::new(&translated)
            .multi_line(true)
            .crlf(true)
            .build()
        {
            return Ok(BodyPattern {
                source: source.to_string(),
                engine: Engine::Plain(regex),
            });
        }
        match fancy_regex::RegexBuilder::new(&format!("(?m){translated}"))
            .backtrack_limit(limit)
            .build()
        {
            Ok(regex) => Ok(BodyPattern {
                source: source.to_string(),
                engine: Engine::Fancy(regex),
            }),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Whether fancy-regex compiled this pattern rather than `regex`.
    pub fn is_fancy(&self) -> bool {
        matches!(self.engine, Engine::Fancy(_))
    }

    /// `text` with every non-overlapping match replaced by `template`'s
    /// expansion, from left to right; `None` when fancy-regex hits its
    /// backtracking limit.
    ///
    /// The work is done here rather than by either crate's `replace_all`:
    /// fancy-regex's panics on a runtime error (phase 4 M3 design 5.5).
    pub fn replace_all(&self, template: &Template, text: &str) -> Option<String> {
        let mut out = String::with_capacity(text.len());
        let mut end = 0;
        match &self.engine {
            Engine::Plain(regex) => {
                let groups = regex.captures_len() - 1;
                for captures in regex.captures_iter(text) {
                    let whole = captures.get(0).expect("every match is group zero");
                    out.push_str(&text[end..whole.start()]);
                    template.expand(
                        &|group| captures.get(group).map(|found| found.as_str()),
                        groups,
                        &mut out,
                    );
                    end = whole.end();
                }
            }
            Engine::Fancy(regex) => {
                let groups = regex.captures_len() - 1;
                for captures in regex.captures_iter(text) {
                    let captures = captures.ok()?;
                    let whole = captures.get(0).expect("every match is group zero");
                    out.push_str(&text[end..whole.start()]);
                    template.expand(
                        &|group| captures.get(group).map(|found| found.as_str()),
                        groups,
                        &mut out,
                    );
                    end = whole.end();
                }
            }
        }
        out.push_str(&text[end..]);
        Some(out)
    }
}

impl PartialEq for BodyPattern {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}
impl Eq for BodyPattern {}

/// A body pattern shows its expression, which is not a secret, and nothing
/// of the engine.
impl std::fmt::Debug for BodyPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("BodyPattern").field(&self.source).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::Pattern;

    /// Each ICU escape the regex crates read otherwise is rewritten (phase 4
    /// M3 design 5.2, the phase 4 / M3a plan's P3).
    #[test]
    fn translate_rewrites_the_icu_escapes() {
        let cases: &[(&str, &str)] = &[
            (r"\h", r"[\t\p{Zs}]"),
            (r"\H", r"[^\t\p{Zs}]"),
            (r"\v", r"[\n\x0B\f\r\x{85}\x{2028}\x{2029}]"),
            (r"\V", r"[^\n\x0B\f\r\x{85}\x{2028}\x{2029}]"),
            (r"\R", r"(?:\r\n|[\n\x0B\f\r\x{85}\x{2028}\x{2029}])"),
            (r"\<", "<"),
            (r"\>", ">"),
            (r"\Z", r"(?=(?:\r\n|[\n\x0B\f\r\x{85}\x{2028}\x{2029}])?\z)"),
            (r"\Qa.b", r"a\.b"),
            (r"\Q a\E", r"\x{20}a"),
            (r"\Q\E", ""),
        ];
        for (written, expected) in cases {
            assert_eq!(translate(written), Ok((*expected).to_string()), "{written}");
        }
    }

    /// What ICU gives no special meaning is copied as written (phase 4 M3
    /// design 5.2).
    #[test]
    fn translate_leaves_everything_else() {
        for source in [r"\\h", r"\d+\.", "[a-z]", "(?i)abc", r"\x{41}", r"a\b"] {
            assert_eq!(translate(source), Ok(source.to_string()), "{source}");
        }
    }

    /// In a character class the two blanks become its members, `<` and `>`
    /// lose their backslash, and `\Q…\E` escapes its text there too (phase
    /// 4 M3 design 5.2).
    #[test]
    fn translate_inside_a_class() {
        let cases: &[(&str, &str)] = &[
            (r"[\h]", r"[\t\p{Zs}]"),
            (r"[a\v]", r"[a\n\x0B\f\r\x{85}\x{2028}\x{2029}]"),
            (r"[\<\>]", "[<>]"),
            (r"[\Q-]\E]", r"[\-\]]"),
            (r"[]\h]", r"[]\t\p{Zs}]"),
            (r"[^]\h]", r"[^]\t\p{Zs}]"),
            (r"[[:alpha:]\h]", r"[[:alpha:]\t\p{Zs}]"),
        ];
        for (written, expected) in cases {
            assert_eq!(translate(written), Ok((*expected).to_string()), "{written}");
        }
    }

    /// Three escapes have no meaning inside a class: the line is refused
    /// with a message of its own (phase 4 M3 design 5.2).
    #[test]
    fn translate_refuses_three_inside_a_class() {
        for letter in ['H', 'V', 'R'] {
            let source = format!(r"[\{letter}]");
            assert_eq!(
                translate(&source),
                Err(format!(
                    "`\\{letter}` cannot be used inside a character class"
                )),
                "{source}"
            );
        }
    }

    /// A pattern compiled through `Pattern::new` reads the escapes as ICU
    /// reads them (phase 4 M3 design 5.2).
    #[test]
    fn translated_patterns_mean_what_icu_means() {
        let p = Pattern::new(r"a\hb").unwrap();
        assert!(p.regex.is_match("a b").unwrap());
        assert!(p.regex.is_match("a\tb").unwrap());
        assert!(!p.regex.is_match("aab").unwrap());
        let p = Pattern::new(r"a\Hb").unwrap();
        assert!(p.regex.is_match("aab").unwrap());
        assert!(!p.regex.is_match("a b").unwrap());
        let p = Pattern::new(r"a\vb").unwrap();
        assert!(p.regex.is_match("a\nb").unwrap());
        assert!(p.regex.is_match("a\u{2028}b").unwrap());
        let p = Pattern::new(r"\R").unwrap();
        assert_eq!(p.regex.find("\r\n").unwrap().unwrap().as_str(), "\r\n");
        let p = Pattern::new(r"a\<b\>").unwrap();
        assert!(p.regex.is_match("a<b>").unwrap());
        let p = Pattern::new(r"\Qa.b\E").unwrap();
        assert!(p.regex.is_match("a.b").unwrap());
        assert!(!p.regex.is_match("axb").unwrap());
        let p = Pattern::new(r"x\Z").unwrap();
        assert!(p.regex.is_match("x").unwrap());
        assert!(p.regex.is_match("x\n").unwrap());
        assert!(p.regex.is_match("x\r\n").unwrap());
        assert!(!p.regex.is_match("x\ny").unwrap());
        let p = Pattern::new(r"^\Q\.+*?()|[]{}^$#&-~\E$").unwrap();
        let text = r"\.+*?()|[]{}^$#&-~";
        assert_eq!(p.regex.find(text).unwrap().unwrap().as_str(), text);
    }

    /// The groups are numbered and the digits cut the way
    /// `NSRegularExpression` numbers and cuts them (phase 4 M3 design 5.3,
    /// the phase 4 / M3a plan's P4).
    #[test]
    fn template_expands_like_nsregularexpression() {
        let three = [Some("w"), Some("A"), Some("B")];
        assert_eq!(expand("$1-$2", &three), "A-B");
        assert_eq!(expand("$12", &three), "A2");
        assert_eq!(expand("$3", &three), "");
        assert_eq!(expand("$0", &three), "w");
        assert_eq!(expand(r"a\$b", &three), "a$b");
        assert_eq!(expand(r"\\", &three), "\\");
        assert_eq!(expand(r"\n", &three), "n");
        assert_eq!(expand(r"x\", &three), "x");
        assert_eq!(expand("${name}", &three), "${name}");
        assert_eq!(expand("$x", &three), "$x");
        assert_eq!(expand("$", &three), "$");
        assert_eq!(expand("$1$1", &three), "AA");
        let ten = [
            Some("w"),
            Some("A"),
            Some("B"),
            Some("C"),
            Some("D"),
            Some("E"),
            Some("F"),
            Some("G"),
            Some("H"),
            Some("I"),
            Some("J"),
        ];
        assert_eq!(expand("$10", &ten), "J");
        assert_eq!(expand("$1", &ten), "A");
        assert_eq!(expand("$100", &ten), "J0");
        assert_eq!(expand("$01", &ten), "A");
        let one = [Some("w"), None];
        assert_eq!(expand("[$1]", &one), "[]");
    }

    fn expand(template: &str, groups: &[Option<&str>]) -> String {
        let parsed = Template::parse(template);
        let mut out = String::new();
        parsed.expand(
            &|i| groups.get(i).copied().flatten(),
            groups.len() - 1,
            &mut out,
        );
        out
    }

    /// A word and a template hold header values: neither shows one in its
    /// `Debug` (the phase 4 / M3a plan's P2, P4).
    #[test]
    fn words_and_templates_show_nothing_in_debug() {
        assert_eq!(format!("{:?}", Template::parse("s3cr3t")), "Template(***)");
        let words = crate::value::split_words("s3cr3t");
        assert_eq!(format!("{:?}", words[0]), "Word(***)");
    }

    /// `pattern` with every match in `text` replaced by `template`.
    fn replaced(pattern: &BodyPattern, template: &str, text: &str) -> Option<String> {
        pattern.replace_all(&Template::parse(template), text)
    }

    /// A body pattern reads a line the way the manual reads it: `^` and `$`
    /// end a line at a CRLF pair, and the `\r` stays out of the line (`phase
    /// 4 M3 design 5.4`).
    #[test]
    fn body_patterns_read_lines_by_crlf() {
        let pattern = BodyPattern::new(r"^foo=(.*)$").unwrap();
        assert!(!pattern.is_fancy());
        assert_eq!(
            replaced(&pattern, "[$1]", "foo=1\r\nfoo=2\nbar"),
            Some("[1]\r\n[2]\nbar".to_string())
        );
    }

    /// What `regex` refuses is compiled with fancy-regex (phase 4 M3 design
    /// 5.4).
    #[test]
    fn a_lookahead_falls_to_fancy_regex() {
        let pattern = BodyPattern::new(r"a(?=b)").unwrap();
        assert!(pattern.is_fancy());
        assert_eq!(replaced(&pattern, "x", "ab ac"), Some("xb ac".to_string()));
    }

    /// A pattern fancy-regex compiles reads a line by `\n` alone, so the `\r`
    /// of a CRLF pair is part of the line and `.` matches it (the difference
    /// the phase 4 M3 design 5.4 registers).
    #[test]
    fn fancy_patterns_count_a_carriage_return_into_the_line() {
        let pattern = BodyPattern::new(r"^a(.)(?=\n)").unwrap();
        assert!(pattern.is_fancy());
        assert_eq!(
            replaced(&pattern, "[$1]", "a\r\n"),
            Some("[\r]\n".to_string())
        );
    }

    /// An empty match goes where each engine allows one: a `^$` matches the
    /// empty body, every empty line, and not the middle of a CRLF pair (phase
    /// 4 M3 design 5.5).
    #[test]
    fn empty_matches_insert_where_the_engine_allows() {
        let pattern = BodyPattern::new(r"^$").unwrap();
        assert!(!pattern.is_fancy());
        assert_eq!(replaced(&pattern, "new", ""), Some("new".to_string()));
        assert_eq!(
            replaced(&pattern, "new", "a\n\nb"),
            Some("a\nnew\nb".to_string())
        );
        assert_eq!(
            replaced(&pattern, "new", "a\r\n\r\nb"),
            Some("a\r\nnew\r\nb".to_string())
        );
    }

    /// A body pattern reads the ICU escapes `translate` rewrites (phase 4 M3
    /// design 5.2).
    #[test]
    fn body_patterns_translate_icu_spellings() {
        let pattern = BodyPattern::new(r"a\hb").unwrap();
        assert!(!pattern.is_fancy());
        assert_eq!(replaced(&pattern, "x", "a b"), Some("x".to_string()));
    }

    /// A pattern neither crate compiles is refused, with the reason fancy-regex
    /// gave; `translate`'s own refusal comes back as it is (phase 4 M3 design
    /// 5.4).
    #[test]
    fn body_patterns_that_compile_nowhere_are_refused() {
        let error = BodyPattern::new("(").err().unwrap();
        assert!(!error.is_empty());
        assert_eq!(
            BodyPattern::new(r"[\H]").err(),
            Some("`\\H` cannot be used inside a character class".to_string())
        );
    }

    /// A pattern that backtracks past fancy-regex's limit gives `None` rather
    /// than a panic, and `new` uses `BODY_BACKTRACK_LIMIT` rather than
    /// fancy-regex's default (phase 4 M3 design 5.4, 5.5).
    #[test]
    fn a_backtracking_body_pattern_gives_none() {
        let star = r"(a|b|ab)*(?=c)";
        let forty = "ab".repeat(40);
        let pattern = BodyPattern::with_backtrack_limit(star, 1000).unwrap();
        assert_eq!(replaced(&pattern, "x", &forty), None);

        let twenty = "ab".repeat(20);
        let small = BodyPattern::with_backtrack_limit(star, 1_000_000).unwrap();
        assert_eq!(replaced(&small, "x", &twenty), None);
        let pattern = BodyPattern::new(star).unwrap();
        assert!(replaced(&pattern, "x", &twenty).is_some());
    }

    /// A body pattern is compared and shown by its expression alone, not by
    /// which crate compiled it (the phase 4 / M3b plan's P2).
    #[test]
    fn body_patterns_compare_and_show_by_source() {
        assert_eq!(
            BodyPattern::new("a+").unwrap(),
            BodyPattern::new("a+").unwrap()
        );
        assert_eq!(
            format!("{:?}", BodyPattern::new("a+").unwrap()),
            "BodyPattern(\"a+\")"
        );
    }
}
