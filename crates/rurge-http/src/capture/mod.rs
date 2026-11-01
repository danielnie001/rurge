//! What the engine kept of the requests and responses it served (`phase 4 M4
//! design 4`): one store per engine, in memory only, up to a total and a
//! per-body limit; entries are costed as they arrive, the oldest go first,
//! and a body that does not fit is truncated rather than dropped.
//!
//! Nothing here touches the network, and nothing here is written anywhere:
//! the headers, bodies and URLs an entry holds are the user's traffic, so no
//! type in this module implements `Debug` and none of it goes into a log line
//! (`phase 4 M4 design 6.6`).

use crate::rewrite::{Undo, layers, undo};
use bytes::Bytes;
use http::header::CONTENT_ENCODING;
use http::{HeaderMap, HeaderValue, Method, StatusCode, Version};
use rurge_proto::RejectKind;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use time::OffsetDateTime;

mod slot;
mod store;

pub(crate) use slot::RequestTap;
pub(crate) use store::Pending;
pub use store::{CaptureStore, SessionSummary, Status};

/// The session a request enters when `capture` is on and its rule names no
/// session of its own (`phase 4 M4 design 5.3`).
pub const DEFAULT_SESSION: &str = "default";

/// How much of the captured requests is kept when nothing else is asked for:
/// 100 MiB (`phase 4 M4 design 4.2`).
pub const DEFAULT_TOTAL: u64 = 100 * 1024 * 1024;

/// How much of each captured body is kept when nothing else is asked for:
/// 5 MiB (`phase 4 M4 design 4.2`).
pub const DEFAULT_BODY: u64 = 5 * 1024 * 1024;

/// What each entry costs besides its headers, texts and bodies: the entry
/// itself, a slot of the queue and a few `String` and `Vec` headers
/// (`phase 4 M4 design 4.3`); the phase 4 / M4a plan's task 8 measures it and
/// may change it.
pub const ENTRY_OVERHEAD: u64 = 512;

/// What the store holds at most: in all, and of each body (`phase 4 M4
/// design 4.2`). A body limit above the total is lowered to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub total: u64,
    pub body: u64,
}

impl Limits {
    /// Limits of `total` bytes in all and `body` bytes of each body, the body
    /// one lowered to the total: a body can never be the reason a store holds
    /// more than its total.
    pub fn new(total: u64, body: u64) -> Limits {
        Limits {
            total,
            body: body.min(total),
        }
    }
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::new(DEFAULT_TOTAL, DEFAULT_BODY)
    }
}

/// Why a captured body was not decoded for the API or a HAR (`phase 4 M4
/// design 4.4`, `7.5`), in the words a note uses (the phase 4 / M4a plan's
/// P7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotDecoded {
    /// The body was truncated, so decoding it would be decoding a part.
    Truncated,
    /// A layer nobody supports, named when its name may be written down.
    Unsupported(Option<String>),
    /// A layer that does not decode: the name it was written as.
    Corrupt(String),
    /// The decoded body is over `limit` bytes.
    TooLarge(u64),
}

impl fmt::Display for NotDecoded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NotDecoded::Truncated => f.write_str("the body was truncated"),
            NotDecoded::Unsupported(Some(name)) => {
                write!(f, "content-encoding {name} is not supported")
            }
            NotDecoded::Unsupported(None) => f.write_str("the content-encoding is not supported"),
            NotDecoded::Corrupt(name) => write!(f, "the body cannot be decoded as {name}"),
            NotDecoded::TooLarge(limit) => write!(f, "the decoded body is over {limit} bytes"),
        }
    }
}

/// A place to capture into: the store the entries go, and the session they
/// belong to (`phase 4 M4 design 4.6`, `5.3`). Cheap to clone; cloning it does
/// not start anything.
#[derive(Clone)]
pub struct Capture {
    pub store: Arc<CaptureStore>,
    pub session: Arc<str>,
}

