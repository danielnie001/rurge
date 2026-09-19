//! `http` / `https` policy parameters (manual: Policies › HTTP and HTTP/2).

use super::tls::TlsOpts;

/// One piece of a header value: literal text or a random URL-safe string
/// rendered anew for every connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeaderPart {
    Literal(String),
    Random { min: usize, max: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderTemplate {
    pub name: String,
    pub value: Vec<HeaderPart>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpSpec {
    /// `Some` for `https`.
    pub tls: Option<TlsOpts>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub always_use_connect: bool,
    pub headers: Vec<HeaderTemplate>,
}

/// Longest random string a placeholder may ask for.
const MAX_RANDOM: usize = 1024;
const PLACEHOLDER: &str = "<random-string(";

fn is_token(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

fn parse_value(text: &str) -> Result<Vec<HeaderPart>, String> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(PLACEHOLDER) {
        if at > 0 {
            parts.push(HeaderPart::Literal(rest[..at].to_string()));
        }
        let after = &rest[at + PLACEHOLDER.len()..];
        let end = after
            .find(")>")
            .ok_or_else(|| "unterminated <random-string(...)>".to_string())?;
        let spec = &after[..end];
        let (min, max) = match spec.split_once('-') {
            Some((a, b)) => (a.trim().parse::<usize>(), b.trim().parse::<usize>()),
            None => (spec.trim().parse::<usize>(), spec.trim().parse::<usize>()),
        };
        let (Ok(min), Ok(max)) = (min, max) else {
            return Err(format!("invalid length in <random-string({spec})>"));
        };
        if min == 0 || min > max || max > MAX_RANDOM {
            return Err(format!(
                "length in <random-string({spec})> must be 1-{MAX_RANDOM}"
            ));
        }
        parts.push(HeaderPart::Random { min, max });
        rest = &after[end + 2..];
    }
    if !rest.is_empty() {
        parts.push(HeaderPart::Literal(rest.to_string()));
    }
    Ok(parts)
}

impl HeaderTemplate {
    /// `Name:value;Name:value`. A value may hold `<random-string(n)>` and
    /// `<random-string(min-max)>` placeholders.
    pub fn parse_list(list: &str) -> Result<Vec<HeaderTemplate>, String> {
        let mut out = Vec::new();
        for item in list.split(';').map(str::trim).filter(|i| !i.is_empty()) {
            let (name, value) = item
                .split_once(':')
                .ok_or_else(|| format!("header `{item}` has no `:`"))?;
            let name = name.trim();
            if !is_token(name) {
                return Err(format!("`{name}` is not a valid header name"));
            }
            let value = value.trim();
            if value.contains(['\r', '\n']) {
                return Err(format!("header `{name}` holds a line break"));
            }
            out.push(HeaderTemplate {
                name: name.to_string(),
                value: parse_value(value)?,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_templates() {
        let list = HeaderTemplate::parse_list(
            "X-Client:rurge; X-Pad: a<random-string(8)>b<random-string(2-5)>;Host:edge.example",
        )
        .unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(
            list[0],
            HeaderTemplate {
                name: "X-Client".into(),
                value: vec![HeaderPart::Literal("rurge".into())]
            }
        );
        assert_eq!(
            list[1].value,
            [
                HeaderPart::Literal("a".into()),
                HeaderPart::Random { min: 8, max: 8 },
                HeaderPart::Literal("b".into()),
                HeaderPart::Random { min: 2, max: 5 },
            ]
        );
        assert_eq!(list[2].name, "Host");
        assert!(HeaderTemplate::parse_list("").unwrap().is_empty());
    }

    #[test]
    fn malformed_header_templates() {
        for bad in [
            "NoColon",
            ": value",
            "Bad Name: v",
            "X: <random-string(0)>",
            "X: <random-string(5-2)>",
            "X: <random-string(9999)>",
            "X: <random-string(3",
            "X: <random-string(a)>",
        ] {
            assert!(HeaderTemplate::parse_list(bad).is_err(), "{bad}");
        }
    }
}
