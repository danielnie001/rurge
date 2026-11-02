//! `GET /v1/rurge/http`: what rurge's HTTP engine has done (phase 4 M1 design
//! 11). It is rurge's own endpoint, not one Surge has, so it lives under
//! `/v1/rurge/…` — the prefix phase 4's general design gives rurge's own
//! interfaces. M1a has the five ways a sniff can end; M1b adds `requests`;
//! M2a adds `mitm`, the outcomes of the decrypted TLS sessions.
//! M3a adds `rewrite`, what the rewrites did (`phase 4 M3 design 12`).
//! M5a adds `quic`, how the QUIC ClientHellos were read and how many flows
//! `auto-quic-block` blocked (`phase 4 M5 design 7`).

use crate::App;
use axum::Json;
use axum::extract::State;
use serde::Serialize;

#[derive(Serialize)]
pub struct HttpJson {
    pub sniffed: SniffedJson,
    /// The requests that reached the rules, refused ones included.
    pub requests: u64,
    /// The TLS sessions `[MITM]` decrypted, and how the ones that did not go
    /// through ended (`phase 4 M2 design 12.3`).
    pub mitm: MitmJson,
    /// What the rewrites of `[URL Rewrite]`, `[Header Rewrite]`,
    /// `[Body Rewrite]` and `[Map Local]` did (`phase 4 M3 design 12`), since
    /// the engine started.
    pub rewrite: RewriteJson,
    /// The QUIC ClientHellos of the flows through the engine, how they were
    /// read, and the flows `auto-quic-block` blocked (`phase 4 M5 design 5.4`),
    /// since the engine started.
    pub quic: QuicJson,
}

/// The outcomes of the QUIC ClientHellos (`phase 4 M5 design 5.4`): the flows
/// whose ClientHello came whole and parsed, the ones it did not come whole for
/// within the wait or the datagram count, the ones it came whole for but could
/// not be read or which the reader gave up on, and the ones
/// `auto-quic-block` blocked, which are also counted as read.
#[derive(Serialize)]
pub struct QuicJson {
    pub hello: u64,
    pub incomplete: u64,
    pub unreadable: u64,
    pub blocked: u64,
}

impl From<rurge_engine::QuicSniffed> for QuicJson {
    fn from(counts: rurge_engine::QuicSniffed) -> Self {
        Self {
            hello: counts.hello,
            incomplete: counts.incomplete,
            unreadable: counts.unreadable,
            blocked: counts.blocked,
        }
    }
}

/// The rewrite counts (`phase 4 M3 design 12`, `M3-D25`): `body` is the
/// requests whose body a `[Body Rewrite]` rule changed, once each however many
/// rules and both directions acted; `mapLocal` the requests a `[Map Local]`
/// rule answered, the `HEAD`, 204 and 304 answers included and the ones it
/// could not answer not.
#[derive(Serialize)]
pub struct RewriteJson {
    pub url: u64,
    pub header: u64,
    pub body: u64,
    #[serde(rename = "mapLocal")]
    pub map_local: u64,
    pub skipped: u64,
    pub failed: u64,
}

/// The outcomes of a handshake rurge answers as the server: the sessions it
/// decrypted, the clients that did not trust the certificate rurge issued,
/// the handshakes that failed another way, and the clients that finished the
/// handshake and went away without a request.
#[derive(Serialize)]
pub struct MitmJson {
    pub decrypted: u64,
    pub untrusted: u64,
    pub failed: u64,
    #[serde(rename = "noRequest")]
    pub no_request: u64,
}

/// The outcomes of the sniffs since the engine started, in the order the
/// design writes them: a TLS ClientHello, plain HTTP, something else, nothing
/// within the wait, and a client that went away.
#[derive(Serialize)]
pub struct SniffedJson {
    pub tls: u64,
    pub http: u64,
    pub other: u64,
    pub timeout: u64,
    pub closed: u64,
}

pub async fn http(State(app): State<App>) -> Json<HttpJson> {
    // The engine's counts carry no `Serialize` of their own (plan P16): this
    // endpoint owns the shape, so the fields are copied over one by one.
    let counts = app.engine.http_counts();
    Json(HttpJson {
        sniffed: SniffedJson {
            tls: counts.sniffed.tls,
            http: counts.sniffed.http,
            other: counts.sniffed.other,
            timeout: counts.sniffed.timeout,
            closed: counts.sniffed.closed,
        },
        requests: counts.requests,
        mitm: MitmJson {
            decrypted: counts.mitm.decrypted,
            untrusted: counts.mitm.untrusted,
            failed: counts.mitm.failed,
            no_request: counts.mitm.no_request,
        },
        rewrite: RewriteJson {
            url: counts.rewrite.url,
            header: counts.rewrite.header,
            body: counts.rewrite.body,
            map_local: counts.rewrite.map_local,
            skipped: counts.rewrite.skipped,
            failed: counts.rewrite.failed,
        },
        quic: QuicJson::from(counts.quic),
    })
}

#[cfg(test)]
mod tests {
    use super::QuicJson;
    use rurge_engine::QuicSniffed;

    /// The four QUIC counts go out under the names the design gives them, in
    /// its order (`phase 4 M5 design 7`).
    #[test]
    fn the_quic_counts_go_out_under_their_names() {
        let json = serde_json::to_string(&QuicJson::from(QuicSniffed {
            hello: 1,
            incomplete: 2,
            unreadable: 3,
            blocked: 4,
        }))
        .unwrap();
        assert_eq!(
            json,
            r#"{"hello":1,"incomplete":2,"unreadable":3,"blocked":4}"#
        );
    }
}
