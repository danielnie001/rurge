//! Runtime observation (M3 design §7.3): the request log (a bounded ring of
//! finished requests plus an index of the in-flight ones) and traffic stats.
//! The M4 API reads these; nothing here reaches outside the process.

use rurge_config::rule::ProtocolKind;
use rurge_config::session::ListenerKind;
use rurge_inbound::{SessionHandle, SessionOutcome};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
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

/// A snapshot of cumulative up/down bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrafficTotals {
    pub up: u64,
    pub down: u64,
}

/// Cumulative up/down byte counters (global, per listener kind, per policy)
/// plus a per-second rate derived from successive [`TrafficStats::sample`] calls.
pub struct TrafficStats {
    up: AtomicU64,
    down: AtomicU64,
    http_up: AtomicU64,
    http_down: AtomicU64,
    socks_up: AtomicU64,
    socks_down: AtomicU64,
    per_policy: Mutex<HashMap<String, (u64, u64)>>,
    // rate sampling: last cumulative-plus-active reading and the last delta
    last_sample: Mutex<(u64, u64)>,
    rate_up: AtomicU64,
    rate_down: AtomicU64,
}

impl Default for TrafficStats {
    fn default() -> Self {
        TrafficStats::new()
    }
}

impl TrafficStats {
    /// A fresh set of counters, all zero.
    pub fn new() -> TrafficStats {
        TrafficStats {
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            http_up: AtomicU64::new(0),
            http_down: AtomicU64::new(0),
            socks_up: AtomicU64::new(0),
            socks_down: AtomicU64::new(0),
            per_policy: Mutex::new(HashMap::new()),
            last_sample: Mutex::new((0, 0)),
            rate_up: AtomicU64::new(0),
            rate_down: AtomicU64::new(0),
        }
    }

    /// Adds a finished session's bytes to the cumulative counters.
    pub fn record(&self, h: &SessionHandle) {
        let (up, down) = h.bytes();
        self.up.fetch_add(up, Ordering::Relaxed);
        self.down.fetch_add(down, Ordering::Relaxed);
        match h.session().listener {
            ListenerKind::Socks5 => {
                self.socks_up.fetch_add(up, Ordering::Relaxed);
                self.socks_down.fetch_add(down, Ordering::Relaxed);
            }
            _ => {
                self.http_up.fetch_add(up, Ordering::Relaxed);
                self.http_down.fetch_add(down, Ordering::Relaxed);
            }
        }
        if let Some(policy) = h
            .policy_chain()
            .into_iter()
            .rev()
            .find(|p| !p.starts_with('!'))
        {
            let mut m = self.per_policy.lock().expect("per-policy traffic");
            let e = m.entry(policy).or_insert((0, 0));
            e.0 += up;
            e.1 += down;
        }
    }

    /// Cumulative bytes across all finished sessions.
    pub fn totals(&self) -> TrafficTotals {
        TrafficTotals {
            up: self.up.load(Ordering::Relaxed),
            down: self.down.load(Ordering::Relaxed),
        }
    }

    /// Cumulative bytes broken down by listener kind (`Http`, `Socks5`).
    pub fn by_listener(&self) -> [(ListenerKind, u64, u64); 2] {
        [
            (
                ListenerKind::Http,
                self.http_up.load(Ordering::Relaxed),
                self.http_down.load(Ordering::Relaxed),
            ),
            (
                ListenerKind::Socks5,
                self.socks_up.load(Ordering::Relaxed),
                self.socks_down.load(Ordering::Relaxed),
            ),
        ]
    }

    /// Cumulative bytes broken down by policy name, sorted by name.
    pub fn by_policy(&self) -> Vec<(String, u64, u64)> {
        let mut v: Vec<_> = self
            .per_policy
            .lock()
            .expect("per-policy traffic")
            .iter()
            .map(|(k, (u, d))| (k.clone(), *u, *d))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Records one rate sample: `active` is the live byte total of in-flight
    /// sessions; the total tracked is finished-cumulative + active. Rate is the
    /// non-negative delta from the previous sample (call once per second).
    pub fn sample(&self, active: (u64, u64)) {
        let total_up = self.up.load(Ordering::Relaxed) + active.0;
        let total_down = self.down.load(Ordering::Relaxed) + active.1;
        let mut last = self.last_sample.lock().expect("rate sample");
        self.rate_up
            .store(total_up.saturating_sub(last.0), Ordering::Relaxed);
        self.rate_down
            .store(total_down.saturating_sub(last.1), Ordering::Relaxed);
        *last = (total_up, total_down);
    }

    /// Bytes per second from the last two samples.
    pub fn rate(&self) -> (u64, u64) {
        (
            self.rate_up.load(Ordering::Relaxed),
            self.rate_down.load(Ordering::Relaxed),
        )
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

    #[test]
    fn traffic_accumulates_globally_and_per_dimension() {
        let t = TrafficStats::new();
        let a = handle(1, "a.test"); // Http listener, policy DIRECT
        a.add_up(100);
        a.add_down(200);
        t.record(&a);
        assert_eq!(t.totals().up, 100);
        assert_eq!(t.totals().down, 200);
        let by_l = t.by_listener();
        let http = by_l
            .iter()
            .find(|(k, _, _)| *k == ListenerKind::Http)
            .unwrap();
        assert_eq!((http.1, http.2), (100, 200));
        let by_p = t.by_policy();
        assert_eq!(by_p, vec![("DIRECT".to_string(), 100, 200)]);
    }

    #[test]
    fn rate_is_the_delta_between_samples() {
        let t = TrafficStats::new();
        // cumulative finished = 0; first sample sees 1000 active bytes up
        t.sample((1000, 0));
        assert_eq!(t.rate(), (1000, 0));
        t.sample((1500, 300));
        assert_eq!(t.rate(), (500, 300));
        // a sample that goes backwards (a session ended, active dropped) clamps to 0
        t.sample((1400, 300));
        assert_eq!(t.rate(), (0, 0));
    }
}
