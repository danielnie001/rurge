//! Runtime observation (M3 design §7.3): the request log (a bounded ring of
//! finished requests plus an index of the in-flight ones) and traffic stats.
//! The M4 API reads these; nothing here reaches outside the process.

use rurge_config::rule::ProtocolKind;
use rurge_config::session::ListenerKind;
use rurge_inbound::{SessionHandle, SessionOutcome};
use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// How a request stands: still running, or how it ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordStatus {
    Active,
    Completed,
    /// Carries the outbound name, e.g. `REJECT` or `REJECT-TINYGIF`.
    Rejected(String),
    Failed,
}

/// A snapshot of one session's observable facts, for the future `active` /
/// request-log API views.
#[derive(Clone, Debug)]
pub struct RequestRecord {
    pub id: u64,
    pub listener: ListenerKind,
    pub src: SocketAddr,
    pub dst: String,
    pub rule: Option<String>,
    pub policy: Vec<String>,
    pub sni: Option<String>,
    pub protocol: Option<ProtocolKind>,
    pub up: u64,
    pub down: u64,
    pub started_ms: u64,
    pub elapsed_ms: u64,
    pub status: RecordStatus,
    pub error: Option<String>,
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn record_of(h: &SessionHandle, outcome: Option<&SessionOutcome>) -> RequestRecord {
    let s = h.session();
    let (up, down) = h.bytes();
    let elapsed_ms = h.elapsed().as_millis() as u64;
    let status = match outcome {
        None => RecordStatus::Active,
        Some(SessionOutcome::Completed) => RecordStatus::Completed,
        Some(SessionOutcome::Rejected(k)) => RecordStatus::Rejected(k.name().to_string()),
        Some(SessionOutcome::Failed(_)) => RecordStatus::Failed,
    };
    // `set_error` explains rejects (and anything else the engine notes); a
    // failure's own message is the error when nothing more specific was set.
    let error = h.error().or_else(|| match outcome {
        Some(SessionOutcome::Failed(m)) => Some(m.clone()),
        _ => None,
    });
    RequestRecord {
        id: h.id(),
        listener: s.listener,
        src: s.src,
        dst: format!("{}:{}", s.dst_host, s.dst_port),
        rule: h.rule(),
        policy: h.policy_chain(),
        sni: h.sni(),
        protocol: h.protocol().or(s.protocol),
        up,
        down,
        started_ms: unix_millis().saturating_sub(elapsed_ms),
        elapsed_ms,
        status,
        error,
    }
}

/// A bounded ring of finished requests plus an index of the in-flight ones.
pub struct RequestLog {
    capacity: usize,
    active: Mutex<BTreeMap<u64, Arc<SessionHandle>>>,
    finished: Mutex<VecDeque<RequestRecord>>,
}

impl RequestLog {
    /// A log that keeps at most `capacity` finished records (oldest evicted first).
    pub fn new(capacity: usize) -> RequestLog {
        RequestLog {
            capacity: capacity.max(1),
            active: Mutex::new(BTreeMap::new()),
            finished: Mutex::new(VecDeque::with_capacity(capacity.max(1))),
        }
    }

    /// Indexes a session as in-flight; call before dialing.
    pub fn mark_active(&self, handle: &Arc<SessionHandle>) {
        self.active
            .lock()
            .expect("active index")
            .insert(handle.id(), handle.clone());
    }

    /// Moves a session from the active index into the finished ring.
    pub fn record_finished(&self, handle: &SessionHandle, outcome: &SessionOutcome) {
        self.active
            .lock()
            .expect("active index")
            .remove(&handle.id());
        let rec = record_of(handle, Some(outcome));
        let mut ring = self.finished.lock().expect("finished ring");
        if ring.len() == self.capacity {
            ring.pop_front();
        }
        ring.push_back(rec);
    }

    /// Finished requests, newest first.
    pub fn recent(&self, n: usize) -> Vec<RequestRecord> {
        self.finished
            .lock()
            .expect("finished ring")
            .iter()
            .rev()
            .take(n)
            .cloned()
            .collect()
    }

