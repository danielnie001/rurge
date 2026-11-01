//! What a capture store costs in memory at its default limits. A measurement,
//! not a gate: the two tests that fill a store are ignored, and the one that
//! reads the peak is the guard of the reading itself. Each test that fills
//! prints what the store holds and the peak working set of the process, taken
//! from the operating system while the test runs (`PeakWorkingSet64` of
//! `Get-Process` on Windows, `VmHWM` in `/proc/<pid>/status` on Linux), so
//! every test is run alone, in a debug build:
//!
//! ```text
//! cargo test -j 8 --workspace --lib -- capture::memory::<name> --ignored --exact --nocapture
//! ```
//!
//! A debug build, not release (`the phase 4 / M4a plan's P15`): release would
//! build the whole workspace a second time, and an entry, a `HeaderMap` and
//! `Bytes` allocate the same bytes in either build — what is measured is the
//! difference between a full store and an empty one.
//!
//! `an_empty_store_is_the_baseline` is the same process with no entry in it.
//! Nothing here prints an entry's headers, URL or body: the numbers only.

use super::{Capture, CaptureStore, Entry, Head, Limits, Outcome, ResponseHead, Timings};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Version};
use std::sync::Arc;
use std::time::SystemTime;

/// How many names a header set of an entry has, on the request side and on the
/// response side (`the phase 4 / M4a plan's P15`).
const HEADERS: usize = 14;

/// A page's response body, and the chunks it arrives in.
const PAGE_BODY: usize = 4096;
const CHUNK: usize = 1024;

/// The response body of every thousand and twenty-fourth entry: six MiB, of
/// which the store keeps its default body limit of five MiB, chunk by chunk of
/// 64 KiB (`phase 4 M4 design 4.2`, `the phase 4 / M4a plan's P15`).
const BIG_BODY: usize = 6 * 1024 * 1024;
const BIG_CHUNK: usize = 64 * 1024;

/// Header line `i` of a set: a name of about twelve bytes and a value of about
/// forty. `offset` keeps the request's names apart from the response's.
fn header(i: usize, offset: usize) -> (HeaderName, HeaderValue) {
    let name = HeaderName::try_from(format!("x-capture-{:02}", i + offset)).expect("a name");
    let value = HeaderValue::from_str(&format!("{i:02} {}", "v".repeat(36))).expect("a value");
    (name, value)
}

/// `HEADERS` header lines, about as wide as a browser's.
fn headers(offset: usize) -> HeaderMap {
    let mut map = HeaderMap::new();
    for i in 0..HEADERS {
        let (name, value) = header(i, offset);
        map.insert(name, value);
    }
    map
}

/// The URL of entry `i`: about eighty bytes, as a page's is.
fn url(i: usize) -> String {
    format!(
        "http://example.test/{}-{i}.html?of=measurement",
        "a-page-in-a-long-folder".repeat(2)
    )
}

/// Where an entry came from, as an entry has one.
fn remote() -> Option<std::net::SocketAddr> {
    Some("127.0.0.1:8080".parse().expect("an address"))
}

/// Store `i` the way a request stores one: through `Capture::begin`, with the
/// response body fed in chunks, and committed with an entry like a page's
/// (`the phase 4 / M4a plan's P15`).
fn put_one(capture: &Capture, i: usize) {
    let mut pending = capture.begin();
    let (size, chunk) = if i % 1024 == 1023 {
        (BIG_BODY, BIG_CHUNK)
    } else {
        (PAGE_BODY, CHUNK)
    };
    let data = vec![b'x'; chunk];
    let mut fed = 0;
    while fed < size {
        pending.response_chunk(&data[..chunk.min(size - fed)]);
        fed += chunk;
    }
    let request_url = url(i);
    let request_headers = headers(0);
    let response_headers = headers(HEADERS);
    pending.commit(|request_body, response_body| Entry {
        id: i as u64,
        session: capture.session.clone(),
        started: SystemTime::now(),
        request: Head {
            method: Method::GET,
            url: request_url,
            version: Version::HTTP_11,
            headers: request_headers,
        },
        request_body,
        response: Some(ResponseHead {
            status: StatusCode::OK,
            version: Version::HTTP_11,
            headers: response_headers,
        }),
        response_body,
        timings: Timings {
            connect: None,
            dns: None,
            wait: None,
            receive: None,
            total: std::time::Duration::ZERO,
            reused: false,
        },
        remote: remote(),
        outcome: Outcome::Completed,
        error: None,
        rule: Some("DOMAIN,example.test,DIRECT".to_string()),
        policy: vec!["DIRECT".to_string()],
        notes: vec![format!("a note about entry {i}")],
    });
}

