//! `TunOptions::check`: the options a TUN cannot be started with, in the words
//! it gives, and those it can; and that `Tun::start` holds to it.

use super::support::tun_on;
use crate::TunOptions;
use crate::device::MemoryDevice;
use std::time::Duration;

const DAY: Duration = Duration::from_secs(24 * 60 * 60);
const MS: Duration = Duration::from_millis(1);

const KEEP_ALIVE: &str = "keep_alive must be at least 1 ms and at most 24 hours";
const TIMEOUT: &str = "timeout must be more than keep_alive and at most 24 hours";
const UDP_IDLE: &str = "udp_idle must be at least 1 ms and at most 24 hours";
const UDP_DNS_IDLE: &str =
    "udp_dns_idle must be at least 1 ms, at most 24 hours and at most udp_idle";
const DNS_TCP_IDLE: &str = "dns_tcp_idle must be at least 1 ms and at most 24 hours";
const ECHO_TIMEOUT: &str = "echo_timeout must be at least 1 ms and at most 24 hours";

/// `options` are refused with `text`, the first that cannot be, and no other.
fn refused(options: TunOptions, text: &str) {
    assert_eq!(options.check(), Err(text.to_string()), "{options:?}");
}

#[test]
fn the_defaults_pass_the_check() {
    assert_eq!(TunOptions::default().check(), Ok(()));
}

/// Each of the sixteen texts, with what is just past the edge of each range. The
/// timers are the stack's to add to its clock: `keep_alive` is at least a
/// millisecond and `timeout` is more than it, and neither is more than a day,
/// which is far below where the stack's sums overflow; `udp_idle`,
/// `dns_tcp_idle` and `echo_timeout` are added to clocks as well, and have the
/// same range; `udp_dns_idle` has it too, and is no more than `udp_idle`.
#[test]
fn options_out_of_range_are_refused() {
    refused(
        TunOptions {
            shards: 0,
            ..TunOptions::default()
        },
        "shards must be more than zero",
    );
    refused(
        TunOptions {
            buffer_min: 0,
            ..TunOptions::default()
        },
        "buffer_min must be more than zero",
    );
    refused(
        TunOptions {
            tx_max: 0,
            ..TunOptions::default()
        },
        "tx_max must be more than zero",
    );
    for keep_alive in [Duration::ZERO, MS - Duration::from_micros(1), DAY + MS] {
        refused(
            TunOptions {
                keep_alive,
                ..TunOptions::default()
            },
            KEEP_ALIVE,
        );
    }
    // the default keep-alive is 30 s: a timeout of that or less
    for timeout in [Duration::from_secs(30), Duration::from_secs(10), DAY + MS] {
        refused(
            TunOptions {
                timeout,
                ..TunOptions::default()
            },
            TIMEOUT,
        );
    }
    refused(
        TunOptions {
            shard_queue: 0,
            ..TunOptions::default()
        },
        "shard_queue must be more than zero",
    );
    refused(
        TunOptions {
            writer_queue: 0,
            ..TunOptions::default()
        },
        "writer_queue must be more than zero",
    );
    for udp_idle in [Duration::ZERO, MS - Duration::from_micros(1), DAY + MS] {
        refused(
            TunOptions {
                udp_idle,
                ..TunOptions::default()
            },
            UDP_IDLE,
        );
    }
    for udp_dns_idle in [Duration::ZERO, MS - Duration::from_micros(1), DAY + MS] {
        refused(
            TunOptions {
                udp_dns_idle,
                ..TunOptions::default()
            },
            UDP_DNS_IDLE,
        );
    }
    // more than `udp_idle`, however little that is: one tick more at each
    // end of the range of `udp_idle`, and the default of 10 s over a short one
    for (udp_idle, udp_dns_idle) in [
        (MS, 2 * MS),
        (Duration::from_secs(5), Duration::from_secs(10)),
        (Duration::from_secs(30), Duration::from_secs(31)),
        (DAY - MS, DAY),
        (DAY, DAY + MS),
    ] {
        refused(
            TunOptions {
                udp_idle,
                udp_dns_idle,
                ..TunOptions::default()
            },
            UDP_DNS_IDLE,
        );
    }
    refused(
        TunOptions {
            udp_queue: 0,
            ..TunOptions::default()
        },
        "udp_queue must be more than zero",
    );
    refused(
        TunOptions {
            udp_bytes: 0,
            ..TunOptions::default()
        },
        "udp_bytes must be more than zero",
    );
    refused(
        TunOptions {
            max_udp_targets: 0,
            ..TunOptions::default()
        },
        "max_udp_targets must be more than zero",
    );
    refused(
        TunOptions {
            dns_queue: 0,
            ..TunOptions::default()
        },
        "dns_queue must be more than zero",
    );
    for dns_tcp_idle in [Duration::ZERO, MS - Duration::from_micros(1), DAY + MS] {
        refused(
            TunOptions {
                dns_tcp_idle,
                ..TunOptions::default()
            },
            DNS_TCP_IDLE,
        );
    }
    for echo_timeout in [Duration::ZERO, MS - Duration::from_micros(1), DAY + MS] {
        refused(
            TunOptions {
                echo_timeout,
                ..TunOptions::default()
            },
            ECHO_TIMEOUT,
        );
    }
    refused(
        TunOptions {
            max_echoes: 0,
            ..TunOptions::default()
        },
        "max_echoes must be more than zero",
    );
}