    /// In-flight requests (bytes are live), newest first.
    pub fn active(&self) -> Vec<RequestRecord> {
        self.active
            .lock()
            .expect("active index")
            .values()
            .rev()
            .map(|h| record_of(h, None))
            .collect()
    }

    /// Total bytes moved by the in-flight requests so far.
    pub fn active_bytes(&self) -> (u64, u64) {
        self.active
            .lock()
            .expect("active index")
            .values()
            .fold((0, 0), |(u, d), h| {
                let (hu, hd) = h.bytes();
                (u + hu, d + hd)
            })
    }

    /// Kills an in-flight session by id; false if it is not active.
    pub fn kill(&self, id: u64) -> bool {
        match self.active.lock().expect("active index").get(&id) {
            Some(h) => {
                h.kill();
                true
            }
            None => false,
        }
    }

    /// Number of finished records currently held.
    pub fn len(&self) -> usize {
        self.finished.lock().expect("finished ring").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;
    use rurge_config::session::{ListenerKind, SessionInfo};
    use rurge_inbound::{SessionHandle, SessionOutcome};
    use std::sync::Arc;

    fn handle(id: u64, host: &str) -> Arc<SessionHandle> {
        let mut s = SessionInfo::tcp(HostName::parse(host), 443);
        s.listener = ListenerKind::Http;
        let h = SessionHandle::new(id, s);
        h.set_rule(Some(format!("DOMAIN,{host},DIRECT")));
        h.set_policy_chain(vec!["DIRECT".into()]);
        h
    }

    #[test]
    fn active_then_finished_moves_into_the_ring() {
        let log = RequestLog::new(2);
        let a = handle(1, "a.test");
        log.mark_active(&a);
        assert_eq!(log.active().len(), 1);
        assert_eq!(log.active()[0].status, RecordStatus::Active);
        a.add_up(10);
        a.add_down(20);
        a.finish(SessionOutcome::Completed);
        log.record_finished(&a, &SessionOutcome::Completed);
        assert_eq!(log.active().len(), 0);
        let recent = log.recent(10);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, 1);
        assert_eq!((recent[0].up, recent[0].down), (10, 20));
        assert_eq!(recent[0].status, RecordStatus::Completed);
        assert_eq!(recent[0].rule.as_deref(), Some("DOMAIN,a.test,DIRECT"));
    }

    #[test]
    fn ring_evicts_oldest_beyond_capacity() {
        let log = RequestLog::new(2);
        for id in 1..=3 {
            let h = handle(id, "x.test");
            log.mark_active(&h);
            log.record_finished(&h, &SessionOutcome::Completed);
        }
        let recent = log.recent(10);
        assert_eq!(recent.len(), 2);
        // newest first
        assert_eq!(recent[0].id, 3);
        assert_eq!(recent[1].id, 2);
    }

    #[test]
    fn kill_cancels_an_active_session_only() {
        let log = RequestLog::new(4);
        let a = handle(7, "k.test");
        log.mark_active(&a);
        assert!(log.kill(7));
        assert!(a.token().is_cancelled());
        assert!(!log.kill(999));
    }

    #[test]
    fn rejected_and_failed_carry_their_status() {
        let log = RequestLog::new(4);
        let r = handle(1, "r.test");
        r.set_error("policy protocol not implemented: ss");
        log.mark_active(&r);
        log.record_finished(
            &r,
            &SessionOutcome::Rejected(rurge_proto::RejectKind::Reject),
        );
        let f = handle(2, "f.test");
        log.mark_active(&f);
        log.record_finished(&f, &SessionOutcome::Failed("dns lookup failed".into()));
        let recent = log.recent(10);
        assert_eq!(recent[0].status, RecordStatus::Failed);
        assert_eq!(recent[0].error.as_deref(), Some("dns lookup failed"));
        assert_eq!(recent[1].status, RecordStatus::Rejected("REJECT".into()));
        assert_eq!(
            recent[1].error.as_deref(),
            Some("policy protocol not implemented: ss")
        );
    }
}
