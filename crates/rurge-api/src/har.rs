//! A capture session written as HAR 1.2 (`phase 4 M4 design 8`), for
//! `GET /v1/rurge/capture/sessions/{name}/har`.
//!
//! One entry maps the way the table of `phase 4 M4 design 8.2` says; an entry
//! the engine never answered is written the way mitmproxy writes one. The
//! entries are written one at a time (`phase 4 M4 design 8.3`): the export
//! does not build the whole file in memory first, and the `Arc` of the
//! snapshot keeps an entry whole until it is written, even when the store
//! evicts or clears it in between.

use crate::routes::capture::{headers_json, outcome_name, reject_kind};
use axum::body::Body;
use axum::http::HeaderMap;
use axum::http::header::{CONTENT_ENCODING, CONTENT_TYPE, LOCATION};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rurge_engine::capture::{Body as CapturedBody, Entry, Timings};
use serde_json::{Value, json};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

/// How long something took, in whole milliseconds, rounded down.
fn ms(d: Duration) -> i64 {
    d.as_millis() as i64
}

/// The timings of one entry as HAR wants them, in whole milliseconds; `-1`
/// stands for a step that was not taken.
pub(crate) struct HarTimings {
    pub blocked: i64,
    pub dns: i64,
    pub connect: i64,
    pub ssl: i64,
    pub send: i64,
    pub wait: i64,
    pub receive: i64,
}

/// The timings of one entry and their sum, `time` (`phase 4 M4 design 8.3`).
/// The lookups the trail kept are those of the rules as well as of the dial,
/// and both happen before the connection is usable, so they are pressed into
/// the step they happened in: the connect time when there was a connection,
/// the whole request otherwise.
pub(crate) fn timings(t: &Timings) -> (HarTimings, i64) {
    let reach = t.connect.unwrap_or(t.total);
    let dns = t.dns.map(|d| d.min(reach));
    let pressed = dns.unwrap_or_default();
    let dialled = t
        .connect
        .filter(|_| !t.reused)
        .map(|c| c.saturating_sub(pressed));
    let borrowed = t
        .connect
        .filter(|_| t.reused)
        .map(|c| c.saturating_sub(pressed));
    let wait = match (t.connect, t.wait) {
        (Some(_), Some(w)) => w,
        (Some(c), None) => t.total.saturating_sub(c),
        (None, _) => t.total.saturating_sub(pressed),
    };
    let h = HarTimings {
        blocked: borrowed.map(ms).unwrap_or(-1),
        dns: dns.map(ms).unwrap_or(-1),
        connect: dialled.map(ms).unwrap_or(-1),
        // the handshake is inside `connect` here and not listed on its own
        ssl: -1,
        // rurge does not time the sending apart, it is inside `wait`
        send: 0,
        wait: ms(wait),
        receive: t.receive.map(ms).unwrap_or(0),
    };
    let steps = [
        h.blocked, h.dns, h.connect, h.ssl, h.send, h.wait, h.receive,
    ];
    let time = steps.iter().filter(|step| **step != -1).sum();
    (h, time)
}

/// One entry as a HAR entry (`phase 4 M4 design 8.2`).
pub(crate) fn entry(e: &Entry, body_limit: u64) -> Value {
    let (h, time) = timings(&e.timings);
    let mut value = json!({
        "startedDateTime": e.started_rfc3339(),
        "time": time,
        "request": request(e),
        "response": response(e, body_limit),
        "cache": {},
        "timings": {
            "blocked": h.blocked,
            "dns": h.dns,
            "connect": h.connect,
            "ssl": h.ssl,
            "send": h.send,
            "wait": h.wait,
            "receive": h.receive,
        },
        "_rurgeId": e.id,
        "_session": e.session.as_ref(),
        "_outcome": outcome_name(e.outcome),
        "_notes": &e.notes,
    });
    if let Some(ip) = e.remote.map(|addr| addr.ip().to_string()) {
        value["serverIPAddress"] = json!(ip);
    }
    if let Some(kind) = reject_kind(e.outcome) {
        value["_rejectKind"] = json!(kind);
    }
    if let Some(error) = &e.error {
        value["_error"] = json!(error);
    }
    value
}

