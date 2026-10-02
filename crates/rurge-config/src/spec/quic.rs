//! What the QUIC-based protocols share on their lines (phase 2 M7 design
//! 3.1): `port-hopping` and `port-hopping-interval`. Of the design's other
//! two, `alpn` is a TLS parameter (`TlsOpts`, `h3` when the line has none)
//! and `ecn` a common one (`CommonOpts`).

use super::common::CommonOpts;
use super::reader::ParamReader;
use super::tls::{TlsOpts, read_tls};
use crate::diagnostic::codes;
use crate::keystore::KeystoreItem;
use std::ops::RangeInclusive;
use std::time::Duration;

/// `port-hopping-interval` when the line has none (manual).
pub const DEFAULT_HOP_INTERVAL: Duration = Duration::from_secs(30);

/// The shortest `port-hopping-interval`: the reference implementation
/// refuses anything shorter; rurge raises it to this.
pub const MIN_HOP_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuicOpts {
    /// `port-hopping`: the server's ports to rotate among; the line's own
    /// port is then not used (manual).
    pub port_hopping: Option<PortHopping>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortHopping {
    /// The ports and inclusive ranges in the order written; a single port is
    /// a range of one.
    pub ports: Vec<RangeInclusive<u16>>,
    /// `port-hopping-interval`: how long one port is used, at least
    /// `MIN_HOP_INTERVAL`.
    pub interval: Duration,
}

/// The TLS options of a QUIC-based protocol: `alpn` is `h3` when the line
/// has none (manual: Policies › TLS).
pub(crate) fn read_quic_tls(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> TlsOpts {
    let mut tls = read_tls(r, keystore);
    if tls.alpn.is_empty() {
        tls.alpn = vec!["h3".to_string()];
    }
    tls
}

/// The QUIC options on the line, and `tfo` taken out of `common`: a QUIC
/// protocol opens no TCP connection, so it does not apply (`W0028`). After
/// an error was reported the returned value is meaningless: the caller
/// checks `r.has_errors()`.
pub fn read_quic(r: &mut ParamReader<'_>, common: &mut CommonOpts) -> QuicOpts {
    if r.has("tfo") {
        let kind = r.policy().kind.keyword();
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            format!("`tfo` does not apply to `{kind}` policies; ignored"),
        );
    }
    common.tfo = false;
    let Some(v) = r.str("port-hopping") else {
        if r.has("port-hopping-interval") {
            r.touch("port-hopping-interval");
            r.warn(
                codes::W_PARAM_NOT_APPLICABLE,
                "`port-hopping-interval` has no effect without `port-hopping`; ignored".to_string(),
            );
        }
        return QuicOpts::default();
    };
    let ports = match parse_ports(v) {
        Some(ports) => ports,
        None => {
            r.invalid(
                "port-hopping",
                v,
                "ports from 1 to 65535 and ranges such as `5000-6000`, separated by `;`",
            );
            Vec::new()
        }
    };
    let interval = read_interval(r);
    QuicOpts {
        port_hopping: Some(PortHopping { ports, interval }),
    }
}

/// `1234;5000-6000`: ports and inclusive ranges, separated by `;`. `None`
/// when an item is empty, is not a port from 1 to 65535, or is a range that
/// runs backwards.
fn parse_ports(value: &str) -> Option<Vec<RangeInclusive<u16>>> {
    let port = |s: &str| s.trim().parse::<u16>().ok().filter(|p| *p != 0);
    value
        .split(';')
        .map(|item| match item.split_once('-') {
            Some((first, last)) => {
                let (first, last) = (port(first)?, port(last)?);
                (first <= last).then_some(first..=last)
            }
            None => port(item).map(|p| p..=p),
        })
        .collect()
}

/// `port-hopping-interval` in whole seconds; a shorter one than
/// `MIN_HOP_INTERVAL` is raised to it with a warning (`W0012`).
fn read_interval(r: &mut ParamReader<'_>) -> Duration {
    let Some(v) = r.str("port-hopping-interval") else {
        return DEFAULT_HOP_INTERVAL;
    };
    let Ok(secs) = v.trim().parse::<u32>() else {
        r.invalid("port-hopping-interval", v, "a whole number of seconds");
        return DEFAULT_HOP_INTERVAL;
    };
    let interval = Duration::from_secs(secs.into());
    if interval < MIN_HOP_INTERVAL {
        let min = MIN_HOP_INTERVAL.as_secs();
        r.warn(
            codes::W_INVALID_VALUE,
            format!("`port-hopping-interval` is below the minimum of {min} seconds; using {min}"),
        );
        return MIN_HOP_INTERVAL;
    }
    interval
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use crate::spec::common::{Applies, Notes, read_common};
    use std::path::Path;
    use std::sync::Arc;

    fn read(def: &str) -> (QuicOpts, CommonOpts, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let mut common = read_common(&mut r, Applies::Proxy, &mut Notes::default());
        // what the protocol reads for itself
        r.touch("password");
        let quic = read_quic(&mut r, &mut common);
        let failed = r.has_errors();
        (quic, common, failed, r.finish())
    }

    fn messages(diags: &[Diagnostic]) -> Vec<(&str, &str)> {
        diags.iter().map(|d| (d.code, d.message.as_str())).collect()
    }

    fn hopping(ports: Vec<RangeInclusive<u16>>, secs: u64) -> Option<PortHopping> {
        Some(PortHopping {
            ports,
            interval: Duration::from_secs(secs),
        })
    }

    #[test]
    fn the_manuals_port_list_and_the_default_interval() {
        let (quic, _, failed, diags) =
            read("hysteria2, h.test, 443, password=p, port-hopping=1234;5000-6000");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(
            quic.port_hopping,
            hopping(vec![1234..=1234, 5000..=6000], 30)
        );
        // blanks around the items, a range of one, the order as written
        let (quic, _, failed, diags) = read(
            "hysteria2, h.test, 443, password=p, port-hopping=\" 20000 - 50000 ; 8443-8443;443 \"",
        );
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(
            quic.port_hopping,
            hopping(vec![20000..=50000, 8443..=8443, 443..=443], 30)
        );
        // nothing written, no hopping
        let (quic, _, failed, diags) = read("hysteria2, h.test, 443, password=p");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(quic, QuicOpts::default());
    }

    #[test]
    fn a_bad_port_list_is_an_error_that_quotes_it() {
        for bad in [
            "",
            "0",
            "65536",
            "6000-5000",
            "5000-",
            "-6000",
            "1-2-3",
            "1234;",
            "1234;;5000",
            "443,8443",
            "https",
        ] {
            let (_, _, failed, diags) = read(&format!(
                "hysteria2, h.test, 443, password=p, port-hopping=\"{bad}\""
            ));
            assert!(failed, "{bad}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    format!(
                        "policy `P`: invalid value `{bad}` for `port-hopping` (expected ports from 1 to 65535 and ranges such as `5000-6000`, separated by `;`)"
                    )
                    .as_str()
                )]
            );
        }
    }

    /// 30 seconds unless written (manual); shorter than 5 is raised to 5,
    /// the floor of the reference implementation (phase 2 M7 design 3.1).
    #[test]
    fn the_interval_is_whole_seconds_and_at_least_5() {
        for (written, secs) in [("60", 60), ("5", 5), (" 10 ", 10)] {
            let (quic, _, failed, diags) = read(&format!(
                "hysteria2, h.test, 443, password=p, port-hopping=5000-6000, port-hopping-interval=\"{written}\""
            ));
            assert!(!failed && diags.is_empty(), "{written}: {diags:?}");
            assert_eq!(quic.port_hopping, hopping(vec![5000..=6000], secs));
        }
        for short in ["4", "0"] {
            let (quic, _, failed, diags) = read(&format!(
                "hysteria2, h.test, 443, password=p, port-hopping=5000-6000, port-hopping-interval={short}"
            ));
            assert!(!failed, "{short}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::W_INVALID_VALUE,
                    "policy `P`: `port-hopping-interval` is below the minimum of 5 seconds; using 5"
                )]
            );
            assert_eq!(quic.port_hopping, hopping(vec![5000..=6000], 5));
        }
        for bad in ["-1", "2.5", "30s", "", "4294967296"] {
            let (_, _, failed, diags) = read(&format!(
                "hysteria2, h.test, 443, password=p, port-hopping=5000-6000, port-hopping-interval=\"{bad}\""
            ));
            assert!(failed, "{bad}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    format!(
                        "policy `P`: invalid value `{bad}` for `port-hopping-interval` (expected a whole number of seconds)"
                    )
                    .as_str()
                )]
            );
        }
    }

    #[test]
    fn the_interval_has_no_effect_without_ports() {
        for interval in ["10", "soon"] {
            let (quic, _, failed, diags) = read(&format!(
                "hysteria2, h.test, 443, password=p, port-hopping-interval={interval}"
            ));
            assert!(!failed, "{interval}");
            assert_eq!(quic, QuicOpts::default());
            assert_eq!(
                messages(&diags),
                [(
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `port-hopping-interval` has no effect without `port-hopping`; ignored"
                )]
            );
        }
    }

    /// QUIC runs over UDP: TCP Fast Open has nothing to act on.
    #[test]
    fn tfo_does_not_apply() {
        let (_, common, failed, diags) = read("hysteria2, h.test, 443, password=p, tfo=true");
        assert!(!failed);
        assert!(!common.tfo);
        assert_eq!(
            messages(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `tfo` does not apply to `hysteria2` policies; ignored"
            )]
        );
    }

    #[test]
    fn alpn_is_h3_unless_written() {
        for (def, alpn) in [
            ("hysteria2, h.test, 443", vec!["h3"]),
            ("hysteria2, h.test, 443, alpn=", vec!["h3"]),
            ("hysteria2, h.test, 443, alpn=h3", vec!["h3"]),
            (
                "hysteria2, h.test, 443, alpn=\"h3-29, h3\"",
                vec!["h3-29", "h3"],
            ),
        ] {
            let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
            let mut r = ParamReader::new(&p);
            let tls = read_quic_tls(&mut r, &[]);
            assert!(r.finish().is_empty(), "{def}");
            assert_eq!(tls.alpn, alpn, "{def}");
        }
    }
}
