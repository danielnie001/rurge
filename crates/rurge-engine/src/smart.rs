//! The engine's side of the `smart` groups (phase 2 M3c design §4, §7): a
//! dial that does not connect through the member a group picked tries the
//! next ones in line, and every session through such a group reports what it
//! saw of the member it went through.

use rurge_inbound::SessionHandle;
use rurge_policy::smart::SmartBook;
use rurge_policy::{PolicyRegistry, Resolution, SmartPick, TerminalKind};
use rurge_proto::{OutboundError, OutboundRef};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Members a dial tries after the one a `smart` group picked (M3c design §9).
pub(crate) const RETRIES: usize = 2;
/// A session through a `smart` group without a byte back this long after its
/// outbound was ready counts against the member (M3c design 4.3).
pub(crate) const NO_RESPONSE: Duration = Duration::from_secs(3);

/// The dials of a session, in order (M3c design §7): through what `first`
/// picked and — when a `smart` group picked it — through the next members in
/// line, at most `RETRIES` of them, each with the chain it would have had. A
/// member that is no proxy (its protocol is not implemented yet) is not
/// tried.
pub(crate) fn attempts(registry: &PolicyRegistry, first: Resolution) -> Vec<Resolution> {
    let Some(pick) = first.smart.clone() else {
        return vec![first];
    };
    let prefix: Vec<String> = first
        .chain
        .iter()
        .take_while(|name| **name != pick.group)
        .cloned()
        .chain([pick.group.clone()])
        .collect();
    let mut out = vec![first];
    for member in &pick.retry {
        if out.len() > RETRIES {
            break;
        }
        let mut next = registry.resolve_member(member);
        if next.terminal != TerminalKind::Proxy {
            continue;
        }
        let mut chain = prefix.clone();
        chain.append(&mut next.chain);
        next.chain = chain;
        next.smart = Some(SmartPick {
            group: pick.group.clone(),
            member: member.clone(),
            retry: Vec::new(),
        });
        out.push(next);
    }
    out
}

/// Whether another member may do better: the connection failed — unlike a
/// REJECT, or a protocol not implemented yet.
pub(crate) fn retryable(e: &OutboundError) -> bool {
    !matches!(e, OutboundError::Reject(_) | OutboundError::Unsupported(_))
}

