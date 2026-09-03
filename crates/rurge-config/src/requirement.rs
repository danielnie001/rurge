//! Line requirement expressions (`#!REQUIREMENT`, `#!IOS-ONLY`, ...).

use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::glob::{Glob, GlobOptions};
use crate::text::Profile;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
#[error("{0}")]
pub struct ReqError(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Environment {
    pub core_version: u64,
    pub system: String,
    pub system_version: String,
    pub device_model: String,
    pub language: String,
    pub device_name: String,
}

impl Environment {
    /// Deterministic environment for tests and snapshots.
    pub fn fixed() -> Self {
        Self {
            core_version: 20,
            system: "macOS".into(),
            system_version: "14.5".into(),
            device_model: "Mac15,6".into(),
            language: "zh-CN".into(),
            device_name: "test-mac".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Var {
    CoreVersion,
    System,
    SystemVersion,
    DeviceModel,
    Language,
    DeviceName,
}

impl Var {
    fn parse(ident: &str) -> Option<Var> {
        Some(match ident.to_ascii_uppercase().as_str() {
            "CORE_VERSION" => Var::CoreVersion,
            "SYSTEM" => Var::System,
            "SYSTEM_VERSION" => Var::SystemVersion,
            "DEVICE_MODEL" => Var::DeviceModel,
            "LANGUAGE" => Var::Language,
            "DEVICE_NAME" => Var::DeviceName,
            _ => return None,
        })
    }
    fn value(&self, env: &Environment) -> Value {
        match self {
            Var::CoreVersion => Value::Int(env.core_version as i64),
            Var::System => Value::Str(env.system.clone()),
            Var::SystemVersion => Value::Str(env.system_version.clone()),
            Var::DeviceModel => Value::Str(env.device_model.clone()),
            Var::Language => Value::Str(env.language.clone()),
            Var::DeviceName => Value::Str(env.device_name.clone()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Str(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrOp {
    BeginsWith,
    Contains,
    EndsWith,
    Like,
    Matches,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Cmp(Var, CmpOp, Value),
    Str(Var, StrOp, String),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Num(i64),
    Str(String),
    Op(&'static str),
    LParen,
    RParen,
}

const OPS: [&str; 13] = [
    "==", "!=", "<>", ">=", "=>", "<=", "=<", "&&", "||", "=", ">", "<", "!",
];

fn tokenize(src: &str) -> Result<Vec<Tok>, ReqError> {
    let chars: Vec<char> = src.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '(' {
            toks.push(Tok::LParen);
            i += 1;
        } else if c == ')' {
            toks.push(Tok::RParen);
            i += 1;
        } else if c == '\'' || c == '"' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != c {
                j += 1;
            }
            if j >= chars.len() {
                return Err(ReqError("unterminated string".into()));
            }
            toks.push(Tok::Str(chars[i + 1..j].iter().collect()));
            i = j + 1;
        } else if c.is_ascii_digit() {
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            let n: String = chars[i..j].iter().collect();
            toks.push(Tok::Num(
                n.parse()
                    .map_err(|_| ReqError(format!("bad number `{n}`")))?,
            ));
            i = j;
        } else if c.is_alphabetic() || c == '_' {
            let mut j = i;
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            toks.push(Tok::Ident(chars[i..j].iter().collect()));
            i = j;
        } else {
            let rest: String = chars[i..].iter().take(2).collect();
            let op = OPS
                .iter()
                .find(|op| rest.starts_with(*op))
                .ok_or_else(|| ReqError(format!("unexpected `{c}`")))?;
            toks.push(Tok::Op(op));
            i += op.len();
        }
    }
    Ok(toks)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn peek_keyword(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(i)) if i.eq_ignore_ascii_case(kw))
    }
    fn or(&mut self) -> Result<Expr, ReqError> {
        let mut left = self.and()?;
        while self.peek() == Some(&Tok::Op("||")) || self.peek_keyword("OR") {
            self.next();
            let right = self.and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn and(&mut self) -> Result<Expr, ReqError> {
        let mut left = self.not()?;
        while self.peek() == Some(&Tok::Op("&&")) || self.peek_keyword("AND") {
            self.next();
            let right = self.not()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn not(&mut self) -> Result<Expr, ReqError> {
        if self.peek() == Some(&Tok::Op("!")) || self.peek_keyword("NOT") {
            self.next();
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.primary()
    }
    fn primary(&mut self) -> Result<Expr, ReqError> {
        match self.next() {
            Some(Tok::LParen) => {
                let e = self.or()?;
                if self.next() != Some(Tok::RParen) {
                    return Err(ReqError("expected `)`".into()));
                }
                Ok(e)
            }
            Some(Tok::Ident(name)) => {
                let var = Var::parse(&name)
                    .ok_or_else(|| ReqError(format!("unknown variable `{name}`")))?;
                match self.next() {
                    Some(Tok::Op(op)) => {
                        let op = match op {
                            "==" | "=" => CmpOp::Eq,
                            "!=" | "<>" => CmpOp::Ne,
                            ">" => CmpOp::Gt,
                            ">=" | "=>" => CmpOp::Ge,
                            "<" => CmpOp::Lt,
                            "<=" | "=<" => CmpOp::Le,
                            other => {
                                return Err(ReqError(format!("unexpected operator `{other}`")));
                            }
                        };
                        let value = match self.next() {
                            Some(Tok::Num(n)) => Value::Int(n),
                            Some(Tok::Str(s)) | Some(Tok::Ident(s)) => Value::Str(s),
                            _ => {
                                return Err(ReqError(
                                    "expected a value after comparison operator".into(),
                                ));
                            }
                        };
                        Ok(Expr::Cmp(var, op, value))
                    }
                    Some(Tok::Ident(kw)) => {
                        let op = match kw.to_ascii_uppercase().as_str() {
                            "BEGINSWITH" => StrOp::BeginsWith,
                            "CONTAINS" => StrOp::Contains,
                            "ENDSWITH" => StrOp::EndsWith,
                            "LIKE" => StrOp::Like,
                            "MATCHES" => StrOp::Matches,
                            other => return Err(ReqError(format!("unknown operator `{other}`"))),
                        };
                        match self.next() {
                            Some(Tok::Str(s)) | Some(Tok::Ident(s)) => Ok(Expr::Str(var, op, s)),
                            Some(Tok::Num(n)) => Ok(Expr::Str(var, op, n.to_string())),
                            _ => Err(ReqError("expected a string after string operator".into())),
                        }
                    }
                    _ => Err(ReqError(format!("expected an operator after `{name}`"))),
                }
            }
            Some(t) => Err(ReqError(format!("unexpected token {t:?}"))),
            None => Err(ReqError("unexpected end of expression".into())),
        }
    }
}

pub fn parse(src: &str) -> Result<Expr, ReqError> {
    let toks = tokenize(src.trim())?;
    if toks.is_empty() {
        return Err(ReqError("empty expression".into()));
    }
    let mut p = Parser { toks, pos: 0 };
    let e = p.or()?;
    if p.pos != p.toks.len() {
        return Err(ReqError("trailing tokens in expression".into()));
    }
    Ok(e)
}

fn value_str(v: &Value) -> String {
    match v {
        Value::Int(n) => n.to_string(),
        Value::Str(s) => s.clone(),
    }
}

// Note: `eval` here is a pure tree-walking interpreter over the closed `Expr` AST defined
// above (comparison/string/boolean operators only) — not a dynamic/arbitrary code evaluator.
pub fn eval(expr: &Expr, env: &Environment) -> bool {
    match expr {
        Expr::And(a, b) => eval(a, env) && eval(b, env),
        Expr::Or(a, b) => eval(a, env) || eval(b, env),
        Expr::Not(a) => !eval(a, env),
        Expr::Cmp(var, op, rhs) => {
            let lhs = var.value(env);
            let ord = match (&lhs, rhs) {
                (Value::Int(a), Value::Int(b)) => a.cmp(b),
                (Value::Int(a), Value::Str(b)) => match b.trim().parse::<i64>() {
                    Ok(b) => a.cmp(&b),
                    Err(_) => return false,
                },
                (Value::Str(a), _) => a.as_str().cmp(value_str(rhs).as_str()),
            };
            match op {
                CmpOp::Eq => ord.is_eq(),
                CmpOp::Ne => ord.is_ne(),
                CmpOp::Gt => ord.is_gt(),
                CmpOp::Ge => ord.is_ge(),
                CmpOp::Lt => ord.is_lt(),
                CmpOp::Le => ord.is_le(),
            }
        }
        Expr::Str(var, op, rhs) => {
            let lhs = value_str(&var.value(env));
            match op {
                StrOp::BeginsWith => lhs.starts_with(rhs.as_str()),
                StrOp::Contains => lhs.contains(rhs.as_str()),
                StrOp::EndsWith => lhs.ends_with(rhs.as_str()),
                StrOp::Like => Glob::new(
                    rhs,
                    GlobOptions {
                        case_insensitive: false,
                        classes: false,
                    },
                )
                .map(|g| g.matches(&lhs))
                .unwrap_or(false),
                StrOp::Matches => fancy_regex::Regex::new(rhs)
                    .map(|re| re.is_match(&lhs).unwrap_or(false))
                    .unwrap_or(false),
            }
        }
    }
}

const SIMPLIFIED: [(&str, &str); 3] = [
    ("IOS-ONLY", "SYSTEM == 'iOS'"),
    ("MACOS-ONLY", "SYSTEM == 'macOS'"),
    ("TVOS-ONLY", "SYSTEM == 'tvOS'"),
];

/// Take the expression at the start of `rest`: a quoted string, or the first whitespace-delimited token.
fn take_expr(rest: &str) -> Result<(String, &str), ReqError> {
    let rest = rest.trim_start();
    if let Some(inner) = rest.strip_prefix('"') {
        let end = inner
            .find('"')
            .ok_or_else(|| ReqError("unterminated quoted requirement".into()))?;
        return Ok((inner[..end].to_string(), &inner[end + 1..]));
    }
    if rest.is_empty() {
        return Err(ReqError("missing requirement expression".into()));
    }
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    Ok((rest[..end].to_string(), &rest[end..]))
}

/// Find ` <marker>` (preceded by whitespace) searching from the end.
fn find_suffix_marker(line: &str, marker: &str) -> Option<usize> {
    let pos = line.rfind(marker)?;
    let before = &line[..pos];
    (before.ends_with(char::is_whitespace)).then_some(pos)
}

/// Split a profile line into (requirement expression source, content).
pub fn split_line(line: &str) -> Result<(Option<String>, String), ReqError> {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix("#!REQUIREMENT") {
        let (expr, content) = take_expr(rest)?;
        return Ok((Some(expr), content.trim().to_string()));
    }
    for (tag, expr) in SIMPLIFIED {
        let prefix = format!("#!{tag}");
        if let Some(rest) = line.strip_prefix(&prefix) {
            if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                return Ok((Some(expr.to_string()), rest.trim().to_string()));
            }
        }
    }
    for marker in ["#!REQUIREMENT", "//!REQUIREMENT"] {
        if let Some(pos) = find_suffix_marker(line, marker) {
            let expr = line[pos + marker.len()..].trim();
            if expr.is_empty() {
                return Err(ReqError("missing requirement expression".into()));
            }
            let expr = expr
                .strip_prefix('"')
                .and_then(|e| e.strip_suffix('"'))
                .unwrap_or(expr);
            return Ok((Some(expr.to_string()), line[..pos].trim().to_string()));
        }
    }
    for (tag, expr) in SIMPLIFIED {
        for prefix in ["#!", "//!"] {
            let marker = format!("{prefix}{tag}");
            if let Some(body) = line.strip_suffix(&marker) {
                if body.ends_with(char::is_whitespace) {
                    return Ok((Some(expr.to_string()), body.trim().to_string()));
                }
            }
        }
    }
    Ok((None, line.to_string()))
}

/// Evaluate every entry's requirement; unmet or invalid lines are disabled.
pub fn apply(profile: &mut Profile, env: &Environment, diags: &mut Diagnostics) {
    for section in &mut profile.sections {
        for entry in &mut section.entries {
            let Some(src) = &entry.requirement else {
                continue;
            };
            match parse(src) {
                Ok(expr) => {
                    if !eval(&expr, env) {
                        entry.disabled = true;
                        diags.push(
                            Diagnostic::info(
                                codes::I_LINE_DISABLED,
                                format!("line disabled: requirement `{src}` not met"),
                            )
                            .at(entry.span.clone()),
                        );
                    }
                }
                Err(e) => {
                    entry.disabled = true;
                    diags.push(
                        Diagnostic::error(
                            codes::E_REQUIREMENT_SYNTAX,
                            format!("invalid requirement `{src}`: {e}"),
                        )
                        .at(entry.span.clone()),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Environment {
        Environment::fixed()
    }

    #[test]
    fn manual_examples_evaluate() {
        let cases: &[(&str, bool)] = &[
            ("CORE_VERSION>=22", false),
            ("CORE_VERSION<22", true),
            ("CORE_VERSION>=20", true),
            ("SYSTEM=='macOS'", true),
            ("SYSTEM = 'iOS'", false),
            ("SYSTEM<>'iOS'", true),
            ("SYSTEM!='macOS'", false),
            ("CORE_VERSION>=22 AND SYSTEM=='iOS'", false),
            (
                "CORE_VERSION>=20 && (SYSTEM = 'iOS' || SYSTEM = 'macOS')",
                true,
            ),
            ("NOT SYSTEM=='iOS'", true),
            ("!(SYSTEM=='macOS')", false),
            ("DEVICE_NAME CONTAINS 'mac'", true),
            ("LANGUAGE BEGINSWITH 'zh'", true),
            ("LANGUAGE ENDSWITH 'US'", false),
            ("DEVICE_MODEL LIKE 'Mac1?,*'", true),
            ("SYSTEM_VERSION MATCHES '^14\\.'", true),
            ("CORE_VERSION=>20", true),
            ("CORE_VERSION=<19", false),
            ("system == 'macOS'", true),
            ("CORE_VERSION >= 6008000", false),
        ];
        for (src, expected) in cases {
            let expr = parse(src).unwrap_or_else(|e| panic!("{src}: {e}"));
            assert_eq!(eval(&expr, &env()), *expected, "expr: {src}");
        }
    }

    #[test]
    fn syntax_errors() {
        for src in [
            "",
            "CORE_VERSION >=",
            "UNKNOWN_VAR == 1",
            "SYSTEM == ",
            "(SYSTEM == 'a'",
            "CORE_VERSION ?? 1",
        ] {
            assert!(parse(src).is_err(), "should fail: {src:?}");
        }
    }

    #[test]
    fn split_line_forms() {
        let ok = |l: &str| split_line(l).unwrap();
        assert_eq!(
            ok("#!REQUIREMENT CORE_VERSION>=22 Group = smart, a, b"),
            (
                Some("CORE_VERSION>=22".into()),
                "Group = smart, a, b".into()
            )
        );
        assert_eq!(
            ok("Group = url-test, a, b //!REQUIREMENT CORE_VERSION<22"),
            (
                Some("CORE_VERSION<22".into()),
                "Group = url-test, a, b".into()
            )
        );
        assert_eq!(
            ok("Group = url-test, a #!REQUIREMENT CORE_VERSION<22"),
            (Some("CORE_VERSION<22".into()), "Group = url-test, a".into())
        );
        assert_eq!(
            ok("#!REQUIREMENT \"CORE_VERSION>=22 AND SYSTEM=='iOS'\" Group = smart, a"),
            (
                Some("CORE_VERSION>=22 AND SYSTEM=='iOS'".into()),
                "Group = smart, a".into()
            )
        );
        assert_eq!(
            ok("#!REQUIREMENT SYSTEM=='macOS'"),
            (Some("SYSTEM=='macOS'".into()), String::new())
        );
        assert_eq!(
            ok("DOMAIN,reject.com,REJECT #!MACOS-ONLY"),
            (
                Some("SYSTEM == 'macOS'".into()),
                "DOMAIN,reject.com,REJECT".into()
            )
        );
        assert_eq!(
            ok("#!IOS-ONLY DOMAIN,a,REJECT"),
            (Some("SYSTEM == 'iOS'".into()), "DOMAIN,a,REJECT".into())
        );
        assert_eq!(
            ok("DOMAIN,a,REJECT //!TVOS-ONLY"),
            (Some("SYSTEM == 'tvOS'".into()), "DOMAIN,a,REJECT".into())
        );
        assert_eq!(ok("plain line"), (None, "plain line".into()));
        assert!(split_line("x #!REQUIREMENT").is_err());
    }

    #[test]
    fn apply_disables_unmet_lines() {
        use crate::text::{Origin, parse_str};
        use std::path::Path;
        use std::sync::Arc;
        let text = "[Rule]\n#!REQUIREMENT CORE_VERSION>=22 DOMAIN,a,REJECT\nDOMAIN,b,REJECT #!MACOS-ONLY\nDOMAIN,c,REJECT #!REQUIREMENT SYSTEM ??\nFINAL,DIRECT\n";
        let (mut p, mut d) = parse_str(text, Arc::from(Path::new("t.conf")), Origin::Main);
        apply(&mut p, &env(), &mut d);
        let r = p.section("Rule").unwrap();
        assert!(r.entries[0].disabled);
        assert!(!r.entries[1].disabled);
        assert!(r.entries[2].disabled);
        assert!(!r.entries[3].disabled);
        let codes: Vec<_> = d.iter().map(|x| x.code).collect();
        assert!(codes.contains(&codes::I_LINE_DISABLED));
        assert!(codes.contains(&codes::E_REQUIREMENT_SYNTAX));
    }
}
