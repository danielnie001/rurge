//! What a shard needs to start over besides its own state (phase 3 M3 design
//! 5.3, the last line against smoltcp's panics): why it does, the seed its new
//! stack is made with, and the text of the panic when that was the reason.

use std::any::Any;
use std::fmt;

/// What is added to the seed of a stack when the system's random source gave
/// none for the new one. Odd, so that the sum is never the old seed, and no
/// seed comes round again before every other has; and with its bits all over,
/// so that two seeds are nothing like each other.
pub(crate) const SEED_STEP: u64 = 0x9e37_79b9_7f4a_7c15;

/// The seed of the stack that takes the place of one with the seed `previous`:
/// `drawn`, what the system's random source gave, or the old seed and a step
/// when it gave none. smoltcp takes the initial sequence numbers of its
/// connections from its seed in a fixed order, so a new stack with the old seed
/// would begin where the old one did.
pub(crate) fn next_seed(previous: u64, drawn: Option<u64>) -> u64 {
    drawn.unwrap_or_else(|| previous.wrapping_add(SEED_STEP))
}

/// Why a shard is rebuilt.
pub(crate) enum Cause<'a> {
    /// A turn of the shard's task panicked: the text of the panic, if it had
    /// one.
    Panic(Option<&'a str>),
    /// The lock was found poisoned: another thread panicked with it held (a
    /// stream's write, say), and printed its own message.
    Poisoned,
}

impl fmt::Display for Cause<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Cause::Panic(Some(text)) => write!(f, "panic: {text}"),
            Cause::Panic(None) => f.write_str("panic with no text"),
            Cause::Poisoned => f.write_str("the lock was poisoned by a panic elsewhere"),
        }
    }
}

/// The text of the payload of a panic, if it is text: what `panic!` makes of
/// its arguments is a `&str` or a `String`.
pub(crate) fn panic_text(payload: &(dyn Any + Send)) -> Option<&str> {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
}
