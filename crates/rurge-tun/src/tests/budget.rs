//! The budget the buffers of the TUN's TCP sockets come out of.

use crate::tcp::Budget;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Shards take from the budget on threads of their own. However their takes
/// fall, together they keep to the bound `take` documents: the budget and
/// two times `min` for each socket. The threads wait until all of them are
/// there, so that their takes do fall together: more than 16 at once that
/// each read what was left and then added their share could together pass
/// the bound. The assertions hold for any order of the takes; how many fall
/// together decides only whether such takes would be caught.
#[test]
fn takes_at_once_keep_to_the_bound() {
    const ROUNDS: usize = 100;
    // a thread for each processor but the one of the test's own thread
    let threads = std::thread::available_parallelism()
        .map_or(2, |n| n.get().saturating_sub(1))
        .clamp(2, 32);
    // a share of what is left is more than the minimum until most of the
    // budget is gone, so the takes that fall together take shares
    let (budget, min) = (1 << 20, 1);
    for round in 0..ROUNDS {
        let pool = Budget::new(budget, min, usize::MAX);
        let ready = AtomicUsize::new(0);
        let go = AtomicBool::new(false);
        let taken: usize = std::thread::scope(|scope| {
            let takers: Vec<_> = (0..threads)
                .map(|_| {
                    scope.spawn(|| {
                        ready.fetch_add(1, Ordering::AcqRel);
                        while !go.load(Ordering::Acquire) {
                            std::thread::yield_now();
                        }
                        let sizes = pool.take();
                        sizes.rx + sizes.tx
                    })
                })
                .collect();
            while ready.load(Ordering::Acquire) < threads {
                std::thread::yield_now();
            }
            go.store(true, Ordering::Release);
            takers
                .into_iter()
                .map(|taker| taker.join().expect("a taker"))
                .sum()
        });
        assert_eq!(
            pool.used(),
            taken,
            "round {round}: what the sockets took is what is used"
        );
        assert!(
            taken <= budget + 2 * min * threads,
            "round {round}: {threads} sockets took {taken} bytes of a budget of {budget}"
        );
    }
}