/// The first that cannot be run with, in the order of the fields.
#[test]
fn the_first_option_out_of_range_is_the_one_reported() {
    refused(
        TunOptions {
            shards: 0,
            writer_queue: 0,
            ..TunOptions::default()
        },
        "shards must be more than zero",
    );
    refused(
        TunOptions {
            keep_alive: Duration::ZERO,
            timeout: Duration::ZERO,
            ..TunOptions::default()
        },
        KEEP_ALIVE,
    );
    refused(
        TunOptions {
            timeout: Duration::ZERO,
            shard_queue: 0,
            ..TunOptions::default()
        },
        TIMEOUT,
    );
    // what comes after the seven there were: `writer_queue` is the last of
    // those, then the rest in the order of the fields, each of them refused
    // before every one that comes after it
    refused(
        TunOptions {
            writer_queue: 0,
            udp_idle: Duration::ZERO,
            ..TunOptions::default()
        },
        "writer_queue must be more than zero",
    );
    refused(
        TunOptions {
            udp_idle: Duration::ZERO,
            udp_dns_idle: Duration::ZERO,
            udp_queue: 0,
            udp_bytes: 0,
            max_udp_targets: 0,
            dns_queue: 0,
            dns_tcp_idle: Duration::ZERO,
            echo_timeout: Duration::ZERO,
            max_echoes: 0,
            ..TunOptions::default()
        },
        UDP_IDLE,
    );
    refused(
        TunOptions {
            udp_dns_idle: Duration::ZERO,
            udp_queue: 0,
            udp_bytes: 0,
            max_udp_targets: 0,
            dns_queue: 0,
            dns_tcp_idle: Duration::ZERO,
            echo_timeout: Duration::ZERO,
            max_echoes: 0,
            ..TunOptions::default()
        },
        UDP_DNS_IDLE,
    );
    refused(
        TunOptions {
            udp_queue: 0,
            udp_bytes: 0,
            max_udp_targets: 0,
            dns_queue: 0,
            dns_tcp_idle: Duration::ZERO,
            echo_timeout: Duration::ZERO,
            max_echoes: 0,
            ..TunOptions::default()
        },
        "udp_queue must be more than zero",
    );
    refused(
        TunOptions {
            udp_bytes: 0,
            max_udp_targets: 0,
            dns_queue: 0,
            dns_tcp_idle: Duration::ZERO,
            echo_timeout: Duration::ZERO,
            max_echoes: 0,
            ..TunOptions::default()
        },
        "udp_bytes must be more than zero",
    );
    refused(
        TunOptions {
            max_udp_targets: 0,
            dns_queue: 0,
            dns_tcp_idle: Duration::ZERO,
            echo_timeout: Duration::ZERO,
            max_echoes: 0,
            ..TunOptions::default()
        },
        "max_udp_targets must be more than zero",
    );
    refused(
        TunOptions {
            dns_queue: 0,
            dns_tcp_idle: Duration::ZERO,
            echo_timeout: Duration::ZERO,
            max_echoes: 0,
            ..TunOptions::default()
        },
        "dns_queue must be more than zero",
    );
    refused(
        TunOptions {
            dns_tcp_idle: Duration::ZERO,
            echo_timeout: Duration::ZERO,
            max_echoes: 0,
            ..TunOptions::default()
        },
        DNS_TCP_IDLE,
    );
    refused(
        TunOptions {
            echo_timeout: Duration::ZERO,
            max_echoes: 0,
            ..TunOptions::default()
        },
        ECHO_TIMEOUT,
    );
}

