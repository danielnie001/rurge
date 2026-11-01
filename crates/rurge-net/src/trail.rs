//! What a dial leaves of itself for the session that made it: the far end its
//! traffic leaves rurge by, and how long it waited for name lookups.
//! The connector and the resolver sit far below the session, with a handful of
//! options structs between them, so instead of every one of those carrying a
//! parameter the session's dial runs inside `Trail::follow` and they write
//! to "the trail of whoever is dialling, if anybody".
//!
//! The scope is a task-local. It reaches everything the dialling task runs
//! itself — `timeout`, `select!` and `join!` included — and nothing it hands
//! to another task (`tokio::spawn`, a `JoinSet`): work there writes nowhere.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

tokio::task_local! {
    static CURRENT: Trail;
}

/// What the dials made under it left. Clones share it.
#[derive(Clone, Default)]
pub struct Trail(Arc<Mutex<Left>>);

#[derive(Default)]
struct Left {
    peer: Option<SocketAddr>,
    /// `None` until a lookup ended: no lookup is not a lookup that took no
    /// time.
    dns: Option<Duration>,
    /// The lookups under way, by their number, and when each began.
    under_way: Vec<(u64, tokio::time::Instant)>,
    next: u64,
}

impl Trail {
    pub fn new() -> Trail {
        Trail::default()
    }

    /// Runs `f` with this trail as the one `connected` and `looked_up` write
    /// to. A `follow` of another trail inside `f` takes over until it ends.
    pub async fn follow<F: Future>(&self, f: F) -> F::Output {
        CURRENT.scope(self.clone(), f).await
    }

    /// The far end the session's traffic leaves rurge by, as the dials and
    /// leases under this trail last noted it: the TCP connection a dial made,
    /// or the connection or carrier a pool or a shared connection lent
    /// (`phase 4 M4 design 10.3`).
    pub fn peer(&self) -> Option<SocketAddr> {
        self.left().peer
    }

    /// Forgets the address: what is noted next belongs to another attempt.
    pub fn clear_peer(&self) {
        self.left().peer = None;
    }

    /// Notes `addr` as the address this trail's connection went to, without a
    /// scope: the engine calls it for a request that borrowed a pooled
    /// connection, whose dial ran under another trail, and the record of the
    /// request takes the address of the dial that made the connection
    /// (`phase 4 M1 design 8.4`). An IPv4-mapped address is noted as the
    /// IPv4 address it is, as `connected` notes one.
    pub fn set_peer(&self, addr: SocketAddr) {
        self.left().peer = Some(canonical(addr));
    }

    /// The time spent on name lookups: those that ended, and those under way up
    /// to now (`phase 4 M4 design 10.4`); `None` when none was begun.
    pub fn dns(&self) -> Option<Duration> {
        let now = tokio::time::Instant::now();
        let left = self.left();
        if left.dns.is_none() && left.under_way.is_empty() {
            return None;
        }
        let under_way = left
            .under_way
            .iter()
            .map(|(_, start)| now.saturating_duration_since(*start))
            .fold(Duration::ZERO, |total, one| total.saturating_add(one));
        Some(left.dns.unwrap_or_default().saturating_add(under_way))
    }

    fn left(&self) -> std::sync::MutexGuard<'_, Left> {
        self.0.lock().expect("trail")
    }
}

/// The session's traffic leaves rurge by a connection or carrier whose far end
/// is `addr`: a TCP connection just made, or a pooled or shared one just lent
/// to the session (`phase 4 M4 design 10.3`). An IPv4-mapped address
/// (`::ffff:a.b.c.d`) is noted as the IPv4 address it is, as the project reads
/// it at every door. Does nothing outside a `follow`.
pub fn connected(addr: SocketAddr) {
    let addr = canonical(addr);
    let _ = CURRENT.try_with(|trail| trail.left().peer = Some(addr));
}

/// The address as this project notes one: an IPv4-mapped address
/// (`::ffff:a.b.c.d`) is the IPv4 address it names; every other address is
/// kept as it is, scope and all.
fn canonical(addr: SocketAddr) -> SocketAddr {
    match addr.ip().to_canonical() {
        IpAddr::V4(v4) => SocketAddr::new(IpAddr::V4(v4), addr.port()),
        IpAddr::V6(_) => addr,
    }
}