impl Capture {
    /// Start an entry: its bodies are taken as they come and it is stored by
    /// `Pending::commit` (`phase 4 M4 design 4.3`, `6.4`).
    pub(crate) fn begin(&self) -> Pending {
        Pending::new(self)
    }
}

/// One request and its response as they were captured (`phase 4 M4 design
/// 6`). It is the same `id` the request has in the request records, and none
/// of it is ever printed.
pub struct Entry {
    pub id: u64,
    pub session: Arc<str>,
    /// The wall clock time the request started at.
    pub started: SystemTime,
    pub request: Head,
    pub request_body: Body,
    pub response: Option<ResponseHead>,
    pub response_body: Body,
    pub timings: Timings,
    pub remote: Option<SocketAddr>,
    pub outcome: Outcome,
    /// The reason the request failed, as the record has it (`phase 4 M4
    /// design 6.5`): what `set_error` wrote, or the failure's own message when
    /// it wrote none; a rejected request may have one too.
    pub error: Option<String>,
    /// The `[Rule]` line that matched, as the record has it.
    pub rule: Option<String>,
    pub policy: Vec<String>,
    pub notes: Vec<String>,
}

impl Entry {
    /// When the request started, in milliseconds since the Unix epoch; 0 when
    /// that is before the epoch.
    pub fn started_ms(&self) -> u64 {
        match self.started.duration_since(UNIX_EPOCH) {
            Ok(since) => since.as_millis() as u64,
            Err(_) => 0,
        }
    }

    /// When the request started, as `YYYY-MM-DDTHH:MM:SS.mmmZ` in UTC (the
    /// phase 4 / M4a plan's P2, `P5`).
    pub fn started_rfc3339(&self) -> String {
        let at = OffsetDateTime::from(self.started);
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            at.year(),
            u8::from(at.month()),
            at.day(),
            at.hour(),
            at.minute(),
            at.second(),
            at.millisecond()
        )
    }
}

/// A request's first line and headers, as they were captured (`phase 4 M4
/// design 6.1`).
pub struct Head {
    pub method: Method,
    pub url: String,
    pub version: Version,
    pub headers: HeaderMap,
}

/// A response's status line and headers (`phase 4 M4 design 6.3`).
pub struct ResponseHead {
    pub status: StatusCode,
    pub version: Version,
    pub headers: HeaderMap,
}

/// A captured body: what was kept of it, how much was seen, and why it stops
/// short (`phase 4 M4 design 4.4`).
pub struct Body {
    /// The bytes kept, at most the body limit of the store.
    pub data: Bytes,
    /// How many bytes went by, kept or not.
    pub size: u64,
    pub truncated: Option<Truncated>,
}

impl Body {
    /// A body that was not there or was not kept at all.
    pub fn empty() -> Body {
        Body {
            data: Bytes::new(),
            size: 0,
            truncated: None,
        }
    }

    /// The body with its `Content-Encoding` layers undone, up to `limit` bytes
    /// (`phase 4 M4 design 7.5`): `Ok(None)` is a body with no layer to undo,
    /// truncated or not.
    pub fn decoded(&self, headers: &HeaderMap, limit: u64) -> Result<Option<Bytes>, NotDecoded> {
        let encodings: Vec<HeaderValue> =
            headers.get_all(CONTENT_ENCODING).iter().cloned().collect();
        let names = layers(&encodings).map_err(|why| match why {
            Undo::Unsupported(name) => NotDecoded::Unsupported(name),
            Undo::Corrupt(name) => NotDecoded::Corrupt(name),
            // `layers` reads no body, so it never says `Over`
            Undo::Over => NotDecoded::Unsupported(None),
        })?;
        if names.is_empty() {
            return Ok(None);
        }
        if self.truncated.is_some() {
            return Err(NotDecoded::Truncated);
        }
        undo(&names, &self.data, limit)
            .map(Some)
            .map_err(|why| match why {
                Undo::Unsupported(name) => NotDecoded::Unsupported(name),
                Undo::Corrupt(name) => NotDecoded::Corrupt(name),
                Undo::Over => NotDecoded::TooLarge(limit),
            })
    }
}

