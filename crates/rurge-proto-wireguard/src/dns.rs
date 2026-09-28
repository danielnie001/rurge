//! Destination names inside the tunnel (phase 2 M4 design 6.3): the A and
//! AAAA questions to a section's `dns-server`, their answers, and a small
//! cache of the answers by their TTL.

use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::{Name, RData, RecordType};
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// How many names the cache holds at most.
const CACHE_SIZE: usize = 256;
/// How long an answer is kept at most, whatever its TTL.
const LONGEST_TTL: Duration = Duration::from_secs(3600);

/// Which addresses a question asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Family {
    V4,
    V6,
}

impl Family {
    fn record_type(self) -> RecordType {
        match self {
            Family::V4 => RecordType::A,
            Family::V6 => RecordType::AAAA,
        }
    }
}

/// The question for `name`'s addresses of `family`, with `id`; `None` for
/// a name no question can hold.
pub(crate) fn question(id: u16, name: &str, family: Family) -> Option<Vec<u8>> {
    let mut fqdn = name.trim_end_matches('.').to_ascii_lowercase();
    fqdn.push('.');
    let name = Name::from_ascii(&fqdn).ok()?;
    let mut message = Message::new(id, MessageType::Query, OpCode::Query);
    message.metadata.recursion_desired = true;
    message.add_query(Query::query(name, family.record_type()));
    message.to_vec().ok()
}

/// The addresses in `response`, each with its TTL, when it answers the
/// question `id` asked for `name`: empty for a name that has none. `None`
/// for anything else — the answer to another question, a server failure.
pub(crate) fn answer(
    response: &[u8],
    id: u16,
    name: &str,
    family: Family,
) -> Option<Vec<(IpAddr, u32)>> {
    let message = Message::from_vec(response).ok()?;
    if message.id != id || message.message_type != MessageType::Response {
        return None;
    }
    if !matches!(
        message.response_code,
        ResponseCode::NoError | ResponseCode::NXDomain
    ) {
        return None;
    }
    let asked = message.queries.first()?;
    let asked_name = asked.name().to_ascii();
    if asked.query_type() != family.record_type()
        || !asked_name
            .trim_end_matches('.')
            .eq_ignore_ascii_case(name.trim_end_matches('.'))
    {
        return None;
    }
    Some(
        message
            .answers
            .iter()
            .filter_map(|record| match (&record.data, family) {
                (RData::A(a), Family::V4) => Some((IpAddr::V4(a.0), record.ttl)),
                (RData::AAAA(a), Family::V6) => Some((IpAddr::V6(a.0), record.ttl)),
                _ => None,
            })
            .collect(),
    )
}

/// Answers by name until their TTL runs out; only answers with addresses.
#[derive(Default)]
pub(crate) struct Cache {
    entries: HashMap<String, (Vec<IpAddr>, Instant)>,
}

impl Cache {
    pub(crate) fn get(&self, name: &str, now: Instant) -> Option<Vec<IpAddr>> {
        self.entries
            .get(name)
            .filter(|(_, until)| now < *until)
            .map(|(addrs, _)| addrs.clone())
    }

    pub(crate) fn put(&mut self, name: &str, addrs: Vec<IpAddr>, ttl: Duration, now: Instant) {
        if addrs.is_empty() || ttl.is_zero() {
            return;
        }
        if self.entries.len() >= CACHE_SIZE && !self.entries.contains_key(name) {
            self.entries.retain(|_, (_, until)| now < *until);
            let soonest = self
                .entries
                .iter()
                .min_by_key(|(_, (_, until))| *until)
                .map(|(name, _)| name.clone());
            if self.entries.len() >= CACHE_SIZE
                && let Some(soonest) = soonest
            {
                self.entries.remove(&soonest);
            }
        }
        self.entries
            .insert(name.to_string(), (addrs, now + ttl.min(LONGEST_TTL)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::dns_reply;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn an_answer_is_read_for_the_question_asked() {
        let asked = question(7, "Echo.Test.", Family::V4).unwrap();
        let reply = dns_reply(&asked, |name, family| {
            assert_eq!((name, family), ("echo.test", Family::V4));
            Some(vec![ip("10.0.0.1"), ip("10.0.0.2")])
        })
        .unwrap();
        assert_eq!(
            answer(&reply, 7, "echo.test", Family::V4),
            Some(vec![(ip("10.0.0.1"), 60), (ip("10.0.0.2"), 60)])
        );
        // another question's answer is no answer
        assert_eq!(answer(&reply, 8, "echo.test", Family::V4), None);
        assert_eq!(answer(&reply, 7, "other.test", Family::V4), None);
        assert_eq!(answer(&reply, 7, "echo.test", Family::V6), None);
        assert_eq!(answer(b"garbage", 7, "echo.test", Family::V4), None);
        assert_eq!(
            answer(&asked, 7, "echo.test", Family::V4),
            None,
            "a question"
        );
    }

    /// A name the server does not know has no addresses: an answer all the
    /// same, which ends the search.
    #[test]
    fn a_name_without_addresses_is_an_empty_answer() {
        let asked = question(9, "nx.test", Family::V6).unwrap();
        let reply = dns_reply(&asked, |_, _| None).unwrap();
        assert_eq!(answer(&reply, 9, "nx.test", Family::V6), Some(Vec::new()));
        assert_eq!(
            question(1, "bücher.test", Family::V4),
            None,
            "no IDN before M8"
        );
    }

    #[test]
    fn the_cache_keeps_an_answer_for_its_ttl() {
        let mut cache = Cache::default();
        let now = Instant::now();
        cache.put("a.test", vec![ip("10.0.0.1")], Duration::from_secs(30), now);
        cache.put("none.test", Vec::new(), Duration::from_secs(30), now);
        cache.put("zero.test", vec![ip("10.0.0.2")], Duration::ZERO, now);
        assert_eq!(
            cache.get("a.test", now + Duration::from_secs(29)),
            Some(vec![ip("10.0.0.1")])
        );
        assert_eq!(cache.get("a.test", now + Duration::from_secs(30)), None);
        assert_eq!(cache.get("none.test", now), None);
        assert_eq!(cache.get("zero.test", now), None);
        // a TTL of a day is kept an hour
        cache.put(
            "long.test",
            vec![ip("10.0.0.3")],
            Duration::from_secs(86400),
            now,
        );
        assert_eq!(cache.get("long.test", now + LONGEST_TTL), None);
    }

    #[test]
    fn a_full_cache_makes_room_by_what_runs_out_first() {
        let mut cache = Cache::default();
        let now = Instant::now();
        for i in 0..CACHE_SIZE {
            let ttl = Duration::from_secs(100 + i as u64);
            cache.put(&format!("n{i}.test"), vec![ip("10.0.0.1")], ttl, now);
        }
        cache.put(
            "new.test",
            vec![ip("10.0.0.9")],
            Duration::from_secs(50),
            now,
        );
        assert_eq!(cache.entries.len(), CACHE_SIZE);
        assert_eq!(
            cache.get("n0.test", now),
            None,
            "the one that ran out soonest"
        );
        assert!(cache.get("n1.test", now).is_some());
        assert!(cache.get("new.test", now).is_some());
    }
}
