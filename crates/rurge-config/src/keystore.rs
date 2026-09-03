//! `[Keystore]` certificates and private keys.

use crate::diagnostic::{ParseError, codes};
use crate::span::Span;
use crate::value::{ParamMap, parse_key_value, split_list};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeystoreType {
    P12,
    OpensshPrivateKey,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeystoreItem {
    pub name: String,
    pub kind: KeystoreType,
    pub base64: String,
    pub password: Option<String>,
    pub unknown: Vec<String>,
    pub span: Span,
}

pub fn parse_keystore_item(
    name: &str,
    definition: &str,
    span: &Span,
) -> Result<KeystoreItem, ParseError> {
    let fields = split_list(definition);
    let (params, _) = ParamMap::from_fields(&fields);
    let base64 = params
        .get("base64")
        .ok_or_else(|| {
            ParseError::new(
                codes::E_SYNTAX,
                format!("keystore item `{name}`: missing `base64`"),
            )
        })?
        .to_string();
    let password = params.get("password").map(str::to_string);
    let kind = match params.get("type").map(|t| t.to_ascii_lowercase()) {
        Some(t) if t == "p12" => KeystoreType::P12,
        Some(t) if t == "openssh-private-key" => KeystoreType::OpensshPrivateKey,
        Some(t) => {
            return Err(ParseError::new(
                codes::E_INVALID_RULE_VALUE,
                format!("keystore item `{name}`: unknown type `{t}`"),
            ));
        }
        None if password.is_some() => KeystoreType::P12,
        None => KeystoreType::OpensshPrivateKey,
    };
    let unknown = fields
        .iter()
        .filter(|f| match parse_key_value(f) {
            Some((k, _)) => !matches!(
                k.to_ascii_lowercase().as_str(),
                "type" | "base64" | "password"
            ),
            None => true,
        })
        .cloned()
        .collect();
    Ok(KeystoreItem {
        name: name.to_string(),
        kind,
        base64,
        password,
        unknown,
        span: span.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;

    fn span() -> Span {
        Span::new(Arc::from(Path::new("k.conf")), 1)
    }

    #[test]
    fn items() {
        let i = parse_keystore_item("cert1", "type=p12, base64=AAAA, password=123456", &span())
            .unwrap();
        assert_eq!(i.kind, KeystoreType::P12);
        assert_eq!(i.password.as_deref(), Some("123456"));
        assert!(i.unknown.is_empty());
        let i =
            parse_keystore_item("key1", "type=openssh-private-key, base64=BBBB", &span()).unwrap();
        assert_eq!(i.kind, KeystoreType::OpensshPrivateKey);
        assert!(i.unknown.is_empty());
        let i = parse_keystore_item("cert2", "base64=CCCC, password=x", &span()).unwrap();
        assert_eq!(i.kind, KeystoreType::P12);
        assert!(i.unknown.is_empty());
        let i = parse_keystore_item("key2", "base64=DDDD", &span()).unwrap();
        assert_eq!(i.kind, KeystoreType::OpensshPrivateKey);
        assert!(i.unknown.is_empty());
        let i = parse_keystore_item(
            "k",
            "type=p12, base64=AAAA, password=x, foo=bar, stray",
            &span(),
        )
        .unwrap();
        assert_eq!(i.unknown, ["foo=bar", "stray"]);
        assert_eq!(
            parse_keystore_item("bad", "type=p12, password=x", &span())
                .unwrap_err()
                .code,
            codes::E_SYNTAX
        );
        assert_eq!(
            parse_keystore_item("bad", "type=jks, base64=x", &span())
                .unwrap_err()
                .code,
            codes::E_INVALID_RULE_VALUE
        );
    }
}
