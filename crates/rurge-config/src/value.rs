//! Value-level helpers shared by all section parsers.

/// Split a comma-separated list at top level, honouring quotes and parentheses.
/// Items are trimmed and unquoted; empty items are dropped.
pub fn split_list(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == '\\' && q == '"' {
                    if let Some(n) = chars.next() {
                        cur.push(n);
                    }
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    cur.push(c);
                }
                '(' => {
                    depth += 1;
                    cur.push(c);
                }
                ')' => {
                    depth = depth.saturating_sub(1);
                    cur.push(c);
                }
                ',' if depth == 0 => {
                    out.push(std::mem::take(&mut cur));
                }
                _ => cur.push(c),
            },
        }
    }
    out.push(cur);
    out.into_iter()
        .map(|f| unquote_field(f.trim()))
        .filter(|f| !f.is_empty())
        .collect()
}

/// Strip surrounding quotes. Double quotes support `\"` and `\\` escapes.
pub fn unquote(s: &str) -> String {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        let inner = &s[1..s.len() - 1];
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
    } else if s.len() >= 2 && s.starts_with('\'') && s.ends_with('\'') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Unquote a `split_list` field. Plain fields are unquoted as-is; a `key="value"`
/// or `key='value'` field has only its value unquoted, so the key survives verbatim.
fn unquote_field(f: &str) -> String {
    match f.split_once('=') {
        Some((k, v))
            if !k.trim().is_empty()
                && (v.trim().starts_with('"') || v.trim().starts_with('\'')) =>
        {
            format!("{}={}", k.trim(), unquote(v.trim()))
        }
        _ => unquote(f),
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
