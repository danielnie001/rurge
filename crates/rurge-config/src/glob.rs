//! Minimal wildcard matcher: `*` (any run, crosses dots), `?` (one char), optional `[...]` classes.

use thiserror::Error;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlobOptions {
    pub case_insensitive: bool,
    pub classes: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("invalid glob pattern: {0}")]
pub struct GlobError(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Lit(char),
    Any,
    One,
    Class {
        negate: bool,
        ranges: Vec<(char, char)>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glob {
    source: String,
    toks: Vec<Tok>,
    ci: bool,
}

fn fold(c: char, ci: bool) -> char {
    if ci { c.to_ascii_lowercase() } else { c }
}

impl Glob {
    pub fn new(pattern: &str, opts: GlobOptions) -> Result<Glob, GlobError> {
        let mut toks = Vec::new();
        let chars: Vec<char> = pattern.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            match c {
                '*' => {
                    if toks.last() != Some(&Tok::Any) {
                        toks.push(Tok::Any);
                    }
                }
                '?' => toks.push(Tok::One),
                '[' if opts.classes => {
                    let mut j = i + 1;
                    let negate = j < chars.len() && (chars[j] == '!' || chars[j] == '^');
                    if negate {
                        j += 1;
                    }
                    let mut ranges = Vec::new();
                    let mut closed = false;
                    while j < chars.len() {
                        if chars[j] == ']' && !ranges.is_empty() {
                            closed = true;
                            break;
                        }
                        let lo = chars[j];
                        if j + 2 < chars.len() && chars[j + 1] == '-' && chars[j + 2] != ']' {
                            ranges.push((
                                fold(lo, opts.case_insensitive),
                                fold(chars[j + 2], opts.case_insensitive),
                            ));
                            j += 3;
                        } else {
                            let f = fold(lo, opts.case_insensitive);
                            ranges.push((f, f));
                            j += 1;
                        }
                    }
                    if !closed {
                        return Err(GlobError(format!(
                            "unterminated character class in `{pattern}`"
                        )));
                    }
                    toks.push(Tok::Class { negate, ranges });
                    i = j;
                }
                _ => toks.push(Tok::Lit(fold(c, opts.case_insensitive))),
            }
            i += 1;
        }
        Ok(Glob {
            source: pattern.to_string(),
            toks,
            ci: opts.case_insensitive,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn has_wildcards(&self) -> bool {
        self.toks.iter().any(|t| !matches!(t, Tok::Lit(_)))
    }

    fn tok_matches(tok: &Tok, c: char) -> bool {
        match tok {
            Tok::Lit(l) => *l == c,
            Tok::One => true,
            Tok::Class { negate, ranges } => {
                ranges.iter().any(|(a, b)| *a <= c && c <= *b) != *negate
            }
            Tok::Any => false,
        }
    }

    pub fn matches(&self, s: &str) -> bool {
        let input: Vec<char> = s.chars().map(|c| fold(c, self.ci)).collect();
        let (mut si, mut pi) = (0usize, 0usize);
        let mut star: Option<(usize, usize)> = None;
        while si < input.len() {
            if pi < self.toks.len() && Self::tok_matches(&self.toks[pi], input[si]) {
                si += 1;
                pi += 1;
            } else if pi < self.toks.len() && self.toks[pi] == Tok::Any {
                star = Some((pi, si));
                pi += 1;
            } else if let Some((sp, ss)) = star {
                pi = sp + 1;
                si = ss + 1;
                star = Some((sp, ss + 1));
            } else {
                return false;
            }
        }
        while pi < self.toks.len() && self.toks[pi] == Tok::Any {
            pi += 1;
        }
        pi == self.toks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ci(p: &str) -> Glob {
        Glob::new(
            p,
            GlobOptions {
                case_insensitive: true,
                classes: false,
            },
        )
        .unwrap()
    }
    fn cs(p: &str) -> Glob {
        Glob::new(
            p,
            GlobOptions {
                case_insensitive: false,
                classes: true,
            },
        )
        .unwrap()
    }

    #[test]
    fn wildcard_semantics() {
        assert!(ci("*.example.com").matches("a.b.example.com"));
        assert!(!ci("*.example.com").matches("example.com"));
        assert!(ci("*google.com").matches("bargoogle.com"));
        assert!(ci("api-*.example.com").matches("api-v2.example.com"));
        assert!(ci("cdn?.example.com").matches("cdn1.example.com"));
        assert!(!ci("cdn?.example.com").matches("cdn10.example.com"));
        assert!(ci("*").matches(""));
        assert!(ci("EXAMPLE.com").matches("example.COM"));
        assert!(!cs("Instagram*").matches("instagram 1.0"));
        assert!(cs("Instagram*").matches("Instagram 1.0"));
    }

    #[test]
    fn classes_and_errors() {
        assert!(cs("cdn[0-9].example.com").matches("cdn7.example.com"));
        assert!(!cs("cdn[!0-9].example.com").matches("cdn7.example.com"));
        assert!(cs("[abc]x").matches("bx"));
        assert!(
            Glob::new(
                "[abc",
                GlobOptions {
                    case_insensitive: false,
                    classes: true
                }
            )
            .is_err()
        );
        // classes disabled: brackets are literals
        assert!(ci("[a]").matches("[a]"));
        assert!(!ci("a").has_wildcards());
        assert!(ci("a*").has_wildcards());
    }
}
