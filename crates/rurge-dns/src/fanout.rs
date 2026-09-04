//! Concurrent query engine (design §7.3, manual `dns/overview.html`): every
//! selected upstream is asked at once, the query is re-sent every `resend`
//! until `attempts` rounds went out, the first valid answer wins, and "empty"
//! is reported only when every upstream said so (or some said so and the
//! rest never answered).

use crate::message::{Answer, Qtype, Question, build_query, parse_response, random_id};
use crate::upstream::{UpstreamError, UpstreamRef};
use std::collections::HashSet;
use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::Duration;
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep_until};

#[derive(Clone, Debug)]
pub struct FanoutOpts {
    pub resend: Duration,
    pub attempts: u32,
}

impl Default for FanoutOpts {
    fn default() -> Self {
        FanoutOpts {
            resend: Duration::from_secs(1),
            attempts: 5,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answers {
    pub v4: Vec<(Ipv4Addr, u32)>,
    pub v6: Vec<(Ipv6Addr, u32)>,
    /// Upstream whose answer won (the A answer's, else the AAAA answer's).
    pub upstream: String,
    /// One family is missing because its answer had not arrived at a resend tick.
    pub partial: bool,
    /// A arrived but AAAA never did — feeds AAAA suppression.
    pub aaaa_timed_out: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FanoutError {
    EmptyAnswer,
    Timeout,
    AllFailed(Vec<(String, String)>),
}

impl fmt::Display for FanoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FanoutError::EmptyAnswer => f.write_str("empty answer"),
            FanoutError::Timeout => f.write_str("timeout"),
            FanoutError::AllFailed(list) => {
                if list.is_empty() {
                    return f.write_str("no upstream");
                }
                let parts: Vec<String> = list.iter().map(|(u, e)| format!("{u}: {e}")).collect();
                write!(f, "all upstreams failed ({})", parts.join("; "))
            }
        }
    }
}

struct Outcome {
    upstream: String,
    qtype: Qtype,
    result: Result<Answer, UpstreamError>,
}

enum State {
    Pending,
    Valid(Answer, String),
    Empty,
}

struct Track {
    question: Question,
    state: State,
    empties: HashSet<String>,
    failures: Vec<(String, String)>,
}

impl Track {
    fn decided(&self) -> bool {
        !matches!(self.state, State::Pending)
    }

    fn apply(
        &mut self,
        upstream: &str,
        result: Result<Answer, UpstreamError>,
        total_upstreams: usize,
    ) {
        if self.decided() {
            return;
        }
        match result {
            Ok(answer) if answer.is_valid_for(&self.question) => {
                self.state = State::Valid(answer, upstream.to_string());
            }
            Ok(answer) if answer.is_empty_for(&self.question) => {
                self.empties.insert(upstream.to_string());
                if self.empties.len() >= total_upstreams {
                    self.state = State::Empty;
                }
            }
            Ok(answer) => self
                .failures
                .push((upstream.to_string(), format!("rcode {}", answer.rcode))),
            Err(e) => self.failures.push((upstream.to_string(), e.to_string())),
        }
    }

