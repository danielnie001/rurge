//! The authentication of every new connection (Hysteria 2 protocol,
//! "Authentication"; phase 2 M7 design 5.2): an HTTP/3 `POST
//! https://hysteria/auth` carrying the password (`Hysteria-Auth`), the rate
//! the client takes in (`Hysteria-CC-RX`, bytes per second, `0` for
//! unknown) and padding. Status 233 lets the client in and says whether
//! the server relays UDP (`Hysteria-UDP`); any other is the web site the
//! server shows strangers: the authentication failed, and the connection
//! is closed as the reference client closes it. The server's own
//! `Hysteria-CC-RX` changes nothing here (M7-D10).

use super::padding;
use super::udp::Datagrams;
use crate::session::{Protocol, close};
use http::header::{HeaderName, HeaderValue};
use http::{HeaderMap, Method, Request, Uri};
use quinn::{Connection, VarInt};
use rurge_net::BoxFuture;
use rurge_net::quic::h3;
use rurge_proto::OutboundError;
use std::ops::RangeInclusive;
use tokio::io::AsyncWriteExt;

/// H3_NO_ERROR (RFC 9114 8.1): a connection let go of.
const H3_NO_ERROR: VarInt = VarInt::from_u32(0x100);
/// H3_GENERAL_PROTOCOL_ERROR (RFC 9114 8.1): what the reference client
/// closes a connection with when its authentication failed
/// (`core/client/client.go`).
const H3_GENERAL_PROTOCOL_ERROR: VarInt = VarInt::from_u32(0x101);

/// The status of an authentication that let the client in.
const AUTH_OK: u16 = 233;

const AUTH: HeaderName = HeaderName::from_static("hysteria-auth");
const CC_RX: HeaderName = HeaderName::from_static("hysteria-cc-rx");
const PADDING: HeaderName = HeaderName::from_static("hysteria-padding");
const UDP: HeaderName = HeaderName::from_static("hysteria-udp");

/// The reference's padding of an authentication request.
const AUTH_PADDING: RangeInclusive<usize> = 256..=2047;

/// What authenticates the policy's connections. No `Debug`: it holds the
/// password.
pub(crate) struct Auth {
    /// `Hysteria-Auth`, marked sensitive: QPACK never indexes it.
    password: HeaderValue,
    /// `Hysteria-CC-RX`.
    rx: u64,
}

/// What an authenticated connection keeps for its requests.
pub(crate) struct Authenticated {
    /// Lives as long as the connection: dropped, it would end our HTTP/3
    /// control stream, which a server may take for the end of HTTP/3 on
    /// the connection.
    h3: h3::Client,
    /// The connection's UDP sessions, when the server relays UDP
    /// (`Hysteria-UDP`).
    pub(super) udp: Option<Datagrams>,
}

impl Auth {
    /// `None` for a password no HTTP field can carry (a control
    /// character). `download_bandwidth` is in Mbps; the server learns it in
    /// bytes per second.
    pub(crate) fn new(password: &str, download_bandwidth: u32) -> Option<Auth> {
        let mut password = HeaderValue::from_str(password).ok()?;
        password.set_sensitive(true);
        Some(Auth {
            password,
            rx: u64::from(download_bandwidth) * 125_000,
        })
    }

    /// The authentication request, its padding drawn anew.
    fn request(&self) -> Request<()> {
        let mut request = Request::new(());
        *request.method_mut() = Method::POST;
        *request.uri_mut() = Uri::from_static("https://hysteria/auth");
        let padding = HeaderValue::from_bytes(&padding(AUTH_PADDING)).expect("letters and digits");
        let headers = request.headers_mut();
        headers.insert(AUTH, self.password.clone());
        headers.insert(CC_RX, HeaderValue::from(self.rx));
        headers.insert(PADDING, padding);
        request
    }