/// A name lookup that took `spent` ended. Does nothing outside a `follow`.
pub fn looked_up(spent: Duration) {
    let _ = CURRENT.try_with(|trail| {
        let mut left = trail.left();
        left.dns = Some(left.dns.unwrap_or_default().saturating_add(spent));
    });
}

/// A name lookup under way (`phase 4 M4 design 10.4`): `begin` registers it on
/// the trail being followed, if any; dropping it — when the lookup ends,
/// however it ends, or when a dial gives up on it half way — adds the time it
/// took and lets it go. It ends on the trail it began on, whichever task drops
/// it.
pub struct Lookup {
    trail: Option<Trail>,
    id: u64,
    started: tokio::time::Instant,
}

impl Lookup {
    pub fn begin() -> Lookup {
        let started = tokio::time::Instant::now();
        let trail = current();
        let id = match &trail {
            Some(trail) => {
                let mut left = trail.left();
                let id = left.next;
                left.next = left.next.wrapping_add(1);
                left.under_way.push((id, started));
                id
            }
            None => 0,
        };
        Lookup { trail, id, started }
    }
}

impl Drop for Lookup {
    fn drop(&mut self) {
        let Some(trail) = &self.trail else {
            return;
        };
        let mut left = trail.left();
        if let Some(at) = left.under_way.iter().position(|(id, _)| *id == self.id) {
            left.under_way.swap_remove(at);
        }
        left.dns = Some(
            left.dns
                .unwrap_or_default()
                .saturating_add(self.started.elapsed()),
        );
    }
}

