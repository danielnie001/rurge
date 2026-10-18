//! A limit on how often something is let happen, in a window that slides.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Lets at most `max` things through in any `window`: a call is let through
/// when fewer than `max` were let through in the `window` before it.
pub(crate) struct Limiter {
    max: usize,
    window: Duration,
    /// When the ones that were let through were let through, oldest first;
    /// never more than `max`. What is refused is not remembered.
    allowed: VecDeque<Instant>,
}

impl Limiter {
    pub(crate) fn new(max: usize, window: Duration) -> Limiter {
        Limiter {
            max,
            window,
            allowed: VecDeque::new(),
        }
    }

    /// Whether the thing that happens at `now` is let through. A thing that
    /// was let through at `then` counts until `then` is a whole `window` ago.
    /// The `now`s of different callers may come a little out of order, which
    /// only holds an entry a little longer, never lets more through.
    pub(crate) fn allow(&mut self, now: Instant) -> bool {
        while self
            .allowed
            .front()
            .is_some_and(|&then| now.saturating_duration_since(then) >= self.window)
        {
            self.allowed.pop_front();
        }
        if self.allowed.len() >= self.max {
            return false;
        }
        self.allowed.push_back(now);
        true
    }
}
