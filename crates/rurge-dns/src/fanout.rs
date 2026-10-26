//! Concurrent query engine (design §7.3, manual `dns/overview.html`): every
//! selected upstream is asked at once, the query is re-sent every `resend`
//! until `attempts` rounds went out, the first valid answer wins, and "empty"
//! is reported only when every upstream said so (or some said so and the
//! rest never answered). `resolve_name` asks for the address records of a
//! name and reads them out of the answers. `forward_raw` takes a query of any
//! type as it came and gives back the reply as it came, for
//! `Resolver::forward`, which the TUN's DNS responder asks.

use crate::message::{
    Answer, Qtype, Question, Rcode, build_query, parse_response, random_id, set_wire_id, wire_id,
    wire_is_response, wire_question, wire_rcode,
};
use crate::upstream::{UpstreamError, UpstreamRef};
use std::collections::HashSet;
use std::fmt;
use std::io;
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
            // Every query shares the fanout-wide deadline, so at t == deadline a dropped
            // upstream's per-query timeout and the loop's own deadline fire together; treating
            // a timeout as a failure (rather than "no answer") would make `AllFailed` race
            // `Timeout`, and would block `settle_empty` (which requires `failures.is_empty()`).
            Err(UpstreamError::Timeout) => {}
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
        if round < attempts && Instant::now() >= next_send && Instant::now() < deadline {
            for up in upstreams {
                for t in tracks.iter().filter(|t| !t.decided()) {
                    tracing::debug!(target: "rurge_dns::fanout", upstream = %up.name(), qtype = t.question.qtype.as_str(), round, "send");
                    // Spawned, and the trail relies on it: a connection an upstream makes is not
                    // noted on the session whose dial asked for the lookup. Awaited in place, a
                    // DNS server's address would end up on a session that then connected nowhere.
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
                // The end: every round went out, or — past the deadline —
                // those that are left never will. A fanout that did not get
                // to run from one resend until the deadline (the process was
                // stopped, the machine slept) would otherwise wait for the
                // next resend, whose time has passed, over and over without
                // ever sleeping.
                if round >= attempts || Instant::now() >= deadline {
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

/// Sends `wire` to every upstream at once, again every `opts.resend` to those
/// that have not answered, `opts.attempts` rounds in all, each send with a
/// fresh random ID; takes a reply only when QR is set and its question equals
/// the query's (the name compared without regard to case). The first NOERROR
/// or NXDOMAIN reply wins; a reply with another code is kept and the first of
/// them is handed back once every upstream has replied or the last round's
/// time is up. The reply carries the query's own ID.
pub(crate) async fn forward_raw(
    upstreams: &[UpstreamRef],
    wire: &[u8],
    opts: &FanoutOpts,
) -> io::Result<Vec<u8>> {
    let (Some(client_id), Some(asked)) = (wire_id(wire), wire_question(wire)) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a DNS query",
        ));
    };
    if upstreams.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no upstream to forward to",
        ));
    }
    let start = Instant::now();
    let attempts = opts.attempts.max(1);
    let deadline = start + opts.resend * attempts;
    // the IDs this forward has sent under, the client's own among them
    let mut used = vec![client_id];
    // the upstreams that gave a reply of another code than NOERROR or NXDOMAIN
    let mut replied = vec![false; upstreams.len()];
    let mut kept: Option<Vec<u8>> = None;
    let mut set: JoinSet<Sent> = JoinSet::new();
    let mut round = 0u32;
    let mut next_send = start;
    loop {
        if round < attempts && Instant::now() >= next_send && Instant::now() < deadline {
            for (index, upstream) in upstreams.iter().enumerate() {
                if replied[index] {
                    continue;
                }
                let id = fresh_id(&mut used);
                let mut send = wire.to_vec();
                set_wire_id(&mut send, id);
                tracing::debug!(target: "rurge_dns::fanout", upstream = %upstream.name(), round, "forward send");
                // Spawned for the reason `resolve_name` spawns its queries.
                set.spawn(send_one(index, upstream.clone(), id, send, deadline));
            }
            round += 1;
            next_send = start + opts.resend * round;
        }
        let tick = if round < attempts {
            next_send
        } else {
            deadline
        };
        tokio::select! {
            joined = set.join_next(), if !set.is_empty() => {
                // a task that panicked has no reply to give
                let Some(Ok(sent)) = joined else { continue };
                let upstream = upstreams[sent.upstream].name();
                let elapsed_ms = sent.elapsed.as_millis() as u64;
                let mut reply = match sent.result {
                    Ok(reply) if is_reply_to(&reply, sent.id, &asked) => reply,
                    Ok(_) => {
                        tracing::debug!(target: "rurge_dns::fanout", upstream = %upstream, elapsed_ms, "forward ignored a message that is no reply to the send");
                        continue;
                    }
                    Err(e) => {
                        tracing::debug!(target: "rurge_dns::fanout", upstream = %upstream, error = %e, elapsed_ms, "forward failed");
                        continue;
                    }
                };
                set_wire_id(&mut reply, client_id);
                let rcode = wire_rcode(&reply);
                let told = rcode.map_or_else(|| "unknown".to_string(), |rcode| rcode.to_string());
                tracing::debug!(target: "rurge_dns::fanout", upstream = %upstream, rcode = %told, elapsed_ms, "forward answer");
                if matches!(rcode, Some(Rcode::NoError | Rcode::NxDomain)) {
                    return Ok(reply);
                }
                replied[sent.upstream] = true;
                kept.get_or_insert(reply);
                if replied.iter().all(|done| *done) {
                    break;
                }
            }
            _ = sleep_until(tick) => {
                // The end, as in `resolve_name`: every round went out and the
                // last one's time is up, or the deadline has passed.
                if round >= attempts || Instant::now() >= deadline {
                    break;
                }
            }
        }
    }
    kept.ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "no upstream answered"))
}