/// `members`, quoted the way the session log's notes quote names.
pub(crate) fn quoted(members: &[String]) -> String {
    members
        .iter()
        .map(|m| format!("`{m}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a session tells the book of the member a `smart` group gave it:
/// once, whichever comes first.
struct Watch {
    book: Arc<SmartBook>,
    policy: String,
    outbound: OutboundRef,
    host: String,
    told: AtomicBool,
}

impl Watch {
    fn success(&self, latency: Duration) {
        if !self.told.swap(true, Ordering::AcqRel) {
            let host = Some(self.host.as_str());
            self.book
                .report_success(&self.policy, &self.outbound, host, latency, Instant::now());
        }
    }

    fn failure(&self) {
        if !self.told.swap(true, Ordering::AcqRel) {
            let host = Some(self.host.as_str());
            self.book
                .report_failure(&self.policy, &self.outbound, host, Instant::now());
        }
    }
}

/// Hangs the report of `policy` on `handle` (M3c design 4.3): the first byte
/// back is a sample; `NO_RESPONSE` without one, or upstream ending before it
/// sent anything while the client was still there, is a failure — whichever
/// comes first, once. A `kill` and a shutdown say nothing of the member.
pub(crate) fn watch(
    handle: &Arc<SessionHandle>,
    book: Arc<SmartBook>,
    policy: &str,
    outbound: &OutboundRef,
    host: &str,
) {
    let watch = Arc::new(Watch {
        book,
        policy: policy.to_string(),
        outbound: outbound.clone(),
        host: host.to_string(),
        told: AtomicBool::new(false),
    });
    let on_byte = watch.clone();
    handle.on_first_byte(move |h| {
        if let Some(latency) = h.first_byte_time() {
            on_byte.success(latency);
        }
    });
    let on_end = watch.clone();
    handle.on_finish(move |h, _| {
        if h.upstream_failed() && !h.first_byte_seen() && !h.was_killed() {
            on_end.failure();
        }
    });
    let session = Arc::downgrade(handle);
    tokio::spawn(async move {
        tokio::time::sleep(NO_RESPONSE).await;
        if let Some(h) = session.upgrade()
            && !h.first_byte_seen()
            && !h.is_finished()
        {
            watch.failure();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;
    use rurge_config::session::SessionInfo;
    use rurge_inbound::SessionOutcome;
    use rurge_policy::smart::{Health, SiteMemory};
    use rurge_proto::{Reject, RejectKind};

    fn outbound() -> OutboundRef {
        Arc::new(Reject::new(RejectKind::Reject))
    }

    fn session() -> Arc<SessionHandle> {
        let h = SessionHandle::new(1, SessionInfo::tcp(HostName::parse("a.test"), 443));
        h.mark_connected();
        h
    }

    /// The first byte back is a sample of the member, and the member worked
    /// at the site; nothing after it counts (M3c design 4.3).
    #[tokio::test]
    async fn the_first_byte_is_the_one_report() {
        let (book, a, h) = (Arc::new(SmartBook::new()), outbound(), session());
        watch(&h, book.clone(), "A", &a, "a.test");
        h.mark_first_byte();
        let now = Instant::now();
        assert!(matches!(book.health("A", &a, now), Health::Healthy(_)));
        assert_eq!(book.site("a.test", now).worked, ["A"]);
        h.mark_upstream_failed();
        h.finish(SessionOutcome::Completed);
        assert!(matches!(book.health("A", &a, now), Health::Healthy(_)));
    }

    /// Three seconds without a byte back is a failure; the session goes on.
    #[tokio::test(start_paused = true)]
    async fn three_silent_seconds_are_a_failure() {
        let (book, a, h) = (Arc::new(SmartBook::new()), outbound(), session());
        watch(&h, book.clone(), "A", &a, "a.test");
        tokio::time::sleep(NO_RESPONSE + Duration::from_millis(10)).await;
        let now = Instant::now();
        assert_eq!(book.health("A", &a, now), Health::Unknown { failures: 1 });
        assert_eq!(book.site("a.test", now).failed, ["A"]);
        assert!(!h.is_finished());
    }

    /// Upstream ending before it answered is a failure; a session that was
    /// killed, or that just ended, says nothing.
    #[tokio::test]
    async fn upstream_ending_first_is_a_failure_and_a_kill_is_not() {
        let book = Arc::new(SmartBook::new());
        let (a, b, c) = (outbound(), outbound(), outbound());
        let failed = session();
        watch(&failed, book.clone(), "A", &a, "a.test");
        failed.mark_upstream_failed();
        failed.finish(SessionOutcome::Failed("upstream closed".into()));
        let killed = session();
        watch(&killed, book.clone(), "B", &b, "a.test");
        killed.kill();
        killed.mark_upstream_failed();
        killed.finish(SessionOutcome::Failed("killed".into()));
        let ended = session();
        watch(&ended, book.clone(), "C", &c, "a.test");
        ended.finish(SessionOutcome::Completed);
        let now = Instant::now();
        assert_eq!(book.health("A", &a, now), Health::Unknown { failures: 1 });
        assert_eq!(book.health("B", &b, now), Health::Unknown { failures: 0 });
        assert_eq!(book.health("C", &c, now), Health::Unknown { failures: 0 });
        assert_eq!(
            book.site("a.test", now),
            SiteMemory {
                worked: Vec::new(),
                failed: vec!["A".to_string()],
            }
        );
    }

    #[test]
    fn a_connect_failure_may_go_to_the_next_member_and_a_reject_may_not() {
        assert!(retryable(&OutboundError::Timeout));
        assert!(retryable(&OutboundError::Proxy("refused".into())));
        assert!(!retryable(&OutboundError::Reject(RejectKind::Reject)));
        assert!(!retryable(&OutboundError::Unsupported("hysteria2".into())));
        assert_eq!(quoted(&["A".into(), "B".into()]), "`A`, `B`");
    }
}
