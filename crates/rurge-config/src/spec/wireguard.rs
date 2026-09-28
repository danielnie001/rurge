//! `wireguard` policy parameters (manual: Policies › WireGuard): the line
//! names a `[WireGuard <name>]` section and the spec carries the section's
//! contents along.

use super::common::CommonOpts;
use super::reader::ParamReader;
use super::tls::refuse_tls;
use crate::diagnostic::codes;
use crate::wireguard::WireGuardSection;

/// Common parameters that mean nothing for a WireGuard policy: the manual
/// has no interface binding for it, and `tfo` / `tos` concern a TCP
/// connection to the server. Warned about (`W0028`) and cleared.
pub const NOT_APPLICABLE: [&str; 4] = ["interface", "allow-other-interface", "tfo", "tos"];

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct WireGuardSpec {
    /// The section `section-name` names, as it is now: an edited section
    /// makes another spec, so a reload builds the policy anew.
    pub section: WireGuardSection,
}

/// Everything `wireguard`-specific on the line, and the common parameters
/// it has no use for taken out of `common`. After an error was reported the
/// returned value is meaningless: the caller checks `r.has_errors()`.
pub fn read_wireguard(
    r: &mut ParamReader<'_>,
    common: &mut CommonOpts,
    sections: &[WireGuardSection],
) -> WireGuardSpec {
    refuse_tls(r);
    for key in NOT_APPLICABLE {
        if r.has(key) {
            r.warn(
                codes::W_PARAM_NOT_APPLICABLE,
                format!("`{key}` does not apply to `wireguard` policies; ignored"),
            );
        }
    }
    common.interface = None;
    common.allow_other_interface = false;
    common.tfo = false;
    common.tos = 0;
    if common.test_url.as_deref().is_some_and(|url| {
        !url.get(..7)
            .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
    }) {
        // never echoed: a subscription line may have set it (M3-D7)
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "the `test-url` of a `wireguard` policy must be a plain http:// URL".to_string(),
        );
    }
    let Some(name) = r
        .str("section-name")
        .map(str::trim)
        .filter(|n| !n.is_empty())
    else {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`section-name` is required".to_string(),
        );
        return WireGuardSpec::default();
    };
    match sections.iter().find(|s| s.name == name) {
        Some(section) => WireGuardSpec {
            section: section.clone(),
        },
        None => {
            r.error(
                codes::E_WIREGUARD_SECTION,
                format!(
                    "`section-name` names `[WireGuard {name}]`, which does not exist or has errors"
                ),
            );
            WireGuardSpec::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Diagnostic;
    use crate::policy::parse_policy;
    use crate::span::Span;
    use crate::spec::IpVersion;
    use crate::spec::common::{Applies, Notes, read_common};
    use std::path::Path;
    use std::sync::Arc;

    fn section(name: &str) -> WireGuardSection {
        WireGuardSection {
            name: name.to_string(),
            mtu: 1280,
            ..WireGuardSection::default()
        }
    }

    fn read(def: &str) -> (WireGuardSpec, CommonOpts, bool, Vec<Diagnostic>) {
        let p = parse_policy("W", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let mut common = read_common(&mut r, Applies::Proxy, &mut Notes::default());
        let spec = read_wireguard(&mut r, &mut common, &[section("home"), section("Office")]);
        let failed = r.has_errors();
        (spec, common, failed, r.finish())
    }

    fn errors(def: &str) -> Vec<(&'static str, String)> {
        let (_, _, failed, diags) = read(def);
        assert!(failed, "{def}");
        diags.into_iter().map(|d| (d.code, d.message)).collect()
    }

    #[test]
    fn the_line_brings_its_section_along() {
        let (spec, _, failed, diags) = read("wireguard, section-name=home");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.section, section("home"));
        // the name matches exactly
        let (spec, _, failed, _) = read("wireguard, section-name = Office");
        assert!(!failed);
        assert_eq!(spec.section.name, "Office");
    }

    #[test]
    fn the_section_name_is_required_and_must_name_a_section() {
        assert_eq!(
            errors("wireguard, test-timeout=5"),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `W`: `section-name` is required".to_string()
            )]
        );
        assert_eq!(
            errors("wireguard, section-name=office"),
            [(
                codes::E_WIREGUARD_SECTION,
                "policy `W`: `section-name` names `[WireGuard office]`, which does not exist or has errors".to_string()
            )]
        );
    }

    /// Interface binding is not supported for WireGuard (manual); `tfo` and
    /// `tos` concern a TCP connection. `ip-version` still picks the family
    /// of the endpoints.
    #[test]
    fn socket_parameters_do_not_apply() {
        let (_, common, failed, diags) = read(
            "wireguard, section-name=home, interface=en0, allow-other-interface=true, tfo=true, tos=0x10, ip-version=v4-only",
        );
        assert!(!failed);
        let found: Vec<(&str, &str)> = diags.iter().map(|d| (d.code, d.message.as_str())).collect();
        assert_eq!(
            found,
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `W`: `interface` does not apply to `wireguard` policies; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `W`: `allow-other-interface` does not apply to `wireguard` policies; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `W`: `tfo` does not apply to `wireguard` policies; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `W`: `tos` does not apply to `wireguard` policies; ignored"
                ),
            ]
        );
        assert_eq!(
            (
                common.interface,
                common.allow_other_interface,
                common.tfo,
                common.tos
            ),
            (None, false, false, 0)
        );
        assert_eq!(common.ip_version, IpVersion::V4Only);
    }

    /// Only plain HTTP (manual); the URL is never quoted.
    #[test]
    fn the_test_url_is_plain_http() {
        let (_, common, failed, _) =
            read("wireguard, section-name=home, test-url=HTTP://10.0.0.1/");
        assert!(!failed);
        assert_eq!(common.test_url.as_deref(), Some("HTTP://10.0.0.1/"));
        assert_eq!(
            errors("wireguard, section-name=home, test-url=https://t.test/?token=t0k3n"),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `W`: the `test-url` of a `wireguard` policy must be a plain http:// URL"
                    .to_string()
            )]
        );
    }

    #[test]
    fn tls_parameters_do_not_apply() {
        let (_, _, failed, diags) = read("wireguard, section-name=home, sni=x.test");
        assert!(!failed);
        assert_eq!(
            (diags[0].code, diags[0].message.as_str()),
            (
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `W`: `sni` does not apply to `wireguard` policies; ignored"
            )
        );
    }
}