/// The edges themselves are inside the ranges.
#[test]
fn the_edges_of_the_ranges_pass() {
    for (keep_alive, timeout) in [
        (MS, Duration::from_secs(600)),
        (Duration::from_secs(30), DAY),
        (DAY - MS, DAY),
        (Duration::from_millis(100), Duration::from_millis(101)),
    ] {
        let options = TunOptions {
            keep_alive,
            timeout,
            ..TunOptions::default()
        };
        assert_eq!(options.check(), Ok(()), "{options:?}");
    }
    // the same two edges for the times of UDP sessions, of DNS connections and
    // of echo requests, and the smallest counts; `udp_dns_idle` is as long as
    // `udp_idle`, which is as long as it can be and still pass
    for time in [MS, Duration::from_secs(60), DAY] {
        let options = TunOptions {
            udp_idle: time,
            udp_dns_idle: time,
            dns_tcp_idle: time,
            echo_timeout: time,
            udp_queue: 1,
            udp_bytes: 1,
            max_udp_targets: 1,
            dns_queue: 1,
            max_echoes: 1,
            ..TunOptions::default()
        };
        assert_eq!(options.check(), Ok(()), "{options:?}");
    }
    // and as short as it can be, whatever `udp_idle` is
    for udp_idle in [MS, Duration::from_secs(60), DAY] {
        let options = TunOptions {
            udp_idle,
            udp_dns_idle: MS,
            ..TunOptions::default()
        };
        assert_eq!(options.check(), Ok(()), "{options:?}");
    }
}

/// A budget of none is every socket on the smallest buffers, no sockets is
/// every SYN answered with a reset, and no half-open connections is none let
/// through its handshake: all of them are settings, and the only ones that
/// are allowed to be zero.
#[test]
fn zero_budgets_and_limits_pass() {
    for options in [
        TunOptions {
            budget: 0,
            ..TunOptions::default()
        },
        TunOptions {
            max_sockets: 0,
            ..TunOptions::default()
        },
        TunOptions {
            max_half_open: 0,
            ..TunOptions::default()
        },
        TunOptions {
            budget: 0,
            max_sockets: 0,
            max_half_open: 0,
            ..TunOptions::default()
        },
    ] {
        assert_eq!(options.check(), Ok(()), "{options:?}");
    }
}

/// What the stack takes whatever it is (a hold, a linger or a handshake that
/// is any time at all, as far as `Duration::MAX`) is not checked.
#[test]
fn the_times_that_are_total_are_not_checked() {
    for time in [Duration::ZERO, Duration::from_millis(1), Duration::MAX] {
        let options = TunOptions {
            drop_hold: time,
            half_open_timeout: time,
            linger: time,
            ..TunOptions::default()
        };
        assert_eq!(options.check(), Ok(()), "{options:?}");
    }
}

/// `Tun::start` does not clamp what `check` refuses: it panics, saying which.
#[tokio::test]
#[should_panic(expected = "the TUN's options: shards must be more than zero")]
async fn a_tun_with_bad_options_does_not_start() {
    let (device, _app) = MemoryDevice::pair(1500, 16);
    let _started = tun_on(
        device,
        TunOptions {
            shards: 0,
            ..TunOptions::default()
        },
    );
}
