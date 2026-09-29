//! `external` policy parameters (manual: Policies › External Proxy Program):
//! a program rurge starts itself, reached as a SOCKS5 proxy on a local port.

use super::common::CommonOpts;
use super::reader::ParamReader;
use super::secret::Secret;
use super::tls::refuse_tls;
use crate::diagnostic::codes;
use std::net::IpAddr;

/// Common parameters that mean nothing for a connection to a local port.
/// Warned about (`W0028`) and cleared.
pub const NOT_APPLICABLE: [&str; 6] = [
    "interface",
    "allow-other-interface",
    "tfo",
    "tos",
    "ip-version",
    "underlying-proxy",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalSpec {
    /// The program, as the operating system finds it.
    pub exec: String,
    /// Its arguments, in the order written. They often carry a password
    /// (`sshpass -p …`), so they never print (M4-D12).
    pub args: Secret<Vec<String>>,
    /// The port its SOCKS5 server listens on, at `127.0.0.1`.
    pub local_port: u16,
    /// Addresses to keep out of the TUN routes (phase 3).
    pub addresses: Vec<IpAddr>,
    /// `udp-relay`: the program's SOCKS5 server takes `UDP ASSOCIATE`
    /// (the manual: it must, for this to be switched on).
    pub udp_relay: bool,
}

/// Everything `external`-specific on the line, and the common parameters it
/// has no use for taken out of `common`. After an error was reported the
/// returned value is meaningless: the caller checks `r.has_errors()`.
pub fn read_external(r: &mut ParamReader<'_>, common: &mut CommonOpts) -> ExternalSpec {
    refuse_tls(r);
    for key in NOT_APPLICABLE {
        if r.has(key) {
            r.warn(
                codes::W_PARAM_NOT_APPLICABLE,
                format!("`{key}` does not apply to `external` policies; ignored"),
            );
        }
    }
    common.interface = None;
    common.allow_other_interface = false;
    common.tfo = false;
    common.tos = 0;
    common.ip_version = Default::default();
    common.underlying_proxy = None;
    let exec = r.str("exec").map(str::trim).unwrap_or_default();
    if exec.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`exec` is required".to_string(),
        );
    }
    let args = r.all("args").into_iter().map(str::to_string).collect();
    let local_port = match r.str("local-port") {
        None => {
            r.error(
                codes::E_INVALID_POLICY_PARAM,
                "`local-port` is required".to_string(),
            );
            0
        }
        Some(v) => match v.trim().parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => {
                r.invalid("local-port", v, "a port, 1-65535");
                0
            }
        },
    };
    let mut addresses = Vec::new();
    for v in r.all("addresses") {
        match v.trim().parse::<IpAddr>() {
            Ok(ip) => addresses.push(ip),
            Err(_) => r.invalid("addresses", v, "an IP address"),
        }
    }
    ExternalSpec {
        exec: exec.to_string(),
        args: Secret::new(args),
        local_port,
        addresses,
        udp_relay: r.bool("udp-relay").unwrap_or(false),
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

    fn read(def: &str) -> (ExternalSpec, CommonOpts, bool, Vec<Diagnostic>) {
        let p = parse_policy("X", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let mut common = read_common(&mut r, Applies::Proxy, &mut Notes::default());
        let spec = read_external(&mut r, &mut common);
        let failed = r.has_errors();
        (spec, common, failed, r.finish())
    }

    fn errors(def: &str) -> Vec<(&'static str, String)> {
        let (_, _, failed, diags) = read(def);
        assert!(failed, "{def}");
        diags.into_iter().map(|d| (d.code, d.message)).collect()
    }

    /// The manual's example: `args` repeat, in order.
    #[test]
    fn the_manual_example() {
        let (spec, _, failed, diags) = read(
            "external, exec = \"/usr/bin/ssh\", args = \"11.22.33.44\", args = \"-D\", args = \"127.0.0.1:1080\", local-port = 1080, addresses = 11.22.33.44",
        );
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.exec, "/usr/bin/ssh");
        assert_eq!(spec.args.expose(), &["11.22.33.44", "-D", "127.0.0.1:1080"]);
        assert_eq!(spec.local_port, 1080);
        assert_eq!(spec.addresses, ["11.22.33.44".parse::<IpAddr>().unwrap()]);
        // no argument ever prints
        assert!(!format!("{spec:?}").contains("127.0.0.1:1080"));
        assert!(format!("{spec:?}").contains("Secret(***)"));
    }

    #[test]
    fn exec_and_local_port_are_required() {
        assert_eq!(
            errors("external, local-port = 1080"),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `X`: `exec` is required".to_string()
            )]
        );
        assert_eq!(
            errors("external, exec = \"  \", local-port = 1080")[0].1,
            "policy `X`: `exec` is required"
        );
        assert_eq!(
            errors("external, exec = /bin/prog"),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `X`: `local-port` is required".to_string()
            )]
        );
        for port in ["0", "65536", "http"] {
            assert_eq!(
                errors(&format!("external, exec = /bin/prog, local-port = {port}")),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    format!(
                        "policy `X`: invalid value `{port}` for `local-port` (expected a port, 1-65535)"
                    )
                )]
            );
        }
    }

    #[test]
    fn addresses_are_ip_addresses() {
        let (spec, _, failed, _) = read(
            "external, exec = /bin/prog, local-port = 1080, addresses = 10.0.0.1, addresses = fd00::1",
        );
        assert!(!failed);
        assert_eq!(spec.addresses.len(), 2);
        assert_eq!(
            errors("external, exec = /bin/prog, local-port = 1080, addresses = vpn.test"),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `X`: invalid value `vpn.test` for `addresses` (expected an IP address)"
                    .to_string()
            )]
        );
    }

    /// A connection to a local port has no interface, no TCP options and no
    /// relay of its own.
    #[test]
    fn socket_parameters_do_not_apply() {
        let (_, common, failed, diags) = read(
            "external, exec = /bin/prog, local-port = 1080, interface = en0, allow-other-interface = true, tfo = true, tos = 0x10, ip-version = v4-only, underlying-proxy = Other",
        );
        assert!(!failed);
        let found: Vec<(&str, &str)> = diags.iter().map(|d| (d.code, d.message.as_str())).collect();
        let expected: Vec<String> = NOT_APPLICABLE
            .iter()
            .map(|key| {
                format!("policy `X`: `{key}` does not apply to `external` policies; ignored")
            })
            .collect();
        assert_eq!(
            found,
            expected
                .iter()
                .map(|m| (codes::W_PARAM_NOT_APPLICABLE, m.as_str()))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            (
                common.interface,
                common.allow_other_interface,
                common.tfo,
                common.tos,
                common.ip_version,
                common.underlying_proxy
            ),
            (None, false, false, 0, IpVersion::Dual, None)
        );
    }

    #[test]
    fn tls_parameters_do_not_apply() {
        let (_, _, failed, diags) =
            read("external, exec = /bin/prog, local-port = 1080, sni = x.test");
        assert!(!failed);
        assert_eq!(
            (diags[0].code, diags[0].message.as_str()),
            (
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `X`: `sni` does not apply to `external` policies; ignored"
            )
        );
    }
}