    /// At the deadline: "some upstreams answered empty and the rest never answered" counts as empty.
    fn settle_empty(&mut self) {
        if !self.decided() && !self.empties.is_empty() && self.failures.is_empty() {
            self.state = State::Empty;
        }
    }
}

async fn one_query(upstream: UpstreamRef, question: Question, deadline: Instant) -> Outcome {
    let qtype = question.qtype;
    let name = upstream.name().to_string();
    let started = Instant::now();
    let result = async {
        let id = random_id();
        let wire =
            build_query(id, &question).map_err(|e| UpstreamError::BadResponse(e.to_string()))?;
        let bytes = upstream.query(&wire, deadline).await?;
        let answer =
            parse_response(&bytes).map_err(|e| UpstreamError::BadResponse(e.to_string()))?;
        if answer.id != id {
            return Err(UpstreamError::BadResponse(format!(
                "id mismatch: sent {id}, got {}",
                answer.id
            )));
        }
        Ok(answer)
    }
    .await;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    match &result {
        Ok(a) => {
            tracing::debug!(target: "rurge_dns::fanout", upstream = %name, qtype = qtype.as_str(), rcode = %a.rcode, records = a.v4.len() + a.v6.len(), elapsed_ms, "answer")
        }
        Err(e) => {
            tracing::debug!(target: "rurge_dns::fanout", upstream = %name, qtype = qtype.as_str(), error = %e, elapsed_ms, "failed")
        }
    }
    Outcome {
        upstream: name,
        qtype,
        result,
    }
}

pub async fn resolve_name(
    upstreams: &[UpstreamRef],
    name: &str,
    want_v6: bool,
    opts: &FanoutOpts,
) -> Result<Answers, FanoutError> {
    if upstreams.is_empty() {
        return Err(FanoutError::AllFailed(Vec::new()));
    }
    let start = Instant::now();
    let attempts = opts.attempts.max(1);
    let deadline = start + opts.resend * attempts;
    let mut tracks: Vec<Track> = [Qtype::A, Qtype::Aaaa]
        .into_iter()
        .filter(|q| *q == Qtype::A || want_v6)
        .map(|qtype| Track {
            question: Question {
                name: name.to_string(),
                qtype,
            },
            state: State::Pending,
            empties: HashSet::new(),
            failures: Vec::new(),
        })
        .collect();
    let mut set: JoinSet<Outcome> = JoinSet::new();
    let mut round = 0u32;
    let mut next_send = start;
    let mut partial = false;
    loop {
        if round < attempts && Instant::now() >= next_send {
            for up in upstreams {
                for t in tracks.iter().filter(|t| !t.decided()) {
                    tracing::debug!(target: "rurge_dns::fanout", upstream = %up.name(), qtype = t.question.qtype.as_str(), round, "send");
                    set.spawn(one_query(up.clone(), t.question.clone(), deadline));
                }
            }
            round += 1;
            next_send = start + opts.resend * round;
        }
        if tracks.iter().all(Track::decided) {
            break;
        }
        let tick = if round < attempts {
            next_send
        } else {
            deadline
        };
        tokio::select! {
            joined = set.join_next(), if !set.is_empty() => {
                if let Some(Ok(outcome)) = joined
                    && let Some(t) = tracks.iter_mut().find(|t| t.question.qtype == outcome.qtype)
                {
                    t.apply(&outcome.upstream, outcome.result, upstreams.len());
                }
            }
            _ = sleep_until(tick) => {
                if round >= attempts {
                    break;
                }
                let any_valid = tracks.iter().any(|t| matches!(t.state, State::Valid(..)));
                let any_pending = tracks.iter().any(|t| !t.decided());
                if any_valid && any_pending {
                    partial = true;
                    break;
                }
            }
        }
    }
    set.abort_all();
    for t in &mut tracks {
        t.settle_empty();
    }
    let mut answers = Answers {
        partial,
        ..Answers::default()
    };
    let mut any_valid = false;
    let mut any_empty = false;
    let mut failures = Vec::new();
    for t in &mut tracks {
        match std::mem::replace(&mut t.state, State::Pending) {
            State::Valid(answer, upstream) => {
                any_valid = true;
                if answers.upstream.is_empty() || t.question.qtype == Qtype::A {
                    answers.upstream = upstream;
                }
                match t.question.qtype {
                    Qtype::A => answers.v4 = answer.v4,
                    Qtype::Aaaa => answers.v6 = answer.v6,
                }
            }
            State::Empty => any_empty = true,
            State::Pending => {
                if t.question.qtype == Qtype::Aaaa {
                    answers.aaaa_timed_out = true;
                }
                answers.partial = true;
                failures.append(&mut t.failures);
            }
        }
    }
    if any_valid {
        return Ok(answers);
    }
    if any_empty {
        return Err(FanoutError::EmptyAnswer);
    }
    if !failures.is_empty() {
        failures.sort();
        failures.dedup();
        return Err(FanoutError::AllFailed(failures));
    }
    Err(FanoutError::Timeout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Rcode;
    use crate::testing::MockDns;
    use crate::upstream::udp::UdpUpstream;
    use std::sync::Arc;

    fn fast() -> FanoutOpts {
        FanoutOpts {
            resend: Duration::from_millis(100),
            attempts: 4,
        }
    }

    fn up(server: &MockDns) -> UpstreamRef {
        Arc::new(UdpUpstream::new(server.addr()))
    }

    #[tokio::test]
    async fn single_upstream_answers() {
        let s = MockDns::spawn().await;
        s.set("a.test", &["10.0.0.1", "10.0.0.2"], &[], 300);
        let ans = resolve_name(&[up(&s)], "a.test", false, &fast())
            .await
            .unwrap();
        assert_eq!(
            ans.v4,
            vec![
                ("10.0.0.1".parse().unwrap(), 300),
                ("10.0.0.2".parse().unwrap(), 300)
            ]
        );
        assert!(ans.v6.is_empty() && !ans.partial && !ans.aaaa_timed_out);
        assert_eq!(ans.upstream, format!("udp://{}", s.addr()));
        assert_eq!(s.query_count("a.test", Qtype::A), 1);
        assert_eq!(
            s.query_count("a.test", Qtype::Aaaa),
            0,
            "AAAA not asked when want_v6 is false"
        );
    }

    #[tokio::test]
    async fn first_valid_answer_wins() {
        let slow = MockDns::spawn().await;
        slow.set("a.test", &["10.0.0.9"], &[], 60);
        slow.set_delay(Duration::from_millis(300));
        let fast_srv = MockDns::spawn().await;
        fast_srv.set("a.test", &["10.0.0.1"], &[], 60);
        let started = Instant::now();
        let ans = resolve_name(&[up(&slow), up(&fast_srv)], "a.test", false, &fast())
            .await
            .unwrap();
        assert_eq!(ans.v4[0].0, "10.0.0.1".parse::<Ipv4Addr>().unwrap());
        assert_eq!(ans.upstream, format!("udp://{}", fast_srv.addr()));
        assert!(started.elapsed() < Duration::from_millis(250));
    }

    #[tokio::test]
    async fn resends_after_the_timer_and_recovers() {
        let s = MockDns::spawn().await;
        s.set("a.test", &["10.0.0.1"], &[], 60);
        s.set_drop_first(1);
        let started = Instant::now();
        let ans = resolve_name(&[up(&s)], "a.test", false, &fast())
            .await
            .unwrap();
        assert_eq!(ans.v4.len(), 1);
        assert!(
            started.elapsed() >= Duration::from_millis(100),
            "answer came from the second round"
        );
        assert_eq!(s.query_count("a.test", Qtype::A), 2);
    }

    #[tokio::test]
    async fn all_dropped_is_a_timeout_after_every_attempt() {
        let s = MockDns::spawn().await;
        s.set("a.test", &["10.0.0.1"], &[], 60);
        s.set_drop_all(true);
        let started = Instant::now();
        let err = resolve_name(&[up(&s)], "a.test", false, &fast())
            .await
            .unwrap_err();
        assert_eq!(err, FanoutError::Timeout);
        assert!(started.elapsed() >= Duration::from_millis(400));
        assert_eq!(s.query_count("a.test", Qtype::A), 4);
    }

    #[tokio::test]
    async fn empty_answer_rules() {
        // all empty → EmptyAnswer immediately
        let e1 = MockDns::spawn().await;
        e1.set_empty("nx.test");
        let e2 = MockDns::spawn().await;
        e2.set_empty("nx.test");
        let started = Instant::now();
        assert_eq!(
            resolve_name(&[up(&e1), up(&e2)], "nx.test", false, &fast()).await,
            Err(FanoutError::EmptyAnswer)
        );
        assert!(started.elapsed() < Duration::from_millis(100));
        // one empty + one dropping → EmptyAnswer at the deadline
        let d = MockDns::spawn().await;
        d.set_drop_all(true);
        assert_eq!(
            resolve_name(&[up(&e1), up(&d)], "nx.test", false, &fast()).await,
            Err(FanoutError::EmptyAnswer)
        );
        // one empty + one valid → valid
        let v = MockDns::spawn().await;
        v.set("nx.test", &["10.0.0.5"], &[], 60);
        let ans = resolve_name(&[up(&e1), up(&v)], "nx.test", false, &fast())
            .await
            .unwrap();
        assert_eq!(ans.upstream, format!("udp://{}", v.addr()));
        // unknown names answer NXDOMAIN, which is also "empty"
        assert_eq!(
            resolve_name(&[up(&v)], "unknown.test", false, &fast()).await,
            Err(FanoutError::EmptyAnswer)
        );
    }

    #[tokio::test]
    async fn server_failures_are_reported() {
        let s = MockDns::spawn().await;
        s.set_rcode("bad.test", Rcode::ServFail);
        match resolve_name(&[up(&s)], "bad.test", false, &fast()).await {
            Err(FanoutError::AllFailed(list)) => {
                assert_eq!(list.len(), 1);
                assert!(list[0].1.contains("SERVFAIL") || list[0].1.contains("rcode"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            resolve_name(&[], "a.test", false, &fast()).await,
            Err(FanoutError::AllFailed(Vec::new()))
        );
    }

    #[tokio::test]
    async fn a_and_aaaa_in_parallel_with_partial_results() {
        let s = MockDns::spawn().await;
        s.set("dual.test", &["10.0.0.1"], &["fd00::1"], 60);
        let ans = resolve_name(&[up(&s)], "dual.test", true, &fast())
            .await
            .unwrap();
        assert_eq!(ans.v4.len(), 1);
        assert_eq!(ans.v6.len(), 1);
        assert!(!ans.partial && !ans.aaaa_timed_out);
        assert_eq!(s.query_count("dual.test", Qtype::Aaaa), 1);

        s.set_drop_qtype(Qtype::Aaaa, true);
        let started = Instant::now();
        let ans = resolve_name(&[up(&s)], "dual.test", true, &fast())
            .await
            .unwrap();
        assert_eq!(ans.v4.len(), 1);
        assert!(ans.v6.is_empty());
        assert!(
            ans.partial && ans.aaaa_timed_out,
            "A answered, AAAA missing at the resend tick"
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(100) && elapsed < Duration::from_millis(350),
            "{elapsed:?}"
        );

        s.set_drop_qtype(Qtype::Aaaa, false);
        s.set_drop_qtype(Qtype::A, true);
        let ans = resolve_name(&[up(&s)], "dual.test", true, &fast())
            .await
            .unwrap();
        assert!(ans.v4.is_empty() && ans.v6.len() == 1);
        assert!(ans.partial && !ans.aaaa_timed_out);

        // AAAA empty (no record) is not "timed out"
        s.set_drop_qtype(Qtype::A, false);
        s.set("v4only.test", &["10.0.0.2"], &[], 60);
        let ans = resolve_name(&[up(&s)], "v4only.test", true, &fast())
            .await
            .unwrap();
        assert!(!ans.partial && !ans.aaaa_timed_out && ans.v6.is_empty());
    }
}
