//! `[Keystore]` certificates and private keys.

use crate::diagnostic::{ParseError, codes};
use crate::policy::named;
use crate::span::Span;
use crate::spec::reader::{quotable, written_as_name};
use crate::value::{ParamMap, parse_key_value, split_fields};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeystoreType {
    P12,
    OpensshPrivateKey,
}

#[derive(Clone, PartialEq, Eq)]
pub struct KeystoreItem {
    pub name: String,
    pub kind: KeystoreType,
    pub base64: String,
    pub password: Option<String>,
    pub unknown: Vec<String>,
    pub span: Span,
}

/// Without the key material and its password, and without the fields nobody
/// reads — one of them can be a password written under a name of its own
/// (`passphrase=…`): only how many there are.
impl std::fmt::Debug for KeystoreItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let KeystoreItem {
            name,
            kind,
            base64: _,
            password,
            unknown,
            span,
        } = self;
        f.debug_struct("KeystoreItem")
            .field("name", name)
            .field("kind", kind)
            .field("base64", &"***")
            .field("password", &password.as_ref().map(|_| "***"))
            .field("unknown", &unknown.len())
            .field("span", span)
            .finish()
    }
}

/// What a diagnostic says of a field of the item `definition` that nobody
/// reads: its name when that can be one, never its value (a password under
/// the name of another field, `passphrase=…`) and never a field without a
/// name.
pub(crate) fn unknown_field(definition: &str, field: &str) -> String {
    match parse_key_value(field) {
        Some((name, _)) if written_as_name(definition, name) => {
            format!("unknown [Keystore] field `{name}` ignored")
        }
        _ => "unknown [Keystore] field ignored".to_string(),
    }
}

