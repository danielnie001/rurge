//! What a dial leaves of itself for the session that made it: where its TCP
//! connection went and how long it waited for name lookups. The connector
//! and the resolver sit far below the session, with a handful of options
//! structs between them, so instead of every one of those carrying a
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

    /// The address the last TCP connection made under this trail went to.
    pub fn peer(&self) -> Option<SocketAddr> {
        self.left().peer
    }

    /// Forgets the address: what is noted next belongs to another attempt.
    pub fn clear_peer(&self) {
        self.left().peer = None;
    }

    /// The time spent on name lookups; `None` when none was made.
    pub fn dns(&self) -> Option<Duration> {
        self.left().dns
    }

    fn left(&self) -> std::sync::MutexGuard<'_, Left> {
        self.0.lock().expect("trail")
    }
}

/// A TCP connection was made to `addr`; an IPv4-mapped address
/// (`::ffff:a.b.c.d`) is noted as the IPv4 address it is, as the project
/// reads it at every door. Does nothing outside a `follow`.
pub fn connected(addr: SocketAddr) {
    let addr = match addr.ip().to_canonical() {
        IpAddr::V4(v4) => SocketAddr::new(IpAddr::V4(v4), addr.port()),
        // as it is, scope and all
        IpAddr::V6(_) => addr,
    };
    let _ = CURRENT.try_with(|trail| trail.left().peer = Some(addr));
}

/// A name lookup that took `spent` ended. Does nothing outside a `follow`.
pub fn looked_up(spent: Duration) {
    let _ = CURRENT.try_with(|trail| {
        let mut left = trail.left();
        left.dns = Some(left.dns.unwrap_or_default().saturating_add(spent));
    });
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
}
