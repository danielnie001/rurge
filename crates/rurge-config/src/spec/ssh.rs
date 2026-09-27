//! `ssh` policy parameters (manual: Policies › SSH).

use super::reader::ParamReader;
use super::secret::Secret;
use super::tls::refuse_tls;
use crate::diagnostic::codes;
use crate::keystore::{KeystoreItem, KeystoreType};
use base64::Engine as _;
use std::time::Duration;

/// `idle-timeout` when the line has none (manual: 180 seconds).
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshSpec {
    pub username: Secret<String>,
    pub password: Option<Secret<String>>,
    /// The name of an `openssh-private-key` item of `[Keystore]`.
    pub private_key: Option<String>,
    /// How long the session may have no open channel before it is closed.
    pub idle_timeout: Duration,
    /// `server-fingerprint`: the server's host key must be one of these;
    /// when there are none, any key is accepted (with a warning).
    pub host_keys: Vec<HostKeyPin>,
}

/// One entry of `server-fingerprint`: a public key the way `ssh-keyscan`
/// prints it, `<algorithm> <base64>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKeyPin {
    pub algorithm: String,
    /// The key in the SSH wire encoding (the decoded base64).
    pub blob: Vec<u8>,
}

/// Everything `ssh`-specific on the line. After an error was reported the
/// returned value is meaningless: the caller checks `r.has_errors()`.
///
/// The credentials are named-only (`username=`, `password=`), as the manual
/// writes them: a positional value stays unread and is reported as an extra
/// positional value, never quoted.
pub fn read_ssh(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> SshSpec {
    refuse_tls(r);
    let username = r.str("username").unwrap_or_default();
    if username.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`username` is required".to_string(),
        );
    }
    let password = r
        .str("password")
        .filter(|p| !p.is_empty())
        .map(Secret::from);
    let mut private_key = None;
    if let Some(v) = r.str("private-key") {
        let name = v.trim();
        match keystore.iter().find(|k| k.name == name) {
            None => r.error(
                codes::E_KEYSTORE_REF,
                format!("`private-key` references unknown keystore item `{name}`"),
            ),
            Some(item) if item.kind != KeystoreType::OpensshPrivateKey => r.error(
                codes::E_KEYSTORE_REF,
                format!("`private-key` needs an `openssh-private-key` keystore item, but `{name}` is `p12`"),
            ),
            Some(_) => private_key = Some(name.to_string()),
        }
    } else if password.is_none() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "either `password` or `private-key` is required".to_string(),
        );
    }
    let idle_timeout = match r.str("idle-timeout") {
        None => DEFAULT_IDLE_TIMEOUT,
        Some(v) => match v.trim().parse::<u64>() {
            Ok(secs) if secs > 0 => Duration::from_secs(secs),
            _ => {
                r.invalid("idle-timeout", v, "seconds, at least 1");
                DEFAULT_IDLE_TIMEOUT
            }
        },
    };
    let mut host_keys = Vec::new();
    if let Some(v) = r.str("server-fingerprint") {
        for (i, entry) in v.split(',').map(str::trim).enumerate() {
            match parse_pin(entry) {
                Ok(pin) => host_keys.push(pin),
                // never echoed: the line may come from a subscription
                Err(why) => r.error(
                    codes::E_INVALID_POLICY_PARAM,
                    format!("`server-fingerprint` entry {}: {why}", i + 1),
                ),
            }
        }
    }
    SshSpec {
        username: username.into(),
        password,
        private_key,
        idle_timeout,
        host_keys,
    }
}

