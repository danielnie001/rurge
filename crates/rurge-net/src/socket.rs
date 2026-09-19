//! Socket options and the connection race behind every direct connection
//! (phase 2 M1 design §5.1).

use crate::connector::interleave;
use rurge_config::spec::IpVersion;
use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::task::JoinSet;
use tokio::time::Instant;

/// Delay between the starts of two connection attempts.
pub const STAGGER: Duration = Duration::from_millis(250);
/// `prefer-v4` / `prefer-v6`: when the other address family joins the race.
pub const OTHER_FAMILY_AFTER: Duration = Duration::from_secs(3);

/// The socket-level part of a policy's common parameters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SocketOpts {
    /// The interface NAME as the platform spells it (e.g. `eth0`,
    /// `Ethernet`); `None` leaves it to the system's routing.
    pub interface: Option<String>,
    /// Fall back to the default interface when `interface` cannot be used.
    pub allow_other_interface: bool,
    pub ip_version: IpVersion,
    /// Which family leads the interleaving (`[General] ipv6`); ignored by the
    /// `prefer-*` and `*-only` modes.
    pub v6_first: bool,
    /// `0` leaves the kernel default: `connect_one` does not call the hook
    /// at all.
    pub tos: u8,
}

/// An IP address family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    V4,
    V6,
}

impl Family {
    /// The family `ip` belongs to.
    pub fn of(ip: &IpAddr) -> Family {
        if ip.is_ipv4() { Family::V4 } else { Family::V6 }
    }
}

/// The two socket options whose spelling differs per platform. Implemented
/// by the binary on top of `rurge-platform` (AR-02); tests use a fake.
pub trait SocketHook: Send + Sync {
    fn bind_interface(
        &self,
        socket: &socket2::Socket,
        interface: &str,
        family: Family,
    ) -> io::Result<()>;
    fn set_tos(&self, socket: &socket2::Socket, family: Family, tos: u8) -> io::Result<()>;
}

/// Does nothing: for commands that never dial (`check`, `rule match`) and tests.
pub struct NoopSocketHook;

impl SocketHook for NoopSocketHook {
    fn bind_interface(&self, _: &socket2::Socket, _: &str, _: Family) -> io::Result<()> {
        Ok(())
    }
    fn set_tos(&self, _: &socket2::Socket, _: Family, _: u8) -> io::Result<()> {
        Ok(())
    }
}

/// Splits resolved addresses into the ones to try first and the ones that
/// join after `OTHER_FAMILY_AFTER` (only the `prefer-*` modes have any).
pub fn plan_addresses(
    addrs: Vec<IpAddr>,
    version: IpVersion,
    v6_first: bool,
) -> (Vec<IpAddr>, Vec<IpAddr>) {
    let split = |addrs: Vec<IpAddr>, v6_preferred: bool| {
        let (v6, v4): (Vec<IpAddr>, Vec<IpAddr>) = addrs.into_iter().partition(IpAddr::is_ipv6);
        let (preferred, other) = if v6_preferred { (v6, v4) } else { (v4, v6) };
        if preferred.is_empty() {
            (other, Vec::new())
        } else {
            (preferred, other)
        }
    };
    match version {
        IpVersion::Dual => (interleave(addrs, v6_first), Vec::new()),
        IpVersion::V4Only => (
            addrs.into_iter().filter(IpAddr::is_ipv4).collect(),
            Vec::new(),
        ),
        IpVersion::V6Only => (
            addrs.into_iter().filter(IpAddr::is_ipv6).collect(),
            Vec::new(),
        ),
        IpVersion::PreferV4 => split(addrs, false),
        IpVersion::PreferV6 => split(addrs, true),
    }
}