/// One request (`phase 4 M4 design 8.2`).
fn request(e: &Entry) -> Value {
    let head = &e.request;
    let mut value = json!({
        "method": head.method.as_str(),
        "url": head.url,
        "httpVersion": format!("{:?}", head.version),
        // `cookie` and `set-cookie` are masked, so a cookie is never read out
        // of one of them
        "cookies": [],
        "headers": headers_json(&head.headers),
        "queryString": query_string(&head.method, &head.url),
        "headersSize": -1,
        "bodySize": e.request_body.size,
    });
    // an entry with no byte of a request body has no `postData` at all
    if e.request_body.size > 0 {
        value["postData"] = post_data(&head.headers, &e.request_body);
    }
    value
}

/// The query of a URL as names and values; a `CONNECT` has a `host:port` for
/// a URL and no query to split (`phase 4 M4 design 8.2`).
fn query_string(method: &axum::http::Method, url: &str) -> Value {
    if *method == axum::http::Method::CONNECT {
        return json!([]);
    }
    let Ok(parsed) = Url::parse(url) else {
        return json!([]);
    };
    Value::Array(
        parsed
            .query_pairs()
            .map(|(name, value)| json!({ "name": name, "value": value }))
            .collect(),
    )
}

/// The request body as `postData`: the text itself only when it was kept
/// whole, is no UTF-8 and has no encoding on top of it; Base64 otherwise,
/// which HAR 1.2 spells `_encoding` here (`phase 4 M4 design 8.2`).
fn post_data(headers: &HeaderMap, body: &CapturedBody) -> Value {
    let mut value = json!({
        "mimeType": header_text(headers, CONTENT_TYPE),
    });
    let readable = body.truncated.is_none() && !headers.contains_key(CONTENT_ENCODING);
    if let (true, Ok(text)) = (readable, std::str::from_utf8(&body.data)) {
        value["text"] = json!(text);
    } else {
        value["text"] = json!(STANDARD.encode(&body.data));
        value["_encoding"] = json!("base64");
    }
    if let Some(comment) = stored_comment(body) {
        value["comment"] = json!(comment);
    }
    value
}

/// One response, or the way mitmproxy writes a request the engine never
/// answered (`phase 4 M4 design 8.2`).
fn response(e: &Entry, body_limit: u64) -> Value {
    let Some(head) = e.response.as_ref() else {
        return json!({
            "status": 0,
            "statusText": "",
            "httpVersion": "",
            "cookies": [],
            "headers": [],
            "content": { "size": 0, "mimeType": "" },
            "redirectURL": "",
            "headersSize": -1,
            "bodySize": -1,
        });
    };
    let body = &e.response_body;
    let decoded = body.decoded(&head.headers, body_limit);
    let mut content = json!({
        "mimeType": header_text(&head.headers, CONTENT_TYPE),
    });
    match &decoded {
        Ok(Some(decoded)) => {
            content["size"] = json!(decoded.len());
            // what the transfer saved; negative when it grew
            content["compression"] = json!(decoded.len() as i64 - body.size as i64);
            put_text(&mut content, decoded);
        }
        Ok(None) | Err(_) => {
            content["size"] = json!(body.data.len());
            put_text(&mut content, &body.data);
        }
    }
    // what was left out first, then why it cannot be read
    let mut comment = stored_comment(body).unwrap_or_default();
    if let Err(reason) = &decoded {
        if !comment.is_empty() {
            comment.push_str("; ");
        }
        comment.push_str(&format!("not decoded: {reason}"));
    }
    if !comment.is_empty() {
        content["comment"] = json!(comment);
    }
    json!({
        "status": head.status.as_u16(),
        "statusText": head.status.canonical_reason().unwrap_or(""),
        "httpVersion": format!("{:?}", head.version),
        // `cookie` and `set-cookie` are masked, so a cookie is never read out
        // of one of them
        "cookies": [],
        "headers": headers_json(&head.headers),
        "content": content,
        "redirectURL": header_text(&head.headers, LOCATION),
        "headersSize": -1,
        "bodySize": body.size,
    })
}

/// A body as `text`: itself when it is UTF-8, Base64 and the `encoding`
/// marker otherwise (`phase 4 M4 design 8.2`).
fn put_text(content: &mut Value, bytes: &[u8]) {
    match std::str::from_utf8(bytes) {
        Ok(text) => content["text"] = json!(text),
        Err(_) => {
            content["text"] = json!(STANDARD.encode(bytes));
            content["encoding"] = json!("base64");
        }
    }
}

/// What was left out of a body, when the store kept only a part of it.
fn stored_comment(body: &CapturedBody) -> Option<String> {
    body.truncated
        .map(|_| format!("{} of {} bytes stored", body.data.len(), body.size))
}