fn parse_pin(entry: &str) -> Result<HostKeyPin, &'static str> {
    let mut parts = entry.split_whitespace();
    let (Some(algorithm), Some(key), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err("expected `<algorithm> <base64 key>`");
    };
    let blob = base64::engine::general_purpose::STANDARD
        .decode(key)
        .map_err(|_| "the key is not valid base64")?;
    // the encoded key starts with its algorithm name, as an SSH string
    let named = blob
        .get(..4)
        .and_then(|n| <[u8; 4]>::try_from(n).ok())
        .map(|n| u32::from_be_bytes(n) as usize)
        .and_then(|n| blob.get(4..4 + n));
    if named != Some(algorithm.as_bytes()) {
        return Err("the key is not of the named algorithm");
    }
    Ok(HostKeyPin {
        algorithm: algorithm.to_string(),
        blob,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Diagnostic;
    use crate::policy::parse_policy;
    use crate::span::Span;
    use std::path::Path;
    use std::sync::Arc;

    /// The manual's three example host keys (`policies/ssh.html`).
    const ED25519: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBk2No6KBq2m9VTCcHXXJBX4/A3RNr+L+yDBl5+TF9qz";
    const ECDSA: &str = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBLdhR3D2BvyD7FTXfx0CrjZF2tVgoVRFi1poGKoX0eXc9OlpiaqNos4niiN0GWyoT4mL724cgvaL+vHW8sTZE5A=";

    fn item(name: &str, kind: KeystoreType) -> KeystoreItem {
        KeystoreItem {
            name: name.into(),
            kind,
            base64: "QUJD".into(),
            password: None,
            unknown: Vec::new(),
            span: Span::new(Arc::from(Path::new("p.conf")), 9),
        }
    }

    fn read(def: &str) -> (SshSpec, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let keystore = [
            item("key1", KeystoreType::OpensshPrivateKey),
            item("cert1", KeystoreType::P12),
        ];
        let mut r = ParamReader::new(&p);
        let spec = read_ssh(&mut r, &keystore);
        let failed = r.has_errors();
        (spec, failed, r.finish())
    }

    fn errors(def: &str) -> Vec<(&'static str, String)> {
        let (_, failed, diags) = read(def);
        assert!(failed, "{def}");
        diags.into_iter().map(|d| (d.code, d.message)).collect()
    }

    #[test]
    fn the_manuals_examples() {
        let (spec, failed, diags) = read("ssh, 1.2.3.4, 22, username=root, password=pw");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.username.expose(), "root");
        assert_eq!(
            spec.password.as_ref().map(|p| p.expose().as_str()),
            Some("pw")
        );
        assert_eq!(spec.private_key, None);
        assert_eq!(spec.idle_timeout, DEFAULT_IDLE_TIMEOUT);
        assert!(spec.host_keys.is_empty());

        let (spec, failed, _) = read("ssh, 1.2.3.4, 22, username=root, private-key=key1");
        assert!(!failed);
        assert_eq!(spec.private_key.as_deref(), Some("key1"));
        assert_eq!(spec.password, None);

        let (spec, failed, diags) = read(&format!(
            "ssh, 1.2.3.4, 22, username=root, password=pw, idle-timeout=60, server-fingerprint=\"{ED25519},{ECDSA}\""
        ));
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.idle_timeout, Duration::from_secs(60));
        let algorithms: Vec<&str> = spec
            .host_keys
            .iter()
            .map(|k| k.algorithm.as_str())
            .collect();
        assert_eq!(algorithms, ["ssh-ed25519", "ecdsa-sha2-nistp256"]);
        assert_eq!(spec.host_keys[0].blob.len(), 51, "4 + 11 + 4 + 32 bytes");
    }

    #[test]
    fn both_credentials_are_kept() {
        let (spec, failed, _) = read("ssh, h.test, 22, username=u, password=pw, private-key=key1");
        assert!(!failed);
        assert!(spec.password.is_some() && spec.private_key.is_some());
    }

    #[test]
    fn a_username_and_one_credential_are_required() {
        assert_eq!(
            errors("ssh, h.test, 22, password=pw"),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: `username` is required".to_string()
            )]
        );
        assert_eq!(
            errors("ssh, h.test, 22, username=u, password="),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: either `password` or `private-key` is required".to_string()
            )]
        );
    }

    #[test]
    fn the_private_key_must_be_an_openssh_keystore_item() {
        assert_eq!(
            errors("ssh, h.test, 22, username=u, private-key=nope"),
            [(
                codes::E_KEYSTORE_REF,
                "policy `P`: `private-key` references unknown keystore item `nope`".to_string()
            )]
        );
        assert_eq!(
            errors("ssh, h.test, 22, username=u, private-key=cert1"),
            [(
                codes::E_KEYSTORE_REF,
                "policy `P`: `private-key` needs an `openssh-private-key` keystore item, but `cert1` is `p12`".to_string()
            )]
        );
    }

    #[test]
    fn a_bad_idle_timeout_is_an_error() {
        let found = errors("ssh, h.test, 22, username=u, password=pw, idle-timeout=0");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, codes::E_INVALID_POLICY_PARAM);
        assert!(found[0].1.contains("`idle-timeout`"), "{found:?}");
    }

    /// A malformed entry is named by its position, never quoted: the line may
    /// come from a subscription.
    #[test]
    fn a_bad_server_fingerprint_entry_is_named_by_position() {
        let found = errors(&format!(
            "ssh, h.test, 22, username=u, password=pw, server-fingerprint=\"{ED25519},ssh-ed25519,ssh-rsa !!!,ssh-rsa AAAAC3NzaC1lZDI1NTE5AAAAIBk2No6KBq2m9VTCcHXXJBX4/A3RNr+L+yDBl5+TF9qz\""
        ));
        let messages: Vec<&str> = found.iter().map(|(_, m)| m.as_str()).collect();
        assert_eq!(
            messages,
            [
                "policy `P`: `server-fingerprint` entry 2: expected `<algorithm> <base64 key>`",
                "policy `P`: `server-fingerprint` entry 3: the key is not valid base64",
                "policy `P`: `server-fingerprint` entry 4: the key is not of the named algorithm",
            ]
        );
    }

    #[test]
    fn tls_parameters_do_not_apply() {
        let (_, failed, diags) = read("ssh, h.test, 22, username=u, password=pw, sni=x.test");
        assert!(!failed);
        assert_eq!(
            (diags[0].code, diags[0].message.as_str()),
            (
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `sni` does not apply to `ssh` policies; ignored"
            )
        );
    }

    #[test]
    fn the_password_never_shows_in_debug_output() {
        let (spec, _, _) = read("ssh, h.test, 22, username=root, password=hunter2");
        let shown = format!("{spec:?}");
        assert!(
            !shown.contains("hunter2") && !shown.contains("root"),
            "{shown}"
        );
    }
}
