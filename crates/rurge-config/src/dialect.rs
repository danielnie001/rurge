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
}
