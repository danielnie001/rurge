//! WebSocket transport parameters (`ws`, `ws-path`, `ws-headers`; manual:
//! Policies › VMess, Policies › Trojan).

use super::http::{is_field_text, is_token};
use super::reader::ParamReader;
use crate::diagnostic::codes;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WsOpts {
    /// Starts with `/`; may carry a query.
    pub path: String,
    /// Extra handshake headers in the order written. A `Host` entry replaces
    /// the default `Host`.
    pub headers: Vec<(String, String)>,
}

const WS_KEYS: [&str; 2] = ["ws-path", "ws-headers"];

/// Headers the WebSocket handshake writes itself (see `transport::ws`).
fn is_managed(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "connection" || lower == "upgrade" || lower.starts_with("sec-websocket-")
}

fn valid_path(path: &str) -> bool {
    path.starts_with('/') && path.bytes().all(|b| b.is_ascii_graphic())
}

/// `None` without `ws=true`; the other two parameters are then `W0028`.
/// Neither the path nor a header value is ever quoted in a diagnostic: both
/// are routinely used as shared secrets.
pub fn read_ws(r: &mut ParamReader<'_>) -> Option<WsOpts> {
    if !r.bool("ws").unwrap_or(false) {
        for key in WS_KEYS {
            if r.has(key) {
                r.touch(key);
                r.warn(
                    codes::W_PARAM_NOT_APPLICABLE,
                    format!("`{key}` has no effect without `ws=true`; ignored"),
                );
            }
        }
        return None;
    }
    let mut path = "/".to_string();
    if let Some(v) = r.str("ws-path") {
        let v = v.trim();
        if valid_path(v) {
            path = v.to_string();
        } else {
            r.error(
                codes::E_INVALID_POLICY_PARAM,
                "invalid `ws-path` (expected an ASCII path that starts with `/` and holds no space or control character)"
                    .to_string(),
            );
        }
    }
    let mut headers = Vec::new();
    if let Some(list) = r.str("ws-headers") {
        let items = list.split('|').map(str::trim).filter(|i| !i.is_empty());
        for (n, item) in items.enumerate() {
            let Some((name, value)) = item.split_once(':') else {
                r.error(
                    codes::E_INVALID_POLICY_PARAM,
                    format!("invalid `ws-headers`: entry #{} has no `:`", n + 1),
                );
                continue;
            };
            let (name, value) = (name.trim(), value.trim());
            if !is_token(name) {
                r.error(
                    codes::E_INVALID_POLICY_PARAM,
                    format!(
                        "invalid `ws-headers`: entry #{} has an invalid header name",
                        n + 1
                    ),
                );
            } else if !is_field_text(value) {
                r.error(
                    codes::E_INVALID_POLICY_PARAM,
                    format!("invalid `ws-headers`: header `{name}` holds a control character"),
                );
            } else if is_managed(name) {
                r.warn(
                    codes::W_INVALID_VALUE,
                    format!(
                        "`ws-headers`: `{name}` is written by the WebSocket handshake itself; ignored"
                    ),
                );
            } else {
                headers.push((name.to_string(), value.to_string()));
            }
        }
    }
    Some(WsOpts { path, headers })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use std::path::Path;
    use std::sync::Arc;

    fn read(def: &str) -> (Option<WsOpts>, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        // what the caller has read by then
        r.touch("password");
        let ws = read_ws(&mut r);
        (ws, r.finish())
    }

    #[test]
    fn defaults_and_the_manuals_spelling() {
        let (ws, diags) = read("trojan, h.test, 443, password=p, ws=true");
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(
            ws,
            Some(WsOpts {
                path: "/".into(),
                headers: Vec::new()
            })
        );
        let (ws, diags) = read(
            "trojan, h.test, 443, password=p, ws=true, ws-path=/ray?ed=1, ws-headers=Host:edge.example|X-Key: v 1 ",
        );
        assert!(diags.is_empty(), "{diags:?}");
        let ws = ws.unwrap();
        assert_eq!(ws.path, "/ray?ed=1");
        assert_eq!(
            ws.headers,
            [
                ("Host".to_string(), "edge.example".to_string()),
                ("X-Key".to_string(), "v 1".to_string())
            ]
        );
    }

    #[test]
    fn without_ws_the_other_two_are_not_applicable() {
        let (ws, diags) = read("trojan, h.test, 443, password=p, ws-path=/x, ws-headers=A:b");
        assert_eq!(ws, None);
        let messages: Vec<(&str, &str)> =
            diags.iter().map(|d| (d.code, d.message.as_str())).collect();
        assert_eq!(
            messages,
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `ws-path` has no effect without `ws=true`; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `ws-headers` has no effect without `ws=true`; ignored"
                ),
            ]
        );
        let (ws, diags) = read("trojan, h.test, 443, password=p, ws=false");
        assert!(ws.is_none() && diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn a_bad_path_is_an_error_that_never_quotes_it() {
        for bad in ["ws-path=secret", "ws-path=/a b", "ws-path=/é", "ws-path="] {
            let (_, diags) = read(&format!("trojan, h.test, 443, password=p, ws=true, {bad}"));
            assert_eq!(diags.len(), 1, "{bad}: {diags:?}");
            assert_eq!(diags[0].code, codes::E_INVALID_POLICY_PARAM);
            assert_eq!(
                diags[0].message,
                "policy `P`: invalid `ws-path` (expected an ASCII path that starts with `/` and holds no space or control character)"
            );
        }
    }

    #[test]
    fn bad_headers_are_errors_and_managed_ones_are_dropped() {
        let (_, diags) = read("trojan, h.test, 443, password=p, ws=true, ws-headers=nocolon");
        assert_eq!(
            diags[0].message,
            "policy `P`: invalid `ws-headers`: entry #1 has no `:`"
        );
        let (_, diags) =
            read("trojan, h.test, 443, password=p, ws=true, ws-headers=A:b|bad name:v");
        assert_eq!(
            diags[0].message,
            "policy `P`: invalid `ws-headers`: entry #2 has an invalid header name"
        );
        let (_, diags) =
            read("trojan, h.test, 443, password=p, ws=true, ws-headers=X-A:line\u{7}feed");
        assert_eq!(
            (diags[0].code, diags[0].message.as_str()),
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: invalid `ws-headers`: header `X-A` holds a control character"
            )
        );
        let (ws, diags) = read(
            "trojan, h.test, 443, password=p, ws=true, ws-headers=Upgrade:h2c|X-A:1|Sec-WebSocket-Protocol:x|connection:close",
        );
        assert_eq!(ws.unwrap().headers, [("X-A".to_string(), "1".to_string())]);
        let dropped: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
        assert!(diags.iter().all(|d| d.code == codes::W_INVALID_VALUE));
        assert_eq!(
            dropped,
            [
                "policy `P`: `ws-headers`: `Upgrade` is written by the WebSocket handshake itself; ignored",
                "policy `P`: `ws-headers`: `Sec-WebSocket-Protocol` is written by the WebSocket handshake itself; ignored",
                "policy `P`: `ws-headers`: `connection` is written by the WebSocket handshake itself; ignored",
            ]
        );
    }
}