/// Connects to the first address that answers. Attempts on `primary` start
/// `STAGGER` apart (at once after a failure); `secondary` joins after
/// `OTHER_FAMILY_AFTER`, or as soon as every primary attempt has failed.
/// Returning drops the attempts still in flight. The caller bounds the total
/// time.
pub async fn race<T, F, Fut>(
    primary: Vec<SocketAddr>,
    secondary: Vec<SocketAddr>,
    connect: F,
) -> io::Result<T>
where
    T: Send + 'static,
    F: Fn(SocketAddr) -> Fut,
    Fut: Future<Output = io::Result<T>> + Send + 'static,
{
    let started = Instant::now();
    let mut queue: VecDeque<SocketAddr> = primary.into();
    let mut secondary = Some(secondary).filter(|s| !s.is_empty());
    let mut attempts: JoinSet<io::Result<T>> = JoinSet::new();
    let mut next_launch = started;
    let mut last = io::Error::new(io::ErrorKind::NotFound, "no addresses to connect to");
    loop {
        if queue.is_empty() && attempts.is_empty() {
            match secondary.take() {
                Some(more) => {
                    queue = more.into();
                    next_launch = Instant::now();
                }
                None => return Err(last),
            }
        }
        tokio::select! {
            _ = tokio::time::sleep_until(next_launch), if !queue.is_empty() => {
                if let Some(addr) = queue.pop_front() {
                    attempts.spawn(connect(addr));
                }
                next_launch = Instant::now() + STAGGER;
            }
            Some(joined) = attempts.join_next(), if !attempts.is_empty() => match joined {
                Ok(Ok(value)) => return Ok(value),
                Ok(Err(e)) => {
                    last = e;
                    next_launch = Instant::now();
                }
                Err(e) => {
                    last = io::Error::other(format!("connection attempt failed: {e}"));
                    next_launch = Instant::now();
                }
            },
            _ = tokio::time::sleep_until(started + OTHER_FAMILY_AFTER),
                if secondary.is_some() && queue.is_empty() =>
            {
                if let Some(more) = secondary.take() {
                    queue = more.into();
                    next_launch = Instant::now();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    fn addr(n: u8) -> SocketAddr {
        SocketAddr::from(([10, 0, 0, n], 80))
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[derive(Clone, Copy)]
    enum Script {
        OkAfter(u64),
        FailAfter(u64),
        Never,
    }

    type Launches = Arc<Mutex<Vec<(SocketAddr, u128)>>>;
    type Attempt = Pin<Box<dyn Future<Output = io::Result<SocketAddr>> + Send>>;

    /// A `connect` function that records when each attempt starts (virtual
    /// milliseconds since `started`) and then follows the script.
    fn scripted(
        scripts: Vec<(SocketAddr, Script)>,
        log: Launches,
        started: Instant,
    ) -> impl Fn(SocketAddr) -> Attempt {
        move |a| {
            log.lock().unwrap().push((a, started.elapsed().as_millis()));
            let script = scripts
                .iter()
                .find(|(x, _)| *x == a)
                .map(|(_, s)| *s)
                .unwrap_or(Script::Never);
            Box::pin(async move {
                match script {
                    Script::OkAfter(ms) => {
                        tokio::time::sleep(Duration::from_millis(ms)).await;
                        Ok(a)
                    }
                    Script::FailAfter(ms) => {
                        tokio::time::sleep(Duration::from_millis(ms)).await;
                        Err(io::Error::new(
                            io::ErrorKind::ConnectionRefused,
                            format!("refused by {a}"),
                        ))
                    }
                    Script::Never => std::future::pending().await,
                }
            })
        }
    }

    async fn run(
        primary: Vec<SocketAddr>,
        secondary: Vec<SocketAddr>,
        scripts: Vec<(SocketAddr, Script)>,
    ) -> (io::Result<SocketAddr>, Vec<(SocketAddr, u128)>, u128) {
        let log: Launches = Arc::default();
        let started = Instant::now();
        let result = race(primary, secondary, scripted(scripts, log.clone(), started)).await;
        let launches = log.lock().unwrap().clone();
        (result, launches, started.elapsed().as_millis())
    }

    #[tokio::test(start_paused = true)]
    async fn attempts_are_staggered_and_the_first_success_wins() {
        let (result, launches, elapsed) = run(
            vec![addr(1), addr(2), addr(3)],
            vec![],
            vec![
                (addr(1), Script::OkAfter(1000)),
                (addr(2), Script::OkAfter(100)),
            ],
        )
        .await;
        assert_eq!(result.unwrap(), addr(2));
        assert_eq!(launches, [(addr(1), 0), (addr(2), 250)]);
        assert_eq!(elapsed, 350);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failure_starts_the_next_attempt_at_once() {
        let (result, launches, _) = run(
            vec![addr(1), addr(2)],
            vec![],
            vec![
                (addr(1), Script::FailAfter(10)),
                (addr(2), Script::OkAfter(5)),
            ],
        )
        .await;
        assert_eq!(result.unwrap(), addr(2));
        assert_eq!(launches, [(addr(1), 0), (addr(2), 10)]);
    }

    #[tokio::test(start_paused = true)]
    async fn the_other_family_joins_after_three_seconds() {
        let (result, launches, _) = run(
            vec![addr(1)],
            vec![addr(2)],
            vec![(addr(1), Script::Never), (addr(2), Script::OkAfter(20))],
        )
        .await;
        assert_eq!(result.unwrap(), addr(2));
        assert_eq!(launches, [(addr(1), 0), (addr(2), 3000)]);
    }

    #[tokio::test(start_paused = true)]
    async fn the_other_family_joins_early_once_the_preferred_one_has_failed() {
        let (result, launches, _) = run(
            vec![addr(1)],
            vec![addr(2)],
            vec![
                (addr(1), Script::FailAfter(50)),
                (addr(2), Script::OkAfter(20)),
            ],
        )
        .await;
        assert_eq!(result.unwrap(), addr(2));
        assert_eq!(launches, [(addr(1), 0), (addr(2), 50)]);
    }

    #[tokio::test(start_paused = true)]
    async fn when_everything_fails_the_last_error_is_returned() {
        let (result, launches, _) = run(
            vec![addr(1)],
            vec![addr(2)],
            vec![
                (addr(1), Script::FailAfter(10)),
                (addr(2), Script::FailAfter(10)),
            ],
        )
        .await;
        let err = result.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionRefused);
        assert_eq!(err.to_string(), format!("refused by {}", addr(2)));
        assert_eq!(launches.len(), 2);
        let (result, launches, _) = run(vec![], vec![], vec![]).await;
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
        assert!(launches.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn the_losers_are_cancelled() {
        struct Flag(Arc<AtomicBool>);
        impl Drop for Flag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let flag = dropped.clone();
        let winner = race(vec![addr(1), addr(2)], vec![], move |a| {
            let guard = (a == addr(1)).then(|| Flag(flag.clone()));
            Box::pin(async move {
                let _guard = guard;
                if a == addr(1) {
                    std::future::pending::<()>().await;
                }
                Ok::<SocketAddr, io::Error>(a)
            }) as Attempt
        })
        .await
        .unwrap();
        assert_eq!(winner, addr(2));
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert!(
            dropped.load(Ordering::SeqCst),
            "the pending attempt was dropped"
        );
    }

    #[test]
    fn addresses_are_planned_per_ip_version() {
        let all = || vec![ip("10.0.0.1"), ip("10.0.0.2"), ip("fd00::1"), ip("fd00::2")];
        let none: Vec<IpAddr> = Vec::new();
        assert_eq!(
            plan_addresses(all(), IpVersion::Dual, false),
            (
                vec![ip("10.0.0.1"), ip("fd00::1"), ip("10.0.0.2"), ip("fd00::2")],
                none.clone()
            )
        );
        assert_eq!(
            plan_addresses(all(), IpVersion::Dual, true).0,
            [ip("fd00::1"), ip("10.0.0.1"), ip("fd00::2"), ip("10.0.0.2")]
        );
        assert_eq!(
            plan_addresses(all(), IpVersion::V4Only, true),
            (vec![ip("10.0.0.1"), ip("10.0.0.2")], none.clone())
        );
        assert_eq!(
            plan_addresses(all(), IpVersion::V6Only, false),
            (vec![ip("fd00::1"), ip("fd00::2")], none.clone())
        );
        assert_eq!(
            plan_addresses(all(), IpVersion::PreferV6, false),
            (
                vec![ip("fd00::1"), ip("fd00::2")],
                vec![ip("10.0.0.1"), ip("10.0.0.2")]
            )
        );
        assert_eq!(
            plan_addresses(all(), IpVersion::PreferV4, true),
            (
                vec![ip("10.0.0.1"), ip("10.0.0.2")],
                vec![ip("fd00::1"), ip("fd00::2")]
            )
        );
        // nothing of the preferred family: the other one goes first, at once
        assert_eq!(
            plan_addresses(vec![ip("10.0.0.1")], IpVersion::PreferV6, false),
            (vec![ip("10.0.0.1")], none.clone())
        );
        assert_eq!(
            plan_addresses(vec![ip("10.0.0.1")], IpVersion::V6Only, false),
            (none.clone(), none)
        );
    }

    #[test]
    fn the_noop_hook_accepts_everything() {
        let socket =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        assert!(
            NoopSocketHook
                .bind_interface(&socket, "nope0", Family::V4)
                .is_ok()
        );
        assert!(NoopSocketHook.set_tos(&socket, Family::V4, 0x10).is_ok());
        assert_eq!(Family::of(&ip("fd00::1")), Family::V6);
        assert_eq!(SocketOpts::default().ip_version, IpVersion::Dual);
    }
}