/// Why a body stops short (`phase 4 M4 design 4.4`): it reached the body
/// limit, or the store had no room left for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Truncated {
    Limit,
    Budget,
}

/// What a request took, in the way the request records measure it
/// (`phase 4 M4 design 6.5`); what a store could not tell is `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timings {
    pub connect: Option<Duration>,
    pub dns: Option<Duration>,
    pub wait: Option<Duration>,
    pub receive: Option<Duration>,
    pub total: Duration,
    /// Whether the request borrowed an upstream connection.
    pub reused: bool,
}

/// How a request ended (`phase 4 M4 design 6.4`, `6.5`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    Upgraded,
    Rejected(RejectKind),
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::{GzEncoder, ZlibEncoder};
    use std::io::Write;

    /// An entry whose start and response body are the ones given; everything
    /// else is the same in every case here.
    fn entry(started: SystemTime, response_body: Body) -> Entry {
        Entry {
            id: 1,
            session: Arc::from("a"),
            started,
            request: Head {
                method: Method::GET,
                url: "http://a.test/".to_string(),
                version: Version::HTTP_11,
                headers: HeaderMap::new(),
            },
            request_body: Body::empty(),
            response: None,
            response_body,
            timings: Timings {
                connect: None,
                dns: None,
                wait: None,
                receive: None,
                total: Duration::ZERO,
                reused: false,
            },
            remote: None,
            outcome: Outcome::Completed,
            error: None,
            rule: None,
            policy: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// A body that went by whole, of `data`.
    fn whole(data: Bytes) -> Body {
        Body {
            size: data.len() as u64,
            data,
            truncated: None,
        }
    }

    /// A header value that is a visible ASCII name.
    fn encoding(name: &str) -> HeaderValue {
        HeaderValue::from_str(name).expect("an ASCII name")
    }

    /// The headers of a body encoded in `values`, one line each.
    fn encodings(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(CONTENT_ENCODING, encoding(value));
        }
        headers
    }

    /// `data` gzipped, as one member.
    fn gzipped(data: &[u8]) -> Bytes {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).expect("in memory");
        Bytes::from(encoder.finish().expect("in memory"))
    }

    /// `data` in a zlib stream.
    fn zlibbed(data: &[u8]) -> Bytes {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).expect("in memory");
        Bytes::from(encoder.finish().expect("in memory"))
    }

    /// The body limit never exceeds the total, and the defaults are 100 MiB
    /// and 5 MiB (`phase 4 M4 design 4.2`).
    #[test]
    fn limits_lower_the_body_to_the_total() {
        assert_eq!(
            Limits::new(10, 20),
            Limits {
                total: 10,
                body: 10
            }
        );
        assert_eq!(
            Limits::new(20, 10),
            Limits {
                total: 20,
                body: 10
            }
        );
        assert_eq!(
            Limits::default(),
            Limits {
                total: 104_857_600,
                body: 5_242_880
            }
        );
    }

    /// A start is a Unix time in milliseconds and a UTC stamp with
    /// milliseconds (`phase 4 M4 design 7.5`, the phase 4 / M4a plan's P2).
    #[test]
    fn started_is_written_as_rfc3339_in_utc_with_milliseconds() {
        let at = UNIX_EPOCH + Duration::from_millis(1_761_984_000_123);
        let stamped = entry(at, Body::empty());
        assert_eq!(stamped.started_ms(), 1_761_984_000_123);
        assert_eq!(stamped.started_rfc3339(), "2025-11-01T08:00:00.123Z");

        let epoch = entry(UNIX_EPOCH + Duration::from_millis(5), Body::empty());
        assert_eq!(epoch.started_rfc3339(), "1970-01-01T00:00:00.005Z");
    }

    /// The layers of `Content-Encoding` are undone from the last one on, and
    /// an empty body under one of them decodes to empty (`phase 4 M3 design
    /// 9.4`, `phase 4 M4 design 7.5`).
    #[test]
    fn bodies_are_decoded_layer_by_layer() {
        let text = Bytes::from_static(b"hello there");
        let twice = gzipped(&zlibbed(&text));
        assert_eq!(
            whole(twice.clone())
                .decoded(&encodings(&["deflate, gzip"]), 1024)
                .expect("deflate then gzip"),
            Some(text.clone())
        );

        let gz = gzipped(&text);
        for name in ["gzip", "x-gzip"] {
            assert_eq!(
                whole(gz.clone())
                    .decoded(&encodings(&[name]), 1024)
                    .expect("gzip"),
                Some(text.clone()),
                "{name}"
            );
        }

        assert_eq!(
            whole(twice)
                .decoded(&encodings(&["deflate", "gzip"]), 1024)
                .expect("two lines"),
            Some(text),
            "two lines undo as one comma-separated pair"
        );

        assert_eq!(
            whole(Bytes::new())
                .decoded(&encodings(&["gzip"]), 1024)
                .expect("an empty body is not a truncated header"),
            Some(Bytes::new())
        );
    }

    /// A body with no layer to undo is not decoded, a truncated one either
    /// (`phase 4 M4 design 7.5`, the phase 4 / M4a plan's P7).
    #[test]
    fn a_body_without_an_encoding_is_not_decoded() {
        let data = Bytes::from_static(b"hello");
        assert_eq!(
            whole(data.clone())
                .decoded(&HeaderMap::new(), 1024)
                .expect("no layer"),
            None
        );
        for values in [&["identity"][..], &[""][..], &["identity", ""][..]] {
            assert_eq!(
                whole(data.clone())
                    .decoded(&encodings(values), 1024)
                    .expect("no layer"),
                None,
                "{values:?}"
            );
        }

        let truncated = Body {
            size: 100,
            data: data.clone(),
            truncated: Some(Truncated::Limit),
        };
        assert_eq!(
            truncated
                .decoded(&HeaderMap::new(), 1024)
                .expect("no layer"),
            None,
            "a truncated body with no layer is not decoded either"
        );
    }

    /// Every reason a body cannot be decoded is said in the words of a note
    /// (`phase 4 M3 design 9.4`, the phase 4 / M4a plan's P7).
    #[test]
    fn what_cannot_be_decoded_says_why() {
        let text = Bytes::from_static(b"hello there");
        let gz = gzipped(&text);

        let truncated = Body {
            size: 100,
            data: gz,
            truncated: Some(Truncated::Budget),
        };
        let Err(why) = truncated.decoded(&encodings(&["gzip"]), 1024) else {
            panic!("a truncated body is not decoded")
        };
        assert_eq!(why, NotDecoded::Truncated);
        assert_eq!(why.to_string(), "the body was truncated");

        let Err(why) = whole(text.clone()).decoded(&encodings(&["br"]), 1024) else {
            panic!("br is nobody's decoder here")
        };
        assert_eq!(why, NotDecoded::Unsupported(Some("br".to_string())));
        assert_eq!(why.to_string(), "content-encoding br is not supported");

        let Err(why) = whole(text.clone()).decoded(&encodings(&["a@b"]), 1024) else {
            panic!("a name that is no token is not written down")
        };
        assert_eq!(why, NotDecoded::Unsupported(None));
        assert_eq!(why.to_string(), "the content-encoding is not supported");

        let corrupt = Bytes::from_static(b"\0\x01 rubbish");
        let Err(why) = whole(corrupt).decoded(&encodings(&["gzip"]), 1024) else {
            panic!("not gzip")
        };
        assert_eq!(why, NotDecoded::Corrupt("gzip".to_string()));
        assert_eq!(why.to_string(), "the body cannot be decoded as gzip");

        let bomb = gzipped(&vec![0u8; 1024]);
        let Err(why) = whole(bomb).decoded(&encodings(&["gzip"]), 10) else {
            panic!("over the limit")
        };
        assert_eq!(why, NotDecoded::TooLarge(10));
        assert_eq!(why.to_string(), "the decoded body is over 10 bytes");
    }
}
