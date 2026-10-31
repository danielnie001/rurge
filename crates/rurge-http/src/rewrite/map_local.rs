//! `[Map Local]`'s rules and the answer one of them gives (`phase 4 M3 design
//! 10`): the data a line answers with, the headers its answer carries, and
//! which rule answers a URL.
//!
//! Nothing here opens a file or asks for data: the engine reads a file's bytes
//! when a request arrives, and a URL's data comes from the resource manager
//! through `UrlData` (the phase 4 / M3b plan's P15, P19). Nothing of the data
//! reaches a note: a note names the rule's expression and the kind of data
//! (`phase 4 M3 design 8.5`).

use crate::rewrite::{Note, Rewrite, UrlData};
use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use rurge_config::rewrite::{MapData, MapLocal};
use std::path::PathBuf;

/// What a `[Map Local]` rule answers with (`phase 4 M3 design 10.3`).
pub(crate) enum MapSource {
    /// A local file, opened when a request arrives: its bytes are not read
    /// here.
    File(PathBuf),
    /// The data of a URL, from the resource manager. `origin` is what a note,
    /// a log line or a `Debug` may show of it: never a path or a query
    /// (`phase 4 M3 design 8.5`).
    Url { data: UrlData, origin: String },
    /// The bytes a line writes itself: `text`, `base64` or `tiny-gif`.
    Bytes(Bytes),
}

/// One `[Map Local]` line, ready to answer.
pub(crate) struct MapRule {
    pattern: rurge_config::rule::Pattern,
    source: MapSource,
    status: StatusCode,
    /// The headers of the answer: the hop-by-hop ones gone, `Content-Type`
    /// there, no `Content-Length` (`phase 4 M3 design 10.4`).
    headers: HeaderMap,
    kind: &'static str,
}

impl MapRule {
    /// The rule's URL expression as the profile wrote it, which is what a note
    /// names the rule by.
    pub(crate) fn source(&self) -> &str {
        &self.pattern.source
    }

    /// What the answer's body is.
    pub(crate) fn data(&self) -> &MapSource {
        &self.source
    }

    /// The status of the answer.
    pub(crate) fn status(&self) -> StatusCode {
        self.status
    }

    /// The headers of the answer, in the order the line wrote them.
    pub(crate) fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// The `data-type` a note names it by: `file`, `text`, `base64` or
    /// `tiny-gif`.
    pub(crate) fn kind(&self) -> &'static str {
        self.kind
    }
}

/// One `[Map Local]` line, ready to answer; `url_data` is how the line's URL
/// comes by its bytes, asked once when the generation is built (`phase 4 M3
/// design 10.3`, the phase 4 / M3b plan's P19).
pub(super) fn map_rule(rule: &MapLocal, url_data: &dyn Fn(&str) -> UrlData) -> MapRule {
    let (source, default_type) = match &rule.data {
        MapData::File(path) => (
            MapSource::File(path.clone()),
            content_type_for(&path.to_string_lossy()),
        ),
        MapData::Url(url) => (
            MapSource::Url {
                data: url_data(url.as_str()),
                origin: rurge_config::remote::origin(url),
            },
            content_type_for(url.path()),
        ),
        MapData::Text(text) => (
            MapSource::Bytes(Bytes::copy_from_slice(text.as_bytes())),
            "text/plain",
        ),
        MapData::Base64(data) => (
            MapSource::Bytes(Bytes::copy_from_slice(data)),
            "application/octet-stream",
        ),
        MapData::TinyGif => (
            MapSource::Bytes(Bytes::from_static(&rurge_inbound::responses::TINY_GIF)),
            "image/gif",
        ),
    };
    let mut headers = HeaderMap::new();
    for (name, value) in &rule.headers {
        let Some(name) = field_name(name) else {
            continue;
        };
        let Some(value) = field_value(&name, value) else {
            continue;
        };
        headers.append(name, value);
    }
    // the answer's framing is the length of its data, so nothing of the
    // client's hop is carried and `Content-Length` is left to the engine
    crate::headers::strip(&mut headers, crate::headers::Keep::NONE);
    if !headers.contains_key(header::CONTENT_TYPE) {
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(default_type));
    }
    MapRule {
        pattern: rule.pattern.clone(),
        source,
        // the loader keeps 200 to 999, which `StatusCode` takes; a value from
        // anywhere else is not worth a panic
        status: StatusCode::from_u16(rule.status).unwrap_or(StatusCode::OK),
        headers,
        kind: rule.data.kind(),
    }
}