/// What one send of `forward_raw` came back with.
struct Sent {
    /// The upstream it went to, by its place in the list.
    upstream: usize,
    /// The ID it went out under.
    id: u16,
    elapsed: Duration,
    result: Result<Vec<u8>, UpstreamError>,
}

async fn send_one(
    upstream: usize,
    to: UpstreamRef,
    id: u16,
    wire: Vec<u8>,
    deadline: Instant,
) -> Sent {
    let started = Instant::now();
    let result = to.query(&wire, deadline).await;
    Sent {
        upstream,
        id,
        elapsed: started.elapsed(),
        result,
    }
}

/// An ID that is none of `used`, added to them. (There are never so many
/// sends of one forward that none is left.)
fn fresh_id(used: &mut Vec<u16>) -> u16 {
    loop {
        let id = random_id();
        if !used.contains(&id) {
            used.push(id);
            return id;
        }
    }
}

/// Whether `reply` answers the question `asked` of the send under `id`: it is a
/// response, it carries that ID, and its question is the one asked — a name
/// that differs in case only is the same name.
fn is_reply_to(reply: &[u8], id: u16, asked: &(String, u16, u16)) -> bool {
    wire_is_response(reply)
        && wire_id(reply) == Some(id)
        && wire_question(reply).as_ref() == Some(asked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Rcode, wire_id, wire_rcode};
    use crate::testing::{MockDns, direct, query_wire, txt_answers};
    use crate::upstream::Upstream;
    use crate::upstream::udp::UdpUpstream;
    use rurge_net::BoxFuture;
    use std::io::ErrorKind;
    use std::sync::Arc;
    use tokio::time::timeout;

    const BOUND: Duration = Duration::from_secs(10);

    fn fast() -> FanoutOpts {
        FanoutOpts {
            resend: Duration::from_millis(100),
            attempts: 4,
        }
    }

    /// The mock over UDP, for the tests that run on the real clock: the two
    /// on a multi-thread runtime, which has no paused clock, and
    /// `server_failures_are_reported`, which asserts neither a count nor a
    /// time. The others ask the mock in memory (`MockDns::upstream`) on a
    /// paused clock, where no answer is late for a timer of the fanout's and
    /// a time it takes is exactly the time it waits.
    fn up(server: &MockDns) -> UpstreamRef {
        Arc::new(UdpUpstream::new(server.addr(), direct()))
    }

    #[tokio::test(start_paused = true)]
    async fn single_upstream_answers() {
        let s = MockDns::spawn().await;
        s.set("a.test", &["10.0.0.1", "10.0.0.2"], &[], 300);
        let ans = resolve_name(&[s.upstream()], "a.test", false, &fast())
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
        assert_eq!(ans.upstream, format!("memory://{}", s.addr()));
        assert_eq!(s.query_count("a.test", Qtype::A), 1);
        assert_eq!(
            s.query_count("a.test", Qtype::Aaaa),
            0,
            "AAAA not asked when want_v6 is false"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn first_valid_answer_wins() {
        let slow = MockDns::spawn().await;
        slow.set("a.test", &["10.0.0.9"], &[], 60);
        slow.set_delay(Duration::from_millis(300));
        let fast_srv = MockDns::spawn().await;
        fast_srv.set("a.test", &["10.0.0.1"], &[], 60);
        let started = Instant::now();
        let ans = resolve_name(
            &[slow.upstream(), fast_srv.upstream()],
            "a.test",
            false,
            &fast(),
        )
        .await
        .unwrap();
        assert_eq!(ans.v4[0].0, "10.0.0.1".parse::<Ipv4Addr>().unwrap());
        assert_eq!(ans.upstream, format!("memory://{}", fast_srv.addr()));
        assert_eq!(
            started.elapsed(),
            Duration::ZERO,
            "the fast answer ends the fanout; the slow one's 300 ms are not waited for"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn resends_after_the_timer_and_recovers() {
        let s = MockDns::spawn().await;
        s.set("a.test", &["10.0.0.1"], &[], 60);
        s.set_drop_first(1);
        let started = Instant::now();
        let ans = resolve_name(&[s.upstream()], "a.test", false, &fast())
            .await
            .unwrap();
        assert_eq!(ans.v4.len(), 1);
        assert_eq!(
            started.elapsed(),
            fast().resend,
            "answer came from the second round, sent at the first resend"
        );
        assert_eq!(s.query_count("a.test", Qtype::A), 2);
    }

    /// What only a multi-thread runtime shows: a dropped upstream's own
    /// `Timeout` and the loop's deadline are one instant and can be seen in
    /// either order, and the result is the timeout, not `AllFailed`: a
    /// question's own timeout is not counted as a failure. That every round
    /// goes out, and when the fanout ends, depend on the clock and are
    /// `all_dropped_sends_every_round_and_times_out_at_the_deadline`'s.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn all_dropped_is_a_timeout_on_a_multi_thread_runtime() {
        let s = MockDns::spawn().await;
        s.set("a.test", &["10.0.0.1"], &[], 60);
        s.set_drop_all(true);
        let err = resolve_name(&[up(&s)], "a.test", false, &fast())
            .await
            .unwrap_err();
        assert_eq!(err, FanoutError::Timeout);
    }

    /// Nothing answers: every round goes out, one question each, and the
    /// fanout ends as a timeout at the deadline, `resend × attempts` after it
    /// began.
    #[tokio::test(start_paused = true)]
    async fn all_dropped_sends_every_round_and_times_out_at_the_deadline() {
        let s = MockDns::spawn().await;
        s.set("a.test", &["10.0.0.1"], &[], 60);
        s.set_drop_all(true);
        let opts = fast();
        let started = Instant::now();
        let err = resolve_name(&[s.upstream()], "a.test", false, &opts)
            .await
            .unwrap_err();
        assert_eq!(err, FanoutError::Timeout);
        assert_eq!(started.elapsed(), opts.resend * opts.attempts);
        assert_eq!(s.query_count("a.test", Qtype::A), 4);
    }

    /// Without options of its own a fanout is the manual's (DNS Overview): the
    /// question goes out again every second, and after five rounds, about five
    /// seconds in all, the lookup fails.
    #[tokio::test(start_paused = true)]
    async fn by_default_the_question_is_repeated_every_second_for_five_rounds() {
        let opts = FanoutOpts::default();
        assert_eq!((opts.resend, opts.attempts), (Duration::from_secs(1), 5));
        let s = MockDns::spawn().await;
        s.set("a.test", &["10.0.0.1"], &[], 60);
        s.set_drop_all(true);
        let started = Instant::now();
        let err = resolve_name(&[s.upstream()], "a.test", false, &opts)
            .await
            .unwrap_err();
        assert_eq!(err, FanoutError::Timeout);
        assert_eq!(started.elapsed(), Duration::from_secs(5));
        assert_eq!(s.query_count("a.test", Qtype::A), 5);
    }

    /// A fanout that does not get to run from one of its rounds until its
    /// deadline has passed — the process was stopped, the machine slept or
    /// was too busy — has rounds it never sent and no time to send them in:
    /// it ends as the timeout it is, and does not go on waiting for a resend
    /// whose time has come and gone.
    #[tokio::test]
    async fn a_fanout_kept_from_running_until_past_its_deadline_ends_as_a_timeout() {
        // A server that is not there: a question to it ends at its deadline,
        // unanswered. The mock records every question as it comes.
        let mock = MockDns::spawn().await;
        mock.set_drop_all(true);
        let upstreams: Vec<UpstreamRef> = vec![mock.upstream()];
        let opts = fast();
        let whole = opts.resend * opts.attempts;
        let fanout =
            tokio::spawn(async move { resolve_name(&upstreams, "a.test", false, &opts).await });
        timeout(BOUND, async {
            while mock.query_count("a.test", Qtype::A) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the first round goes out within the bound");
        // The fanout waits for a resend, and runs next when the clock is
        // past its deadline: nothing of it runs in between.
        let sent = mock.query_count("a.test", Qtype::A);
        tokio::time::pause();
        tokio::time::advance(whole).await;
        tokio::time::resume();
        let outcome = timeout(BOUND, fanout)
            .await
            .expect("a fanout past its deadline ends within the bound");
        assert_eq!(outcome.unwrap(), Err(FanoutError::Timeout));
        assert_eq!(
            mock.query_count("a.test", Qtype::A),
            sent,
            "no round goes out past the deadline"
        );
    }

    /// The same wake-up past the deadline, with the A answer taken and the
    /// AAAA answer never to come: the fanout hands over the address it has,
    /// and what it hands over is what it hands over when it ends at the first
    /// resend instead — the two ways out of the loop do not differ in the
    /// value. With the A answer in hand the partial-result exit leaves the
    /// loop at that wake-up whether or not the deadline check is there, so
    /// this pins the value, not the check the test above is for.
    #[tokio::test(start_paused = true)]
    async fn a_fanout_kept_from_running_until_past_its_deadline_hands_over_what_it_has() {
        let mock = MockDns::spawn().await;
        mock.set("dual.test", &["10.0.0.1"], &["fd00::1"], 60);
        mock.set_drop_qtype(Qtype::Aaaa, true);
        let upstreams: Vec<UpstreamRef> = vec![mock.upstream()];
        let opts = fast();
        let whole = opts.resend * opts.attempts;
        let expected = Answers {
            v4: vec![("10.0.0.1".parse().unwrap(), 60)],
            v6: Vec::new(),
            upstream: format!("memory://{}", mock.addr()),
            partial: true,
            aaaa_timed_out: true,
        };
        // The ordinary way out: A is in, and the first resend finds AAAA missing.
        let ordinary = resolve_name(&upstreams, "dual.test", true, &opts)
            .await
            .unwrap();
        assert_eq!(ordinary, expected);

        let late = upstreams.clone();
        let fanout =
            tokio::spawn(async move { resolve_name(&late, "dual.test", true, &opts).await });
        // On this clock a sleep ends when every other task has run out of
        // work: the first round is out and the fanout has taken A's answer.
        tokio::time::sleep(Duration::from_millis(1)).await;
        tokio::time::advance(whole).await;
        // The clock is the real one again, so that a fanout that does not end
        // fails the bound instead of waiting for a paused clock.
        tokio::time::resume();
        let outcome = timeout(BOUND, fanout)
            .await
            .expect("a fanout past its deadline ends within the bound");
        assert_eq!(outcome.unwrap().unwrap(), expected);
    }

    #[tokio::test(start_paused = true)]
    async fn empty_answer_rules() {
        // all empty → EmptyAnswer immediately
        let e1 = MockDns::spawn().await;
        e1.set_empty("nx.test");
        let e2 = MockDns::spawn().await;
        e2.set_empty("nx.test");
        let started = Instant::now();
        assert_eq!(
            resolve_name(&[e1.upstream(), e2.upstream()], "nx.test", false, &fast()).await,
            Err(FanoutError::EmptyAnswer)
        );
        assert_eq!(started.elapsed(), Duration::ZERO);
        // one empty + one dropping → EmptyAnswer at the deadline
        let d = MockDns::spawn().await;
        d.set_drop_all(true);
        let started = Instant::now();
        assert_eq!(
            resolve_name(&[e1.upstream(), d.upstream()], "nx.test", false, &fast()).await,
            Err(FanoutError::EmptyAnswer)
        );
        assert_eq!(started.elapsed(), fast().resend * fast().attempts);
        // one empty + one valid → valid
        let v = MockDns::spawn().await;
        v.set("nx.test", &["10.0.0.5"], &[], 60);
        let ans = resolve_name(&[e1.upstream(), v.upstream()], "nx.test", false, &fast())
            .await
            .unwrap();
        assert_eq!(ans.upstream, format!("memory://{}", v.addr()));
        // unknown names answer NXDOMAIN, which is also "empty"
        assert_eq!(
            resolve_name(&[v.upstream()], "unknown.test", false, &fast()).await,
            Err(FanoutError::EmptyAnswer)
        );
    }

    /// On a multi-thread runtime, a dropped upstream's per-query `Timeout` and the loop's own
    /// deadline sleep can be observed in either order (design note: both target the identical
    /// `Instant`). This must still resolve to `EmptyAnswer`, not `AllFailed`, since the other
    /// upstream did answer empty.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_plus_silent_upstream_is_empty_answer_on_multi_thread() {
        let e = MockDns::spawn().await;
        e.set_empty("nx.test");
        let d = MockDns::spawn().await;
        d.set_drop_all(true);
        assert_eq!(
            resolve_name(&[up(&e), up(&d)], "nx.test", false, &fast()).await,
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
                assert!(list[0].1.contains("SERVFAIL"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            resolve_name(&[], "a.test", false, &fast()).await,
            Err(FanoutError::AllFailed(Vec::new()))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_and_aaaa_in_parallel_with_partial_results() {
        let s = MockDns::spawn().await;
        s.set("dual.test", &["10.0.0.1"], &["fd00::1"], 60);
        let ans = resolve_name(&[s.upstream()], "dual.test", true, &fast())
            .await
            .unwrap();
        assert_eq!(ans.v4.len(), 1);
        assert_eq!(ans.v6.len(), 1);
        assert!(!ans.partial && !ans.aaaa_timed_out);
        assert_eq!(s.query_count("dual.test", Qtype::Aaaa), 1);

        s.set_drop_qtype(Qtype::Aaaa, true);
        let started = Instant::now();
        let ans = resolve_name(&[s.upstream()], "dual.test", true, &fast())
            .await
            .unwrap();
        assert_eq!(ans.v4.len(), 1);
        assert!(ans.v6.is_empty());
        assert!(
            ans.partial && ans.aaaa_timed_out,
            "A answered, AAAA missing at the resend tick"
        );
        assert_eq!(
            started.elapsed(),
            fast().resend,
            "the partial result comes at the first resend"
        );

        s.set_drop_qtype(Qtype::Aaaa, false);
        s.set_drop_qtype(Qtype::A, true);
        let ans = resolve_name(&[s.upstream()], "dual.test", true, &fast())
            .await
            .unwrap();
        assert!(ans.v4.is_empty() && ans.v6.len() == 1);
        assert!(ans.partial && !ans.aaaa_timed_out);

        // AAAA empty (no record) is not "timed out"
        s.set_drop_qtype(Qtype::A, false);
        s.set("v4only.test", &["10.0.0.2"], &[], 60);
        let ans = resolve_name(&[s.upstream()], "v4only.test", true, &fast())
            .await
            .unwrap();
        assert!(!ans.partial && !ans.aaaa_timed_out && ans.v6.is_empty());
    }

    // What `forward_raw` does with a query as it comes. All on the paused
    // clock and in memory (`MockDns::upstream`): the time a forward takes is
    // exactly the time it waits for.

    const TXT: u16 = 16;

    fn txt_query(id: u16) -> Vec<u8> {
        query_wire(id, "a.test", TXT, None)
    }

    /// An upstream that asks `inner` and hands its reply on as `rewrite`
    /// leaves it: for the replies a server may give and the mock never does.
    struct Rewriting {
        name: String,
        inner: UpstreamRef,
        rewrite: fn(&mut [u8]),
    }

    impl Rewriting {
        fn over(inner: UpstreamRef, rewrite: fn(&mut [u8])) -> UpstreamRef {
            Arc::new(Rewriting {
                name: format!("rewriting {}", inner.name()),
                inner,
                rewrite,
            })
        }
    }

    impl Upstream for Rewriting {
        fn name(&self) -> &str {
            &self.name
        }

        fn query<'a>(
            &'a self,
            wire: &'a [u8],
            deadline: Instant,
        ) -> BoxFuture<'a, Result<Vec<u8>, UpstreamError>> {
            Box::pin(async move {
                let mut reply = self.inner.query(wire, deadline).await?;
                (self.rewrite)(&mut reply);
                Ok(reply)
            })
        }
    }

    /// The name of the question of `a.test` stands at byte 12: the length
    /// of its first label, then the label.
    fn another_name(reply: &mut [u8]) {
        reply[13] = b'b';
    }

    fn name_in_capitals(reply: &mut [u8]) {
        reply[12..20].make_ascii_uppercase();
    }

    fn name_in_small_letters(reply: &mut [u8]) {
        reply[12..20].make_ascii_lowercase();
    }

    fn another_id(reply: &mut [u8]) {
        reply[0] ^= 0xff;
    }

    fn not_a_response(reply: &mut [u8]) {
        reply[2] &= !0x80;
    }

    /// The first NOERROR or NXDOMAIN reply ends the forward, whichever
    /// upstream it comes from and whatever the others said before it; one
    /// that has replied is not asked again.
    #[tokio::test(start_paused = true)]
    async fn forward_takes_the_first_noerror_or_nxdomain() {
        let servfail = MockDns::spawn().await;
        servfail.set_rcode("a.test", Rcode::ServFail);
        let slow = MockDns::spawn().await;
        slow.set_txt("a.test", "slow");
        slow.set_delay(Duration::from_millis(250));
        let started = Instant::now();
        let reply = forward_raw(
            &[servfail.upstream(), slow.upstream()],
            &txt_query(7),
            &fast(),
        )
        .await
        .unwrap();
        assert_eq!(wire_rcode(&reply), Some(Rcode::NoError));
        assert_eq!(txt_answers(&reply), ["slow"]);
        assert_eq!(wire_id(&reply), Some(7));
        assert_eq!(started.elapsed(), Duration::from_millis(250));
        // the one that said SERVFAIL was asked the once; the other at the
        // start and at each resend until its answer came
        assert_eq!(servfail.wires().len(), 1);
        assert_eq!(slow.wires().len(), 3);

        // NXDOMAIN is as good, and ends it at once
        let nowhere = MockDns::spawn().await;
        let started = Instant::now();
        let reply = forward_raw(
            &[slow.upstream(), nowhere.upstream()],
            &txt_query(8),
            &fast(),
        )
        .await
        .unwrap();
        assert_eq!(wire_rcode(&reply), Some(Rcode::NxDomain));
        assert_eq!(wire_id(&reply), Some(8));
        assert_eq!(started.elapsed(), Duration::ZERO);
    }

    /// When nothing better comes the first of the other codes is handed
    /// back: as soon as every upstream has replied, and if one never does,
    /// when the last round's time is up.
    #[tokio::test(start_paused = true)]
    async fn forward_returns_another_code_when_nothing_better_comes() {
        let servfail = MockDns::spawn().await;
        servfail.set_rcode("a.test", Rcode::ServFail);
        let refused = MockDns::spawn().await;
        refused.set_rcode("a.test", Rcode::Refused);
        refused.set_delay(Duration::from_millis(50));
        let started = Instant::now();
        // REFUSED is listed first, SERVFAIL comes first
        let reply = forward_raw(
            &[refused.upstream(), servfail.upstream()],
            &txt_query(7),
            &fast(),
        )
        .await
        .unwrap();
        assert_eq!(wire_rcode(&reply), Some(Rcode::ServFail));
        assert_eq!(wire_id(&reply), Some(7));
        assert_eq!(
            started.elapsed(),
            Duration::from_millis(50),
            "not the deadline"
        );

        let silent = MockDns::spawn().await;
        silent.set_drop_all(true);
        let started = Instant::now();
        let reply = forward_raw(
            &[silent.upstream(), servfail.upstream()],
            &txt_query(7),
            &fast(),
        )
        .await
        .unwrap();
        assert_eq!(wire_rcode(&reply), Some(Rcode::ServFail));
        assert_eq!(started.elapsed(), fast().resend * fast().attempts);
        assert_eq!(silent.wires().len(), 4, "asked at every round");
        // asked once before, and once now, though the forward went on to the
        // last round
        assert_eq!(servfail.wires().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn forward_resends_to_a_silent_upstream() {
        let mock = MockDns::spawn().await;
        mock.set_txt("a.test", "hello");
        mock.set_drop_first(1);
        let started = Instant::now();
        let reply = forward_raw(&[mock.upstream()], &txt_query(7), &fast())
            .await
            .unwrap();
        assert_eq!(txt_answers(&reply), ["hello"]);
        assert_eq!(started.elapsed(), fast().resend);
        assert_eq!(mock.wires().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn forward_times_out_without_any_reply() {
        let mock = MockDns::spawn().await;
        mock.set_txt("a.test", "hello");
        mock.set_drop_all(true);
        let started = Instant::now();
        let err = forward_raw(&[mock.upstream()], &txt_query(7), &fast())
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::TimedOut);
        assert_eq!(started.elapsed(), fast().resend * fast().attempts);
        assert_eq!(mock.wires().len(), 4, "every round went out");
    }

    /// A reply to another question than the one asked is nobody's reply:
    /// not taken, and the upstream is asked again.
    #[tokio::test(start_paused = true)]
    async fn forward_ignores_a_reply_to_another_question() {
        let right = MockDns::spawn().await;
        right.set_txt("a.test", "right");
        right.set_delay(Duration::from_millis(250));
        let wrong = MockDns::spawn().await;
        wrong.set_txt("a.test", "wrong");
        let wrong_up = Rewriting::over(wrong.upstream(), another_name);
        let started = Instant::now();
        let reply = forward_raw(
            &[wrong_up.clone(), right.upstream()],
            &txt_query(7),
            &fast(),
        )
        .await
        .unwrap();
        assert_eq!(txt_answers(&reply), ["right"]);
        assert_eq!(started.elapsed(), Duration::from_millis(250));
        assert_eq!(wrong.wires().len(), 3, "asked again at each resend");

        // alone, it gets nothing
        let started = Instant::now();
        let err = forward_raw(&[wrong_up], &txt_query(7), &fast())
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::TimedOut);
        assert_eq!(started.elapsed(), fast().resend * fast().attempts);
    }

    /// A reply whose question is the one asked but for the case of its name —
    /// what a server sends back that changes the case of a name, or a client
    /// that mixes it sent one — answers that question: the case of a name is
    /// no part of it. The reply is handed back as it came.
    #[tokio::test(start_paused = true)]
    async fn forward_accepts_a_reply_whose_question_differs_only_in_case() {
        let mock = MockDns::spawn().await;
        mock.set_txt("a.test", "hello");
        let capitals = Rewriting::over(mock.upstream(), name_in_capitals);
        let started = Instant::now();
        let reply = forward_raw(&[capitals], &txt_query(7), &fast())
            .await
            .unwrap();
        assert_eq!(started.elapsed(), Duration::ZERO);
        assert_eq!(txt_answers(&reply), ["hello"]);
        assert_eq!(wire_id(&reply), Some(7));
        assert_eq!(&reply[12..20], b"\x01A\x04TEST\x00");
        // and the other way round: a query in capitals, a reply in small letters
        let small = Rewriting::over(mock.upstream(), name_in_small_letters);
        let reply = forward_raw(&[small], &query_wire(8, "A.TeSt", TXT, None), &fast())
            .await
            .unwrap();
        assert_eq!(txt_answers(&reply), ["hello"]);
        assert_eq!(&reply[12..20], b"\x01a\x04test\x00");
    }

    /// Every send has an ID of its own — none the client's, none another
    /// send's, though it is sent again and to several upstreams — and the
    /// reply has the client's again.
    #[tokio::test(start_paused = true)]
    async fn each_send_has_a_fresh_id() {
        let a = MockDns::spawn().await;
        let b = MockDns::spawn().await;
        for mock in [&a, &b] {
            mock.set_txt("a.test", "hello");
            mock.set_drop_first(2);
        }
        let reply = forward_raw(&[a.upstream(), b.upstream()], &txt_query(0x4242), &fast())
            .await
            .unwrap();
        assert_eq!(wire_id(&reply), Some(0x4242));
        let ids: Vec<u16> = a
            .wires()
            .iter()
            .chain(b.wires().iter())
            .map(|wire| wire_id(wire).unwrap())
            .collect();
        // three rounds to two upstreams; the last of the other may not have
        // been seen when the first answer ended it
        assert!(ids.len() >= 5, "{ids:?}");
        let distinct: HashSet<u16> = ids.iter().copied().collect();
        assert_eq!(distinct.len(), ids.len(), "{ids:?}");
        assert!(!distinct.contains(&0x4242), "{ids:?}");
        // all else of the query is as it was
        for wire in a.wires().iter().chain(b.wires().iter()) {
            assert_eq!(wire[2..], txt_query(0)[2..]);
        }
    }

    /// A message that has another ID than the send, or is no response, is no
    /// reply to it.
    #[tokio::test(start_paused = true)]
    async fn forward_ignores_what_is_no_reply_to_the_send() {
        let mock = MockDns::spawn().await;
        mock.set_txt("a.test", "hello");
        for rewrite in [another_id as fn(&mut [u8]), not_a_response] {
            let up = Rewriting::over(mock.upstream(), rewrite);
            let started = Instant::now();
            let err = forward_raw(&[up], &txt_query(7), &fast())
                .await
                .unwrap_err();
            assert_eq!(err.kind(), ErrorKind::TimedOut);
            assert_eq!(started.elapsed(), fast().resend * fast().attempts);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn forward_needs_a_query_and_an_upstream() {
        let mock = MockDns::spawn().await;
        let err = forward_raw(&[], &txt_query(7), &fast()).await.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
        let err = forward_raw(&[mock.upstream()], b"short", &fast())
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(mock.wires().is_empty());
    }
}