/// The first value of a header, when it is there and readable.
fn header_text(headers: &HeaderMap, name: axum::http::header::HeaderName) -> &str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

/// A whole session as a HAR document: `{"log": …}`, its entries oldest first,
/// each serialized when the stream asks for it (`phase 4 M4 design 8.4`).
pub(crate) fn document(entries: Vec<Arc<Entry>>, body_limit: u64) -> Body {
    let opening = format!(
        "{{\"log\":{{\"version\":\"1.2\",\"creator\":{{\"name\":\"rurge\",\"version\":\"{}\"}},\"pages\":[],\"entries\":[",
        env!("CARGO_PKG_VERSION")
    );
    let opening = std::iter::once(Ok::<Vec<u8>, Infallible>(opening.into_bytes()));
    // One entry at a time: an entry is serialized when the stream is polled
    // for it, so besides the snapshot the export holds one entry's JSON and
    // no more, however many entries there are.
    let body = entries.into_iter().enumerate().map(move |(i, e)| {
        let mut bytes = Vec::new();
        if i > 0 {
            bytes.push(b',');
        }
        if serde_json::to_writer(&mut bytes, &entry(&e, body_limit)).is_err() {
            // `entry` makes a value, which always serializes; a document that
            // cannot be finished still has to be a document
            bytes.truncate(usize::from(i > 0));
            bytes.extend_from_slice(b"null");
        }
        Ok::<Vec<u8>, Infallible>(bytes)
    });
    let closing = std::iter::once(Ok::<Vec<u8>, Infallible>(b"]}}".to_vec()));
    Body::from_stream(futures_util::stream::iter(
        opening.chain(body).chain(closing),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Version};
    use bytes::Bytes;
    use rurge_engine::capture::{
        Body as CapturedBody, CaptureStore, Head, Limits, Outcome, ResponseHead,
    };
    use std::time::{Duration, UNIX_EPOCH};

    fn millis_of(d: Duration) -> i64 {
        d.as_millis() as i64
    }

    fn taken(
        connect: Option<Duration>,
        dns: Option<Duration>,
        wait: Option<Duration>,
        receive: Option<Duration>,
        total: Duration,
        reused: bool,
    ) -> Timings {
        Timings {
            connect,
            dns,
            wait,
            receive,
            total,
            reused,
        }
    }

    fn seconds(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// An entry of session `a` whose response body is `kept`, everything else
    /// the same in every case here.
    fn kept_entry(id: u64) -> Entry {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain"),
        );
        Entry {
            id,
            session: Arc::from("a"),
            started: UNIX_EPOCH + Duration::from_millis(1_761_984_000_000),
            request: Head {
                method: Method::GET,
                url: "http://a.test/one".to_string(),
                version: Version::HTTP_11,
                headers: HeaderMap::new(),
            },
            request_body: CapturedBody::empty(),
            response: Some(ResponseHead {
                status: StatusCode::OK,
                version: Version::HTTP_11,
                headers,
            }),
            response_body: CapturedBody {
                data: Bytes::from_static(b"kept"),
                size: 4,
                truncated: None,
            },
            timings: taken(
                Some(seconds(3)),
                None,
                Some(seconds(20)),
                Some(seconds(12)),
                seconds(35),
                false,
            ),
            remote: None,
            outcome: Outcome::Completed,
            error: None,
            rule: None,
            policy: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Every step of the HAR timings comes out of the request's own times, and
    /// they add up to `time` (`phase 4 M4 design 8.3`): the lookups are
    /// pressed into the part of the request they happened in, because they run
    /// for the rules as well as for the dial and may overlap.
    #[test]
    fn har_timings_add_up_to_the_total() {
        // A connection dialled for this request: the lookups come out of the
        // connect time.
        let (h, time) = timings(&taken(
            Some(seconds(30)),
            Some(seconds(10)),
            Some(seconds(20)),
            Some(seconds(5)),
            seconds(55),
            false,
        ));
        assert_eq!(
            (
                h.blocked, h.dns, h.connect, h.ssl, h.send, h.wait, h.receive
            ),
            (-1, 10, 20, -1, 0, 20, 5),
            "a dialled connection"
        );
        assert_eq!(time, 55, "a dialled connection");

        // A borrowed one: nothing was dialled, so the connect time is the
        // wait for the connection instead.
        let (h, time) = timings(&taken(
            Some(seconds(4)),
            Some(seconds(3)),
            Some(seconds(10)),
            Some(seconds(2)),
            seconds(16),
            true,
        ));
        assert_eq!(
            (
                h.blocked, h.dns, h.connect, h.ssl, h.send, h.wait, h.receive
            ),
            (1, 3, -1, -1, 0, 10, 2),
            "a borrowed connection"
        );
        assert_eq!(time, 16, "a borrowed connection");

        // Never connected: the lookup is pressed into the whole request and
        // the rest of it is the wait.
        let (h, time) = timings(&taken(
            None,
            Some(seconds(7)),
            None,
            None,
            seconds(12),
            false,
        ));
        assert_eq!(
            (
                h.blocked, h.dns, h.connect, h.ssl, h.send, h.wait, h.receive
            ),
            (-1, 7, -1, -1, 0, 5, 0),
            "no connection"
        );
        assert_eq!(time, 12, "no connection");

        // Connected, but no first byte came: the wait is what is left of the
        // request after the connection.
        let (h, time) = timings(&taken(
            Some(seconds(8)),
            None,
            None,
            None,
            seconds(20),
            false,
        ));
        assert_eq!(
            (
                h.blocked, h.dns, h.connect, h.ssl, h.send, h.wait, h.receive
            ),
            (-1, -1, 8, -1, 0, 12, 0),
            "connected and never answered"
        );
        assert_eq!(time, 20, "connected and never answered");

        // The lookups for the rules took longer than the dial: what is left of
        // the connect time is nothing, not less than nothing.
        let (h, time) = timings(&taken(
            Some(seconds(30)),
            Some(seconds(50)),
            Some(seconds(1)),
            Some(seconds(1)),
            seconds(32),
            false,
        ));
        assert_eq!(
            (
                h.blocked, h.dns, h.connect, h.ssl, h.send, h.wait, h.receive
            ),
            (-1, 30, 0, -1, 0, 1, 1),
            "lookups longer than the dial"
        );
        assert_eq!(time, 32, "lookups longer than the dial");

        // With fractions of a millisecond each step is rounded down on its
        // own, so `time` is the sum of the steps and the whole request is at
        // most a millisecond per step more than it.
        let t = taken(
            Some(Duration::from_micros(30_600)),
            Some(Duration::from_micros(10_400)),
            Some(Duration::from_micros(20_900)),
            Some(Duration::from_micros(5_500)),
            Duration::from_micros(56_000),
            false,
        );
        let (h, time) = timings(&t);
        let steps = [
            h.blocked, h.dns, h.connect, h.ssl, h.send, h.wait, h.receive,
        ];
        let sum: i64 = steps.iter().filter(|step| **step != -1).sum();
        assert_eq!(time, sum, "the steps add up to `time`");
        assert_eq!(
            (h.dns, h.connect, h.wait, h.receive),
            (10, 20, 20, 5),
            "each step is rounded down on its own"
        );
        let whole = millis_of(t.total);
        assert!(
            (0..=3).contains(&(whole - time)),
            "the whole request is {whole} ms and `time` is {time} ms"
        );
    }

    /// The export holds the entries of its snapshot while they are written
    /// (`phase 4 M4 design 8.4`): what is cleared in between is still in the
    /// document, whole.
    #[tokio::test]
    async fn an_export_holds_its_entries_while_they_are_cleared() {
        use http_body_util::BodyExt;

        let store = CaptureStore::new(Limits::default());
        for id in 1..=3 {
            assert!(store.put(kept_entry(id)), "entry {id} fits");
        }
        let entries = store.session("a");
        assert_eq!(entries.len(), 3);
        let body = document(entries, Limits::default().body);
        // cleared before a single frame of the export is polled
        assert_eq!(store.clear(None), 3);
        assert!(store.session("a").is_empty());

        let bytes = body
            .collect()
            .await
            .expect("the export is a stream in memory")
            .to_bytes();
        let doc: Value = serde_json::from_slice(&bytes).expect("a JSON document");
        let listed = doc["log"]["entries"]
            .as_array()
            .expect("the document lists entries");
        assert_eq!(listed.len(), 3, "{doc}");
        for (i, entry) in listed.iter().enumerate() {
            assert_eq!(entry["_rurgeId"], (i + 1) as u64, "{doc}");
            assert_eq!(entry["response"]["content"]["size"], 4, "{doc}");
            assert_eq!(entry["response"]["content"]["text"], "kept", "{doc}");
        }
    }
}
