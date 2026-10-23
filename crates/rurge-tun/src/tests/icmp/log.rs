//! What is said of a failed echo request: at debug level, with the name or the
//! address, and nothing above debug level has either.

use super::{APP4, APP6, FAR4, FAR6, lab};
use crate::TunOptions;
use crate::packet::Reason;
use crate::testing::PingScript;
use crate::tests::filter::warning;
use crate::tests::log_capture::LogCapture;
use crate::tests::support::{WITHIN, eventually};
use rurge_dns::fakeip::{v4_of, v6_of};
use std::net::IpAddr;
use std::time::Duration;

/// What went wrong with an echo request is said at debug level, with the name
/// or the address; nothing above debug level has an address or a name in it,
/// and the only line above it is the one the drop counter says once for a
/// reason.
#[tokio::test]
async fn a_failed_echo_is_said_at_debug_and_no_address_is_said_above() {
    let log = LogCapture::start();
    let lab = lab(TunOptions {
        echo_timeout: Duration::from_millis(100),
        ..TunOptions::default()
    });
    let lost = lab.assign("lost.test");
    let empty = lab.assign("empty.test");
    lab.resolver.insert("empty.test", Vec::new());
    lab.extras.pinger.script(FAR4, PingScript::Fail);
    lab.extras.pinger.script(FAR6, PingScript::Silent);
    lab.ping(APP4, IpAddr::V4(v4_of(lost)), 1, 1, b"x");
    lab.ping(APP6, IpAddr::V6(v6_of(empty)), 1, 2, b"x");
    lab.ping(APP4, FAR4, 1, 3, b"x");
    lab.ping(APP6, FAR6, 1, 4, b"x");
    lab.pinged_times(2).await;
    lab.tasks_end().await;
    // two requests dropped for the setting: the first is said at WARN, the
    // second at debug level
    lab.forwarding(false);
    lab.ping(APP4, FAR4, 1, 5, b"x");
    lab.ping(APP4, FAR4, 1, 6, b"x");
    eventually("the requests to be counted", WITHIN, || {
        lab.counters().unsupported == 2
    })
    .await;
    let said = |wanted: &[&str]| {
        log.lines().into_iter().any(|line| {
            line.starts_with("DEBUG") && wanted.iter().all(|piece| line.contains(piece))
        })
    };
    assert!(said(&["lost.test"]), "{:?}", log.lines());
    assert!(said(&["empty.test"]), "{:?}", log.lines());
    assert!(
        said(&["192.0.2.10", "scripted failure"]),
        "{:?}",
        log.lines()
    );
    assert!(said(&["2001:db8::a"]), "{:?}", log.lines());
    assert!(said(&["unsupported", "192.0.2.10"]), "{:?}", log.lines());
    let lines = log.lines();
    let above: Vec<&String> = lines
        .iter()
        .filter(|line| !line.starts_with("DEBUG") && !line.starts_with("TRACE"))
        .collect();
    assert_eq!(above, [&warning(Reason::Unsupported)], "{lines:?}");
}
