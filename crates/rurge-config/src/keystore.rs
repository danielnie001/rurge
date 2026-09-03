//! `[Keystore]` certificates and private keys.

use crate::diagnostic::{ParseError, codes};
use crate::span::Span;
use crate::value::{ParamMap, split_list};

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
    pub span: Span,
}

pub fn parse_keystore_item(
    name: &str,
    definition: &str,
    span: &Span,
) -> Result<KeystoreItem, ParseError> {
    let (params, _) = ParamMap::from_fields(&split_list(definition));
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
    Ok(KeystoreItem {
        name: name.to_string(),
        kind,
        base64,
        password,
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
        let i =
            parse_keystore_item("key1", "type=openssh-private-key, base64=BBBB", &span()).unwrap();
        assert_eq!(i.kind, KeystoreType::OpensshPrivateKey);
        let i = parse_keystore_item("cert2", "base64=CCCC, password=x", &span()).unwrap();
        assert_eq!(i.kind, KeystoreType::P12);
        let i = parse_keystore_item("key2", "base64=DDDD", &span()).unwrap();
        assert_eq!(i.kind, KeystoreType::OpensshPrivateKey);
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
