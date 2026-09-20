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

pub(crate) fn is_token(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// A header value may hold HTAB but no other control character.
pub(crate) fn is_field_text(text: &str) -> bool {
    text.chars().all(|c| c == '\t' || !c.is_control())
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
            if !is_field_text(value) {
                return Err(format!("header `{name}` holds a control character"));
            }
            out.push(HeaderTemplate {
                name: name.to_string(),
                value: parse_value(value)?,
            });
        }
        Ok(out)
    }

    /// Whether the template can be written into a request head as it is: a
    /// token for a name, no control character (other than HTAB) in the
    /// value, sane random lengths. `parse_list` only ever produces valid
    /// templates; code that gets a template from anywhere else (a
    /// caller-supplied `PolicySpec`, for instance) must check before writing
    /// it to the wire.
    pub fn is_valid(&self) -> bool {
        is_token(&self.name)
            && self.value.iter().all(|part| match part {
                HeaderPart::Literal(text) => is_field_text(text),
                HeaderPart::Random { min, max } => *min >= 1 && min <= max && *max <= MAX_RANDOM,
            })
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

    #[test]
    fn every_template_parse_list_accepts_is_valid() {
        let list = HeaderTemplate::parse_list(
            "X-Client:rurge; X-Pad: a<random-string(8)>b<random-string(2-5)>;Host:edge.example",
        )
        .unwrap();
        assert!(list.iter().all(HeaderTemplate::is_valid));
    }

    #[test]
    fn is_valid_rejects_what_parse_list_can_never_produce() {
        let literal = |text: &str| vec![HeaderPart::Literal(text.to_string())];
        let bad_name = HeaderTemplate {
            name: "Bad Name".into(),
            value: literal("v"),
        };
        assert!(!bad_name.is_valid());
        let empty_name = HeaderTemplate {
            name: "".into(),
            value: literal("v"),
        };
        assert!(!empty_name.is_valid());
        let injected = HeaderTemplate {
            name: "X".into(),
            value: literal("a\r\nX-Evil: 1"),
        };
        assert!(!injected.is_valid());
        let escape = HeaderTemplate {
            name: "X".into(),
            value: literal("\u{1b}"),
        };
        assert!(!escape.is_valid());
        let backwards = HeaderTemplate {
            name: "X".into(),
            value: vec![HeaderPart::Random { min: 5, max: 2 }],
        };
        assert!(!backwards.is_valid());
        let zero = HeaderTemplate {
            name: "X".into(),
            value: vec![HeaderPart::Random { min: 0, max: 3 }],
        };
        assert!(!zero.is_valid());
        let too_long = HeaderTemplate {
            name: "X".into(),
            value: vec![HeaderPart::Random { min: 1, max: 5000 }],
        };
        assert!(!too_long.is_valid());
        let tab = HeaderTemplate {
            name: "X".into(),
            value: literal("a\tb"),
        };
        assert!(tab.is_valid());
    }

    #[test]
    fn parse_list_rejects_other_control_characters_too() {
        assert!(HeaderTemplate::parse_list("X: a\u{1b}b").is_err());
    }
}