impl Rewrite {
    /// The first rule whose URL expression matches `url`, in the order written
    /// (`phase 4 M3 design 10.2`, the phase 4 / M3b plan's P15). A rule whose
    /// expression runs out of backtracking steps is skipped with a note, and
    /// the next one is tried.
    pub(crate) fn map_local(&self, url: &str, notes: &mut Vec<Note>) -> Option<&MapRule> {
        for rule in &self.map_local {
            match rule.pattern.regex.is_match(url) {
                Ok(true) => return Some(rule),
                Ok(false) => {}
                Err(_) => notes.push(Note::Skipped(format!(
                    "Map Local skipped: {}: the regular expression hit its backtracking limit",
                    rule.source()
                ))),
            }
        }
        None
    }
}

/// The `Content-Type` a file or a URL's path is answered with, by the
/// extension of its last segment (`phase 4 M3 design 10.4`, the phase 4 / M3b
/// plan's P17).
///
/// The extension is what follows the last `.`, ASCII lowercase, and a name
/// whose only dot is its first character has none: `.hidden` is a name, not an
/// extension. A name the table does not know, and one with no extension, is
/// `application/octet-stream`. The type is written without a `charset`: the
/// table says what the bytes are, and the manual's own answers do not add one.
pub(crate) fn content_type_for(name: &str) -> &'static str {
    // a file's path and a URL's path are both cut at the last separator: on a
    // Windows path that is a backslash, in a URL a slash
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let extension = match last.rfind('.') {
        Some(dot) if dot > 0 => last[dot + 1..].to_ascii_lowercase(),
        _ => String::new(),
    };
    match extension.as_str() {
        "aac" => "audio/aac",
        "apng" => "image/apng",
        "avif" => "image/avif",
        "bin" => "application/octet-stream",
        "bmp" => "image/bmp",
        "css" => "text/css",
        "csv" => "text/csv",
        "gif" => "image/gif",
        "gz" => "application/gzip",
        "htm" | "html" => "text/html",
        "ico" => "image/vnd.microsoft.icon",
        "jpeg" | "jpg" => "image/jpeg",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "jsonld" => "application/ld+json",
        "m3u8" => "application/vnd.apple.mpegurl",
        "m4a" => "audio/mp4",
        "md" => "text/markdown",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "mpeg" => "video/mpeg",
        "oga" | "opus" => "audio/ogg",
        "ogv" => "video/ogg",
        "otf" => "font/otf",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "tif" | "tiff" => "image/tiff",
        "ts" => "video/mp2t",
        "ttf" => "font/ttf",
        "txt" => "text/plain",
        "wasm" => "application/wasm",
        "wav" => "audio/wav",
        "weba" => "audio/webm",
        "webm" => "video/webm",
        "webmanifest" => "application/manifest+json",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "xhtml" => "application/xhtml+xml",
        "xml" => "application/xml",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// The field name a line wrote, as the header table takes it. The loader has
/// checked it, so a name this build still cannot take is dropped and only the
/// name is said, never a value.
fn field_name(name: &str) -> Option<HeaderName> {
    match HeaderName::from_bytes(name.as_bytes()) {
        Ok(name) => Some(name),
        Err(_) => {
            tracing::debug!(
                name,
                "a [Map Local] header names a field this build cannot carry; the header is dropped"
            );
            None
        }
    }
}

/// The value a line wrote, as the header table takes it. Nothing of the value
/// is said, and it stays out of every note (`phase 4 M3 design 8.5`).
fn field_value(name: &HeaderName, value: &str) -> Option<HeaderValue> {
    match HeaderValue::from_bytes(value.as_bytes()) {
        Ok(value) => Some(value),
        Err(_) => {
            tracing::debug!(
                name = %name,
                "a [Map Local] header carries a value this build cannot take; the header is dropped"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rewrite::{RewriteCounters, config_of, test_roots};
    use http::header;
    use rurge_config::mitm::MitmConfig;
    use rurge_config::rewrite::RewriteConfig;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    /// A generation of the `[Map Local]` section of `cfg`, and the URLs it
    /// asked `url_data` for as it was built.
    fn recording(cfg: &RewriteConfig) -> (Rewrite, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        let rewrite = Rewrite::build(
            cfg,
            &MitmConfig::default(),
            test_roots(),
            Arc::new(RewriteCounters::default()),
            &move |url: &str| {
                recorder.lock().expect("the lock").push(url.to_string());
                Arc::new(|| None) as UrlData
            },
        )
        .expect("a generation")
        .expect("the lines make rules");
        (rewrite, seen)
    }

    /// The rule that answers `url`, and the notes it took to find it.
    fn answer<'a>(rewrite: &'a Rewrite, url: &str) -> (Option<&'a MapRule>, Vec<Note>) {
        let mut notes = Vec::new();
        let rule = rewrite.map_local(url, &mut notes);
        (rule, notes)
    }

    /// The `Content-Type` of the answer a rule gives.
    fn content_type(rewrite: &Rewrite, url: &str) -> String {
        let (rule, notes) = answer(rewrite, url);
        assert!(notes.is_empty(), "{notes:?}");
        let rule = rule.expect("a rule matches");
        rule.headers()[header::CONTENT_TYPE]
            .to_str()
            .expect("a visible value")
            .to_string()
    }

    /// The first rule whose expression matches answers, and one whose
    /// expression runs out of backtracking steps is passed over with a note
    /// (`phase 4 M3 design 10.2`, the phase 4 / M3b plan's P15).
    #[test]
    fn the_first_matching_rule_answers() {
        let lines = "[Map Local]\n\
                     ^http://a\\.test/ data-type=text data=first\n\
                     ^http://a\\.test/ data-type=text data=second";
        let (rewrite, _) = recording(&config_of(lines));
        let (rule, notes) = answer(&rewrite, "http://a.test/p");
        let rule = rule.expect("both rules match and the first answers");
        match rule.data() {
            MapSource::Bytes(bytes) => assert_eq!(&bytes[..], b"first"),
            _ => panic!("a text rule answers with its bytes"),
        }
        assert!(notes.is_empty(), "{notes:?}");

        let lines = "[Map Local]\n\
                     (?i)(a|b|ab)*(?=c) data-type=text data=limit\n\
                     ^http://a\\.test/ data-type=text data=second";
        let cfg = config_of(lines);
        let (rewrite, _) = recording(&cfg);
        // the first rule's expression really does give up on this input
        let url = format!("http://a.test/{}", "ab".repeat(40));
        assert!(
            cfg.map_local[0].pattern.regex.is_match(&url).is_err(),
            "the expression gives up on this input"
        );
        let (rule, notes) = answer(&rewrite, &url);
        let rule = rule.expect("the second rule matches");
        match rule.data() {
            MapSource::Bytes(bytes) => assert_eq!(&bytes[..], b"second"),
            _ => panic!("a text rule answers with its bytes"),
        }
        assert_eq!(
            notes,
            [Note::Skipped(
                "Map Local skipped: (?i)(a|b|ab)*(?=c): the regular expression hit its backtracking limit"
                    .to_string()
            )]
        );
    }

    /// The extension of a name picks its type, ASCII lowercase, from the table
    /// of phase 4 M3 design 10.4; a name the table does not know, and one with
    /// no extension, is `application/octet-stream` (the phase 4 / M3b plan's
    /// P17).
    #[test]
    fn content_types_follow_the_extension() {
        for (name, expected) in [
            ("a.json", "application/json"),
            ("A.JSON", "application/json"),
            ("x.tar.gz", "application/gzip"),
            ("noext", "application/octet-stream"),
            (".hidden", "application/octet-stream"),
            ("a.unknownext", "application/octet-stream"),
        ] {
            assert_eq!(content_type_for(name), expected, "{name}");
        }

        // the table as the plan writes it, one name per entry
        for (extension, expected) in [
            ("aac", "audio/aac"),
            ("apng", "image/apng"),
            ("avif", "image/avif"),
            ("bin", "application/octet-stream"),
            ("bmp", "image/bmp"),
            ("css", "text/css"),
            ("csv", "text/csv"),
            ("gif", "image/gif"),
            ("gz", "application/gzip"),
            ("htm", "text/html"),
            ("html", "text/html"),
            ("ico", "image/vnd.microsoft.icon"),
            ("jpeg", "image/jpeg"),
            ("jpg", "image/jpeg"),
            ("js", "text/javascript"),
            ("mjs", "text/javascript"),
            ("json", "application/json"),
            ("jsonld", "application/ld+json"),
            ("m3u8", "application/vnd.apple.mpegurl"),
            ("m4a", "audio/mp4"),
            ("md", "text/markdown"),
            ("mp3", "audio/mpeg"),
            ("mp4", "video/mp4"),
            ("mpeg", "video/mpeg"),
            ("oga", "audio/ogg"),
            ("opus", "audio/ogg"),
            ("ogv", "video/ogg"),
            ("otf", "font/otf"),
            ("pdf", "application/pdf"),
            ("png", "image/png"),
            ("svg", "image/svg+xml"),
            ("tif", "image/tiff"),
            ("tiff", "image/tiff"),
            ("ts", "video/mp2t"),
            ("ttf", "font/ttf"),
            ("txt", "text/plain"),
            ("wasm", "application/wasm"),
            ("wav", "audio/wav"),
            ("weba", "audio/webm"),
            ("webm", "video/webm"),
            ("webmanifest", "application/manifest+json"),
            ("webp", "image/webp"),
            ("woff", "font/woff"),
            ("woff2", "font/woff2"),
            ("xhtml", "application/xhtml+xml"),
            ("xml", "application/xml"),
            ("zip", "application/zip"),
        ] {
            let name = format!("x.{extension}");
            assert_eq!(content_type_for(&name), expected, "{extension}");
        }
    }

    /// A line with no `Content-Type` is given the one of its `data-type`, a
    /// file and a URL by their names, and one that wrote its own keeps it
    /// (`phase 4 M3 design 10.4`, the phase 4 / M3b plan's P17).
    #[test]
    fn default_content_types() {
        let lines = "[Map Local]\n\
                     ^http://a\\.test/text data-type=text data=hi\n\
                     ^http://a\\.test/gif data-type=tiny-gif\n\
                     ^http://a\\.test/b64 data-type=base64 data=aGk=\n\
                     ^http://a\\.test/file data-type=file data=data/x.webp\n\
                     ^http://a\\.test/url data-type=file data=https://a.test/p/x.svg\n\
                     ^http://a\\.test/kept data-type=text data=hi header=\"content-type:x/y\"";
        let (rewrite, _) = recording(&config_of(lines));
        for (url, expected) in [
            ("http://a.test/text", "text/plain"),
            ("http://a.test/gif", "image/gif"),
            ("http://a.test/b64", "application/octet-stream"),
            ("http://a.test/file", "image/webp"),
            ("http://a.test/url", "image/svg+xml"),
            ("http://a.test/kept", "x/y"),
        ] {
            assert_eq!(content_type(&rewrite, url), expected, "{url}");
        }
    }

    /// The headers of an answer lose the hop-by-hop ones and gain the
    /// `Content-Type` the line did not write (`phase 4 M3 design 10.4`).
    #[test]
    fn map_local_headers_lose_the_hop_by_hop_ones() {
        let lines = "[Map Local]\n\
                     ^http://a\\.test/ data-type=text data=hi header=\"Connection:close|Keep-Alive:x|X-A:1\"";
        let (rewrite, _) = recording(&config_of(lines));
        let (rule, notes) = answer(&rewrite, "http://a.test/p");
        assert!(notes.is_empty(), "{notes:?}");
        let headers = rule.expect("a rule matches").headers();
        let mut names: Vec<&str> = headers.keys().map(|name| name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["content-type", "x-a"]);
        assert_eq!(headers["x-a"], "1");
        assert_eq!(headers[header::CONTENT_TYPE], "text/plain");
        assert!(!headers.contains_key(header::CONTENT_LENGTH));
        assert!(!headers.contains_key(header::TRANSFER_ENCODING));
    }

    /// Each `data-type` gives the body the manual says it does, and a URL is
    /// asked for its data once, when the generation is built (`phase 4 M3
    /// design 10.3`, the phase 4 / M3b plan's P15).
    #[test]
    fn the_data_of_each_type() {
        let lines = "[Map Local]\n\
                     ^http://a\\.test/text data-type=text data=hi\n\
                     ^http://a\\.test/b64 data-type=base64 data=aGk=\n\
                     ^http://a\\.test/gif data-type=tiny-gif\n\
                     ^http://a\\.test/file data-type=file data=data/x.json\n\
                     ^http://a\\.test/url data-type=file data=https://a.test/p/x.json";
        let (rewrite, seen) = recording(&config_of(lines));

        for (url, expected) in [
            ("http://a.test/text", &b"hi"[..]),
            ("http://a.test/b64", &b"hi"[..]),
            ("http://a.test/gif", &rurge_inbound::responses::TINY_GIF[..]),
        ] {
            let (rule, _) = answer(&rewrite, url);
            match rule.expect("a rule matches").data() {
                MapSource::Bytes(bytes) => assert_eq!(&bytes[..], expected, "{url}"),
                _ => panic!("{url} answers with its own bytes"),
            }
        }

        let (rule, _) = answer(&rewrite, "http://a.test/file");
        match rule.expect("a rule matches").data() {
            MapSource::File(path) => assert_eq!(path, Path::new("data/x.json")),
            _ => panic!("a file rule answers with a path"),
        }

        let (rule, _) = answer(&rewrite, "http://a.test/url");
        match rule.expect("a rule matches").data() {
            MapSource::Url { origin, .. } => assert_eq!(origin, "https://a.test/…"),
            _ => panic!("a URL rule answers with the data of its URL"),
        }
        let seen = seen.lock().expect("the lock");
        assert_eq!(*seen, ["https://a.test/p/x.json".to_string()]);
    }
}