/// The peak working set of this process so far, in bytes, from the operating
/// system; `None` where this does not read it. Copied from
/// `crates/rurge-tun/src/tests/bench/peak.rs`.
fn peak_resident_bytes() -> Option<u64> {
    #[cfg(windows)]
    {
        // a read-only system tool, started by the test itself
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command"])
            .arg(format!(
                "(Get-Process -Id {}).PeakWorkingSet64",
                std::process::id()
            ))
            .output()
            .ok()?;
        parse_count(&String::from_utf8_lossy(&output.stdout))
    }
    #[cfg(target_os = "linux")]
    {
        parse_vm_hwm(&std::fs::read_to_string("/proc/self/status").ok()?)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

/// The number a command printed on its last line. Copied from
/// `crates/rurge-tun/src/tests/bench/peak.rs`.
fn parse_count(output: &str) -> Option<u64> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .next_back()?
        .parse()
        .ok()
}

/// `VmHWM` of `/proc/<pid>/status`, in bytes: its peak resident set. Copied
/// from `crates/rurge-tun/src/tests/bench/peak.rs`.
fn parse_vm_hwm(status: &str) -> Option<u64> {
    let line = status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))?;
    let kib: u64 = line.trim().strip_suffix("kB")?.trim().parse().ok()?;
    Some(kib * 1024)
}

/// A store filled at the default limits until the oldest entries start to be
/// evicted faster than one in ten, and the peak this process reached.
///
/// What to read: this test's peak less `an_empty_store_is_the_baseline`'s, and
/// that difference against the store's total (`the phase 4 / M4a plan's P15`).
#[test]
#[ignore = "a measurement, not a gate: run on its own, in a debug build"]
fn a_full_store_at_the_default_limits() {
    let store = CaptureStore::new(Limits::default());
    let capture = Capture {
        store: store.clone(),
        session: Arc::from("measurement"),
    };
    let mut made = 0usize;
    loop {
        put_one(&capture, made);
        made += 1;
        let status = store.status();
        if status.evicted > status.stored / 10 {
            break;
        }
    }
    let status = store.status();
    println!("{made} entries made");
    println!(
        "limits: {} total, {} a body; used {}, entries {}, stored {}, evicted {}, dropped {}, truncated {}",
        status.limits.total,
        status.limits.body,
        status.used,
        status.entries,
        status.stored,
        status.evicted,
        status.dropped,
        status.truncated
    );
    println!("peak resident bytes: {:?}", peak_resident_bytes());
    assert!(
        status.used <= status.limits.total,
        "the store never holds more than its total"
    );
    assert!(status.entries > 0, "a store that held nothing was filled");
    assert!(
        status.truncated > 0,
        "the entries over the body limit were truncated"
    );
}

/// The same process with no entry in it (`the phase 4 / M4a plan's P15`).
#[test]
#[ignore = "a measurement, not a gate: run on its own, in a debug build"]
fn an_empty_store_is_the_baseline() {
    let store = CaptureStore::new(Limits::default());
    let status = store.status();
    println!(
        "an empty store: limits {} total / {} a body, used {}, entries {}",
        status.limits.total, status.limits.body, status.used, status.entries
    );
    println!("peak resident bytes: {:?}", peak_resident_bytes());
}

/// The peak is read from what the system prints. Copied from
/// `crates/rurge-tun/src/tests/bench/peak.rs`.
#[test]
fn the_peak_is_read_from_what_the_system_prints() {
    assert_eq!(parse_count("123456789\r\n"), Some(123_456_789));
    assert_eq!(parse_count("\r\nwarning\r\n42\r\n\r\n"), Some(42));
    assert_eq!(parse_count(""), None);
    assert_eq!(parse_count("not a number"), None);
    let status = "Name:\trurge\nVmPeak:\t  900000 kB\nVmHWM:\t  123456 kB\nVmRSS:\t   100000 kB\n";
    assert_eq!(parse_vm_hwm(status), Some(123_456 * 1024));
    assert_eq!(parse_vm_hwm("Name:\trurge\n"), None);
    assert_eq!(parse_vm_hwm("VmHWM:\tmuch\n"), None);
}