/// The trail being followed now, a clone that shares its record; `None` outside
/// a `follow`. For what notes a far end later and elsewhere: a redial in the
/// relay, a carrier's far end read after its dial.
pub fn current() -> Option<Trail> {
    CURRENT.try_with(Trail::clone).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddrV6;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// What is noted inside a `follow` lands on its trail; before it and after
    /// it, on none.
    #[tokio::test]
    async fn what_is_noted_inside_a_follow_lands_on_its_trail_and_nowhere_else() {
        let trail = Trail::new();
        connected(addr("192.0.2.1:1"));
        looked_up(ms(9));
        trail
            .follow(async {
                connected(addr("192.0.2.2:2"));
                looked_up(ms(3));
            })
            .await;
        connected(addr("192.0.2.3:3"));
        looked_up(ms(9));
        assert_eq!(trail.peer(), Some(addr("192.0.2.2:2")));
        assert_eq!(trail.dns(), Some(ms(3)));
    }

    /// A trail nobody follows stays empty, whatever is noted.
    #[tokio::test]
    async fn noting_outside_any_follow_does_nothing() {
        let trail = Trail::new();
        connected(addr("192.0.2.1:1"));
        looked_up(ms(1));
        assert_eq!((trail.peer(), trail.dns()), (None, None));
    }

    /// `::ffff:a.b.c.d` is the IPv4 address it names, as it is read at every
    /// door of the project: the address of a record is not the one place that
    /// shows the mapped form. Any other address is left as it is.
    #[tokio::test]
    async fn an_ipv4_mapped_peer_is_noted_as_ipv4() {
        let trail = Trail::new();
        trail
            .follow(async { connected(addr("[::ffff:192.0.2.1]:80")) })
            .await;
        assert_eq!(trail.peer(), Some(addr("192.0.2.1:80")));
        let scoped = SocketAddr::V6(SocketAddrV6::new("fe80::1".parse().unwrap(), 80, 0, 7));
        for other in [
            addr("192.0.2.2:81"),
            addr("[2001:db8::1]:80"),
            addr("[::1]:80"),
            scoped,
        ] {
            trail.follow(async { connected(other) }).await;
            assert_eq!(trail.peer(), Some(other));
        }
    }

    #[tokio::test]
    async fn the_same_trail_may_be_followed_inside_itself() {
        let trail = Trail::new();
        trail
            .follow(async {
                connected(addr("192.0.2.1:1"));
                trail
                    .follow(async {
                        connected(addr("192.0.2.2:2"));
                        looked_up(ms(4));
                    })
                    .await;
                looked_up(ms(5));
            })
            .await;
        assert_eq!(trail.peer(), Some(addr("192.0.2.2:2")));
        assert_eq!(trail.dns(), Some(ms(9)));
    }

    /// `timeout` and a `select!` branch poll in the dialling task: the scope
    /// holds.
    #[tokio::test(start_paused = true)]
    async fn a_timeout_and_a_select_branch_still_see_the_trail() {
        let trail = Trail::new();
        trail
            .follow(async {
                tokio::time::timeout(Duration::from_secs(5), async {
                    connected(addr("192.0.2.1:1"));
                })
                .await
                .expect("in time");
                tokio::select! {
                    _ = std::future::pending::<()>() => unreachable!("never ready"),
                    _ = async { looked_up(ms(7)) } => {}
                }
            })
            .await;
        assert_eq!(trail.peer(), Some(addr("192.0.2.1:1")));
        assert_eq!(trail.dns(), Some(ms(7)));
    }

    /// Two dials in flight together, interleaved on one thread, each write to
    /// their own trail.
    #[tokio::test]
    async fn two_follows_at_once_write_each_to_its_own_trail() {
        let (one, two) = (Trail::new(), Trail::new());
        let dial = |trail: &Trail, peer: &str, spent: u64| {
            let (peer, spent) = (addr(peer), ms(spent));
            let trail = trail.clone();
            async move {
                trail
                    .follow(async {
                        tokio::task::yield_now().await;
                        connected(peer);
                        tokio::task::yield_now().await;
                        looked_up(spent);
                    })
                    .await
            }
        };
        tokio::join!(dial(&one, "192.0.2.1:1", 10), dial(&two, "192.0.2.2:2", 20));
        assert_eq!(
            (one.peer(), one.dns()),
            (Some(addr("192.0.2.1:1")), Some(ms(10)))
        );
        assert_eq!(
            (two.peer(), two.dns()),
            (Some(addr("192.0.2.2:2")), Some(ms(20)))
        );
    }

    /// The documented limit: a task the dial spawns does not see the scope.
    #[tokio::test]
    async fn a_task_spawned_inside_a_follow_writes_nowhere() {
        let trail = Trail::new();
        trail
            .follow(async {
                tokio::spawn(async {
                    connected(addr("192.0.2.1:1"));
                    looked_up(ms(5));
                })
                .await
                .unwrap();
            })
            .await;
        assert_eq!((trail.peer(), trail.dns()), (None, None));
    }

    /// `set_peer` writes this trail's own state, from no scope at all: the
    /// engine notes there the address of the connection a request borrowed,
    /// whose dial ran under another trail. The mapped form is the IPv4 one,
    /// and `clear_peer` takes it away again.
    #[test]
    fn set_peer_notes_the_address_on_this_trail() {
        let trail = Trail::new();
        trail.set_peer(addr("[::ffff:192.0.2.1]:80"));
        assert_eq!(trail.peer(), Some(addr("192.0.2.1:80")));
        trail.set_peer(addr("[2001:db8::1]:443"));
        assert_eq!(trail.peer(), Some(addr("[2001:db8::1]:443")));
        trail.clear_peer();
        assert_eq!(trail.peer(), None);
    }

    /// Nothing noted is no lookup; a lookup that took no time is one.
    #[tokio::test]
    async fn lookups_add_up_and_none_is_not_zero() {
        let trail = Trail::new();
        assert_eq!(trail.dns(), None);
        trail
            .follow(async {
                looked_up(Duration::ZERO);
            })
            .await;
        assert_eq!(trail.dns(), Some(Duration::ZERO));
        trail
            .follow(async {
                looked_up(ms(2));
                looked_up(ms(3));
            })
            .await;
        assert_eq!(trail.dns(), Some(ms(5)));
    }

    #[tokio::test]
    async fn a_later_connection_replaces_the_earlier_one_and_clearing_keeps_the_lookups() {
        let trail = Trail::new();
        trail
            .follow(async {
                connected(addr("192.0.2.1:1"));
                looked_up(ms(4));
                connected(addr("192.0.2.2:2"));
            })
            .await;
        assert_eq!(trail.peer(), Some(addr("192.0.2.2:2")));
        trail.clear_peer();
        assert_eq!((trail.peer(), trail.dns()), (None, Some(ms(4))));
    }

    /// A dial cut short — the timeout around it fired — has left what it had
    /// noted by then.
    #[tokio::test(start_paused = true)]
    async fn what_a_dropped_follow_noted_stays() {
        let trail = Trail::new();
        let cut = tokio::time::timeout(
            Duration::from_secs(1),
            trail.follow(async {
                connected(addr("192.0.2.1:1"));
                looked_up(ms(6));
                std::future::pending::<()>().await;
            }),
        )
        .await;
        assert!(cut.is_err(), "the dial was cut short");
        assert_eq!(trail.peer(), Some(addr("192.0.2.1:1")));
        assert_eq!(trail.dns(), Some(ms(6)));
    }

    /// A `follow` of another trail inside one takes over until it ends.
    #[tokio::test]
    async fn an_inner_follow_of_another_trail_shadows_the_outer_until_it_ends() {
        let (outer, inner) = (Trail::new(), Trail::new());
        outer
            .follow(async {
                connected(addr("192.0.2.1:1"));
                inner
                    .follow(async {
                        connected(addr("192.0.2.2:2"));
                        looked_up(ms(2));
                    })
                    .await;
                looked_up(ms(5));
            })
            .await;
        assert_eq!(
            (outer.peer(), outer.dns()),
            (Some(addr("192.0.2.1:1")), Some(ms(5)))
        );
        assert_eq!(
            (inner.peer(), inner.dns()),
            (Some(addr("192.0.2.2:2")), Some(ms(2)))
        );
    }

    /// A lookup still under way counts up to now, and stops counting when it
    /// ends (`phase 4 M4 design 10.4`).
    #[tokio::test(start_paused = true)]
    async fn a_lookup_under_way_counts_up_to_now() {
        let trail = Trail::new();
        let under_way = trail.follow(async { Lookup::begin() }).await;
        tokio::time::advance(ms(30)).await;
        assert_eq!(trail.dns(), Some(ms(30)));
        tokio::time::advance(ms(20)).await;
        assert_eq!(trail.dns(), Some(ms(50)));
        drop(under_way);
        assert_eq!(trail.dns(), Some(ms(50)));
        tokio::time::advance(ms(100)).await;
        assert_eq!(trail.dns(), Some(ms(50)));
    }

    /// What ended and what is under way add up; two lookups under way at once
    /// each count the time they have taken, not once between them.
    #[tokio::test(start_paused = true)]
    async fn lookups_done_and_under_way_add_up() {
        let trail = Trail::new();
        let under_way = trail
            .follow(async {
                looked_up(ms(5));
                Lookup::begin()
            })
            .await;
        tokio::time::advance(ms(10)).await;
        assert_eq!(trail.dns(), Some(ms(15)));
        drop(under_way);
        assert_eq!(trail.dns(), Some(ms(15)));

        let trail = Trail::new();
        let (one, two) = trail
            .follow(async { (Lookup::begin(), Lookup::begin()) })
            .await;
        tokio::time::advance(ms(10)).await;
        assert_eq!(trail.dns(), Some(ms(20)));
        drop((one, two));
        assert_eq!(trail.dns(), Some(ms(20)));
    }

    /// A lookup begun outside any `follow` has no trail to be noted on;
    /// dropping it writes nowhere.
    #[tokio::test(start_paused = true)]
    async fn a_lookup_begun_outside_a_follow_notes_nothing() {
        let trail = Trail::new();
        let under_way = Lookup::begin();
        tokio::time::advance(ms(10)).await;
        drop(under_way);
        assert_eq!(trail.dns(), None);
        assert!(current().is_none());
    }

    /// A lookup ends on the trail it began on, whichever task drops it: the
    /// ticket carries its trail.
    #[tokio::test(start_paused = true)]
    async fn a_lookup_ends_on_the_trail_it_began_on() {
        let (a, b) = (Trail::new(), Trail::new());
        let under_way = a.follow(async { Lookup::begin() }).await;
        tokio::time::advance(ms(10)).await;
        b.follow(async move { drop(under_way) }).await;
        assert_eq!(a.dns(), Some(ms(10)));
        assert_eq!(b.dns(), None);
    }

    /// `current` is what is followed now: none outside a `follow`, the outer
    /// trail inside one, the inner one where a `follow` of another trail takes
    /// over.
    #[tokio::test]
    async fn current_is_the_trail_followed_now() {
        let (outer, inner) = (Trail::new(), Trail::new());
        assert!(current().is_none());
        outer
            .follow(async {
                current()
                    .expect("inside a follow")
                    .set_peer(addr("192.0.2.1:1"));
                inner
                    .follow(async {
                        current()
                            .expect("inside a follow")
                            .set_peer(addr("192.0.2.2:2"));
                    })
                    .await;
            })
            .await;
        assert_eq!(outer.peer(), Some(addr("192.0.2.1:1")));
        assert_eq!(inner.peer(), Some(addr("192.0.2.2:2")));
    }
}