pub fn parse_keystore_item(
    name: &str,
    definition: &str,
    span: &Span,
) -> Result<KeystoreItem, ParseError> {
    let fields = split_fields(definition);
    let (params, _) = ParamMap::from_fields(&fields);
    let base64 = params
        .get("base64")
        .ok_or_else(|| {
            ParseError::new(
                codes::E_SYNTAX,
                format!("{}: missing `base64`", named("keystore item", name)),
            )
        })?
        .to_string();
    let password = params.get("password").map(str::to_string);
    let kind = match params.get("type").map(|t| t.to_ascii_lowercase()) {
        Some(t) if t == "p12" => KeystoreType::P12,
        Some(t) if t == "openssh-private-key" => KeystoreType::OpensshPrivateKey,
        Some(t) => {
            // a comma missing after `type=p12` lets the next field slide in
            let shown = if quotable(&t) {
                format!(" `{t}`")
            } else {
                String::new()
            };
            return Err(ParseError::new(
                codes::E_INVALID_RULE_VALUE,
                format!("{}: unknown type{shown}", named("keystore item", name)),
            ));
        }
        None if password.is_some() => KeystoreType::P12,
        None => KeystoreType::OpensshPrivateKey,
    };
    let unknown = fields
        .iter()
        .filter(|field| match &field.param {
            Some((key, _)) => !matches!(
                key.to_ascii_lowercase().as_str(),
                "type" | "base64" | "password"
            ),
            None => true,
        })
        .map(|field| field.text.clone())
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

    /// A field nobody reads is named, never quoted with its value: it can be
    /// a password under the name of another field (`passphrase=…`, mixed up
    /// with `[MITM]`'s). A field without a name has none to say, and a name is
    /// one only as the item writes it.
    #[test]
    fn an_unknown_field_is_named_and_never_quoted_with_its_value() {
        let longest = "a".repeat(32);
        let too_long = "a".repeat(33);
        let said = |extra: &str| -> Vec<String> {
            let definition = format!("type=p12, base64=AAAA, {extra}");
            let item = parse_keystore_item("k", &definition, &span()).unwrap();
            item.unknown
                .iter()
                .map(|field| unknown_field(&definition, field))
                .collect()
        };
        let named = |name: &str| vec![format!("unknown [Keystore] field `{name}` ignored")];
        let nameless = || vec!["unknown [Keystore] field ignored".to_string()];
        assert_eq!(said("foo=bar"), named("foo"));
        assert_eq!(said("passphrase=hunter2"), named("passphrase"));
        assert_eq!(said("passphrase = hunter2 "), named("passphrase"));
        assert_eq!(said(&format!("{longest}=x")), named(&longest));
        assert_eq!(said(&format!("{too_long}=x")), nameless());
        // no name to say: a bare value, upper case, a whole item in quotes
        assert_eq!(said("hunter2"), nameless());
        assert_eq!(said("Passphrase=hunter2"), nameless());
        assert_eq!(said("\"passphrase=hunter2\""), nameless());
        assert_eq!(said("aHVudGVyMg=="), nameless());
        // two of them
        assert_eq!(
            said("foo=bar, hunter2"),
            [
                "unknown [Keystore] field `foo` ignored",
                "unknown [Keystore] field ignored"
            ]
        );
    }

    /// The type that is none is quoted when it can be what somebody wrote
    /// for `type`; a comma missing after `type=p12` lets the next field slide in.
    #[test]
    fn an_unknown_type_is_quoted_only_when_it_can_be_one() {
        let longest = "a".repeat(64);
        let too_long = "a".repeat(65);
        let said = |definition: &str| {
            parse_keystore_item("k", definition, &span())
                .unwrap_err()
                .message
        };
        assert_eq!(
            said("type=jks, base64=AAAA"),
            "keystore item `k`: unknown type `jks`"
        );
        assert_eq!(
            said(&format!("type={longest}, base64=AAAA")),
            format!("keystore item `k`: unknown type `{longest}`")
        );
        for definition in [
            format!("type={too_long}, base64=AAAA"),
            "type=p12 password=hunter2, base64=AAAA".to_string(),
            "type=p12\u{3000}hunter2, base64=AAAA".to_string(),
            "type=a=b, base64=AAAA".to_string(),
        ] {
            assert_eq!(
                said(&definition),
                "keystore item `k`: unknown type",
                "{definition}"
            );
        }
    }

    /// An item is named by what stands in front of the `=` of its line, and
    /// that is a password when the line is a share link or has its fields
    /// joined by an input method's `，` or `＝`: the name is told when it is one.
    #[test]
    fn an_item_is_named_only_when_the_name_is_one() {
        let said = |name: &str, definition: &str| {
            parse_keystore_item(name, definition, &span())
                .unwrap_err()
                .message
        };
        assert_eq!(
            said("cert 1", "type=p12, password=x"),
            "keystore item `cert 1`: missing `base64`"
        );
        assert_eq!(
            said("cert 1", "type=jks, base64=AAAA"),
            "keystore item `cert 1`: unknown type `jks`"
        );
        for name in [
            "trojan://hunter2@t.example:443?sni",
            "k\u{ff0c}hunter2",
            "k\u{ff1d}hunter2",
        ] {
            assert_eq!(
                said(name, "type=p12, password=x"),
                "keystore item: missing `base64`",
                "{name}"
            );
            assert_eq!(
                said(name, "type=jks, base64=AAAA"),
                "keystore item: unknown type `jks`",
                "{name}"
            );
            assert_eq!(
                said(name, "type=p12 password=hunter2, base64=AAAA"),
                "keystore item: unknown type",
                "{name}"
            );
        }
    }

    /// The item shows how many fields nobody reads, not what they are.
    #[test]
    fn the_debug_of_an_item_counts_the_fields_nobody_reads() {
        let item = parse_keystore_item(
            "k",
            "type=p12, base64=AAAA, password=hunter2, passphrase=hunter3, stray",
            &span(),
        )
        .unwrap();
        let shown = format!("{item:?}");
        assert!(!shown.contains("hunter"), "{shown}");
        assert!(shown.contains("unknown: 2"), "{shown}");
    }
}
