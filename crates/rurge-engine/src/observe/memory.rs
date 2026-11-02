//! What a request log costs in memory at its default capacity. A measurement,
//! not a gate: the two tests that measure are ignored, and the one that reads
//! the peak is the guard of the reading itself. Each of the two prints what
//! its log holds and the peak working set of the process, taken from the
//! operating system while the test runs (`PeakWorkingSet64` of `Get-Process`
//! on Windows, `VmHWM` in `/proc/<pid>/status` on Linux), so every test is run
//! alone, in a debug build:
//!
//! ```text
//! cargo test -j 8 --workspace --lib -- observe::memory::<name> --ignored --exact --nocapture
//! ```
//!
//! A debug build, not release (`the phase 4 / M4b plan's P12`): release would
//! build the whole workspace a second time, and a record, its strings and its
//! vectors allocate the same bytes in either build — what is measured is the
//! difference between a full log and an empty one.
//!
//! `an_empty_request_log_is_the_baseline` is the same test binary run alone,
//! with a log that holds nothing: what the process costs before any record.
//! Nothing here prints a record: the numbers only (`the phase 4 / M4b plan's
//! P12`).

use super::*;
use rurge_config::session::SessionInfo;

/// A session shaped like an HTTP request, as the engine makes one: a name and
/// a port for the destination, the URL the engine was handed, the SNI a sniff
/// read, the rule and the policy chain that matched, one remark, and the
/// counts a finished request has (`the phase 4 / M4b plan's P12`).
fn handle(id: u64) -> Arc<SessionHandle> {
    let host = format!("host-{id:06}.measurement.test");
    let mut s = SessionInfo::tcp(HostName::parse(&host), 443);
    s.listener = ListenerKind::Http;
    s.url = Some(format!(
        "http://{host}/a-folder-for-the-measurement/page-{id}.html?of=records"
    ));
    s.sni = Some(format!("sni-{id:06}.example.test"));
    let h = SessionHandle::new(id, s);
    h.set_rule(Some(format!("DOMAIN,{host},DIRECT")));
    h.set_policy_chain(vec!["DIRECT".to_string(), "Proxy".to_string()]);
    h.add_note(format!("Sub-rule matched: DOMAIN,{host}"));
    h.set_method("GET");
    h.set_status(200);
    h.mark_connected();
    h.mark_first_byte();
    h.add_up(512);
    h.add_down(4096);
    // a GET carries no body; the counter is still one a record reads
    h.add_request_body(0);
    h.add_response_body(4096);
    h
}

/// The peak working set of this process so far, in bytes, from the operating
/// system; `None` where this does not read it. Copied from
/// `crates/rurge-http/src/capture/memory.rs`.
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
/// `crates/rurge-http/src/capture/memory.rs`.
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
/// from `crates/rurge-http/src/capture/memory.rs`.
fn parse_vm_hwm(status: &str) -> Option<u64> {
    let line = status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))?;
    let kib: u64 = line.trim().strip_suffix("kB")?.trim().parse().ok()?;
    Some(kib * 1024)
}

/// A log at the default capacity, filled with more than it holds so the
/// oldest records are evicted, and the peak this process reached.
///
/// What to read: this test's peak less `an_empty_request_log_is_the_baseline`'
/// peak, divided by the capacity — one record's bytes (`the phase 4 / M4b
/// plan's P12`). Every handle is dropped as soon as its record is made, so the
/// peak measures the records and not the sessions that made them.
#[test]
#[ignore = "a measurement, not a gate: run on its own, in a debug build"]
fn a_full_request_log_of_http_requests() {
    let log = RequestLog::new(100_000);
    let traffic = TrafficStats::new();
    for id in 0..110_000u64 {
        let h = handle(id);
        log.mark_active(&h);
        h.finish(SessionOutcome::Completed);
        log.record_finished(&h, &SessionOutcome::Completed, &traffic);
    }
    // the peak is read before anything else is: `recent` would copy every
    // record and raise it (`the phase 4 / M4b plan's execution record row 61`)
    let peak = peak_resident_bytes();
    println!("peak resident bytes: {peak:?}");
    let held = log.len();
    println!("records held: {held}");
    assert_eq!(held, 100_000, "the log keeps its capacity and no more");
}

/// The same test binary, run alone, with a log that holds nothing (`the phase
/// 4 / M4b plan's P12`).
#[test]
#[ignore = "a measurement, not a gate: run on its own, in a debug build"]
fn an_empty_request_log_is_the_baseline() {
    let log = RequestLog::new(100_000);
    let peak = peak_resident_bytes();
    println!("peak resident bytes: {peak:?}");
    assert!(log.is_empty(), "the baseline log holds nothing");
}

/// The peak is read from what the system prints. Copied from
/// `crates/rurge-http/src/capture/memory.rs`.
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