    /// Whether the server relays UDP, once it let the client in.
    async fn authenticate(&self, h3: &h3::Client) -> Result<bool, OutboundError> {
        let mut stream = h3.request(self.request()).await?;
        // no content
        stream.shutdown().await?;
        let response = stream.response().await?;
        if response.status().as_u16() != AUTH_OK {
            return Err(OutboundError::Proxy(
                "hysteria2: authentication failed".to_string(),
            ));
        }
        Ok(relays_udp(response.headers()))
    }
}

/// `Hysteria-UDP` as the reference reads it (Go's `strconv.ParseBool`):
/// anything but one of its forms of true — no field included — is false.
fn relays_udp(headers: &HeaderMap) -> bool {
    headers.get(UDP).is_some_and(|value| {
        matches!(
            value.as_bytes(),
            b"1" | b"t" | b"T" | b"true" | b"TRUE" | b"True"
        )
    })
}

impl Protocol for Auth {
    type State = Authenticated;

    const NAME: &'static str = "hysteria2";

    const NO_ERROR: VarInt = H3_NO_ERROR;

    fn handshake<'a>(
        &'a self,
        connection: &'a Connection,
    ) -> BoxFuture<'a, Result<Authenticated, OutboundError>> {
        Box::pin(async move {
            let h3 = h3::Client::new(connection.clone());
            match self.authenticate(&h3).await {
                Ok(udp) => Ok(Authenticated {
                    h3,
                    udp: udp.then(|| Datagrams::start(connection)),
                }),
                Err(e) => {
                    close(connection, H3_GENERAL_PROTOCOL_ERROR);
                    Err(e)
                }
            }
        })
    }

    /// The server sent GOAWAY: it is going down.
    fn is_going_away(&self, state: &Authenticated) -> bool {
        state.h3.is_going_away()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_is_a_post_to_hysteria_auth_that_hides_the_password() {
        let auth = Auth::new("s3cret", 8).expect("a password a field can carry");
        let request = auth.request();
        assert_eq!(request.method(), Method::POST);
        assert_eq!(request.uri(), "https://hysteria/auth");
        let headers = request.headers();
        assert_eq!(headers[AUTH], "s3cret");
        assert!(headers[AUTH].is_sensitive());
        // 8 Mbps in bytes per second
        assert_eq!(headers[CC_RX], "1000000");
        let padding = headers[PADDING].as_bytes();
        assert!(AUTH_PADDING.contains(&padding.len()), "{}", padding.len());
        assert!(padding.iter().all(u8::is_ascii_alphanumeric));
        assert_eq!(headers.len(), 3);
        assert!(!format!("{request:?}").contains("s3cret"));
        // drawn anew for every connection
        let lengths: std::collections::HashSet<usize> = (0..50)
            .map(|_| auth.request().headers()[PADDING].len())
            .collect();
        assert!(lengths.len() > 1, "{lengths:?}");
        // without a bandwidth the rate is unknown
        let unknown = Auth::new("s3cret", 0).unwrap();
        assert_eq!(unknown.request().headers()[CC_RX], "0");
    }

    #[test]
    fn a_password_with_a_control_character_cannot_be_sent() {
        for password in ["a\nb", "a\rb", "a\0b", "a\u{1}b", "a\u{7f}b"] {
            assert!(Auth::new(password, 0).is_none(), "{password:?}");
        }
        for password in ["a b", "tab\there", "ünïcödé", "p@ss:w;rd"] {
            assert!(Auth::new(password, 0).is_some(), "{password:?}");
        }
    }

    #[test]
    fn hysteria_udp_reads_as_the_reference_reads_it() {
        let answer = |value: Option<&'static str>| {
            let mut headers = HeaderMap::new();
            if let Some(value) = value {
                headers.insert(UDP, HeaderValue::from_static(value));
            }
            relays_udp(&headers)
        };
        for value in ["true", "1", "t", "T", "TRUE", "True"] {
            assert!(answer(Some(value)), "{value}");
        }
        for value in ["false", "0", "yes", "tRUE", "", " true"] {
            assert!(!answer(Some(value)), "{value:?}");
        }
        assert!(!answer(None));
    }
}
