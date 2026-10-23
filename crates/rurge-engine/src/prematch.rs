//! Pre-matching before a connection exists (phase 3 M3 design 12): what the
//! engine answers when the virtual adapter asks whether a rule refuses a DNS
//! query or a connection that is only being begun, and the record of the
//! refusal.

use crate::control::Mode;
use crate::engine::{Engine, note_sub_rule, rule_raw};
use rurge_config::rule::PolicyRef;
use rurge_config::{PreMatchLayer, SessionInfo};
use rurge_inbound::SessionOutcome;
use rurge_proto::RejectKind;
use rurge_rules::PreMatch;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a rule's refusals are left out of the request log after one has
/// been put there.
pub(crate) const RECORD_EVERY: Duration = Duration::from_secs(5 * 60);

/// When each rule of one generation last had a refusal put in the request log
/// (the rule is its index in the profile). A generation of its own starts
/// from nothing. There is a place for each rule of the profile at most.
#[derive(Default)]
pub(crate) struct Recorded(Mutex<HashMap<usize, Instant>>);

impl Recorded {
    /// Whether a refusal by `rule` at `now` is to be put in the request log:
    /// the first is, and then none until `RECORD_EVERY` after the last that
    /// was. Saying yes is what makes it the last.
    pub(crate) fn due(&self, rule: usize, now: Instant) -> bool {
        let mut recorded = self.0.lock().expect("pre-matching records");
        match recorded.get(&rule) {
            // an instant before the last (a clock read late on another
            // thread) is no time at all since it
            Some(&last) if now.saturating_duration_since(last) < RECORD_EVERY => false,
            _ => {
                recorded.insert(rule, now);
                true
            }
        }
    }
}

impl Engine {
    /// What a pre-matching rule makes of `session`, a DNS query for its name
    /// (`Dns`) or a TCP connection that is being begun (`Syn`): the kind of
    /// refusal, or `None` when no rule refuses it. Only while the outbound
    /// mode is the rule mode, as it is now (not as the generation was built
    /// with), and by the rules of the generation in use. Synchronous, and
    /// never waits: the TUN asks it under its own locks.
    ///
    /// A refusal that `REJECT` and `REJECT-TINYGIF` make counts towards the
    /// upgrade to `REJECT-DROP` as one of a session does, by the destination's
    /// name or address (`connect_session`). It is put in the request log, with
    /// the process behind it looked up, for the first of a rule in five
    /// minutes of a generation (`RECORD_EVERY`); another is neither recorded
    /// nor looked up, and counted all the same.
    pub(crate) fn pre_match_refusal(
        &self,
        layer: PreMatchLayer,
        session: &SessionInfo,
    ) -> Option<RejectKind> {
        if self.mode() != Mode::Rule {
            return None;
        }
        let rt = self.runtime();
        let PreMatch {
            rule,
            policy,
            sub_rule,
        } = rt.rules.pre_match(session, layer)?;
        // what the rule engine keeps for this is a rule with a REJECT-family
        // policy of the built-in kind
        let PolicyRef::Builtin(builtin) = policy else {
            return None;
        };
        let kind = RejectKind::from_builtin(builtin)?;
        let now = Instant::now();
        let refusal = self.escalation.refusal(session, kind, now);
        if rt.pre_matched.due(rule, now) {
            let process = self.look_up_owner(session);
            let handle = self.new_handle_with(session.clone(), process);
            handle.set_rule(rule_raw(&rt.rules, rule));
            // the chain of a session that a built-in policy refused
            // (`PolicyRegistry::resolve`): the policy the rule names, whether
            // or not the refusal was upgraded
            handle.set_policy_chain(vec![builtin.name().to_string()]);
            handle.add_note(match layer {
                PreMatchLayer::Dns => "pre-matching at DNS",
                PreMatchLayer::Syn => "pre-matching at SYN",
            });
            if let Some(hit) = &sub_rule {
                note_sub_rule(&handle, hit);
            }
            handle.finish(SessionOutcome::Rejected(refusal));
        }
        Some(refusal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    /// The five minutes are five minutes: the tests below take the interval from
    /// the constant, so this is where the number itself is held to what the
    /// documents say of it.
    #[test]
    fn the_interval_is_five_minutes() {
        assert_eq!(RECORD_EVERY, Duration::from_secs(300));
    }

    /// The first refusal by a rule is recorded and then none for five minutes,
    /// the minutes counted from the refusal that was recorded; at five minutes
    /// exactly the rule is due again.
    #[test]
    fn a_rule_is_recorded_once_in_five_minutes() {
        let recorded = Recorded::default();
        let t0 = Instant::now();
        assert!(recorded.due(3, t0), "the first refusal is recorded");
        assert!(!recorded.due(3, t0), "not the second, at the same time");
        assert!(!recorded.due(3, t0 + SECOND));
        assert!(!recorded.due(3, t0 + RECORD_EVERY - Duration::from_millis(1)));
        assert!(
            recorded.due(3, t0 + RECORD_EVERY),
            "five minutes after the last that was recorded"
        );
        // and the five minutes begin again from there
        assert!(!recorded.due(3, t0 + RECORD_EVERY + RECORD_EVERY - SECOND));
        assert!(recorded.due(3, t0 + RECORD_EVERY * 2));
    }

    /// A refusal that is not recorded does not put the next off: the five
    /// minutes are counted from the one that was.
    #[test]
    fn a_refusal_that_is_not_recorded_does_not_move_the_five_minutes() {
        let recorded = Recorded::default();
        let t0 = Instant::now();
        assert!(recorded.due(0, t0));
        for seconds in [1, 100, 299] {
            assert!(!recorded.due(0, t0 + SECOND * seconds), "{seconds} s");
        }
        assert!(recorded.due(0, t0 + RECORD_EVERY));
    }

    /// Each rule has its own five minutes.
    #[test]
    fn the_rules_are_counted_apart() {
        let recorded = Recorded::default();
        let t0 = Instant::now();
        assert!(recorded.due(1, t0));
        assert!(recorded.due(2, t0 + SECOND * 10));
        assert!(!recorded.due(1, t0 + SECOND * 20));
        assert!(!recorded.due(2, t0 + SECOND * 20));
        assert!(
            recorded.due(7, t0 + SECOND * 20),
            "a third rule, a first time"
        );
        assert!(recorded.due(1, t0 + RECORD_EVERY));
        assert!(!recorded.due(2, t0 + RECORD_EVERY), "ten seconds early");
        assert!(recorded.due(2, t0 + RECORD_EVERY + SECOND * 10));
    }

    /// A clock read a moment late on another thread can give an instant before
    /// the one remembered: that is no time at all since it, not five minutes.
    #[test]
    fn an_instant_before_the_last_is_no_time_since_it() {
        let recorded = Recorded::default();
        let t0 = Instant::now();
        assert!(recorded.due(5, t0 + SECOND * 10));
        assert!(!recorded.due(5, t0));
        assert!(!recorded.due(5, t0 + SECOND * 10));
    }
}
