//! What `[Header Rewrite]` does with a header table (`phase 4 M3 design 7`):
//! the four actions, the framing headers that may not change, and the `Host`
//! a request sends on.
//!
//! The table is changed in place, one rule after another, so that a later rule
//! sees what an earlier one left (`phase 4 M3 design 7.2`). Nothing here reads
//! a value into a note: the notes say which rule acted, and a value may not
//! leave (`phase 4 M3 design 8.5`).

use http::{HeaderValue, Request, Version};

/// What the header rules of one direction came to (`phase 4 M3 design 7.2` –
/// `7.4`): whether the table changed, whether the message's framing changed,
/// and what `Host` became.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Headers {
    pub(crate) changed: bool,
    pub(crate) framing: bool,
    pub(crate) host: HostChange,
}

/// The `Host` a request sends on after the rules of its direction
/// (`phase 4 M3 design 7.4`, the phase 4 / M3a plan's P9).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum HostChange {
    #[default]
    Same,
    Changed(HeaderValue),
}

/// Writes the rewritten `Host` back after the request's form is settled
/// (`phase 4 M3 design 7.4`): `Host` for an HTTP/1.1 upstream, the authority of
/// the request target for an HTTP/2 one. `false` when the value cannot be an
/// authority there.
pub(crate) fn write_back_host<B>(
    req: &mut Request<B>,
    host: &HeaderValue,
    version: Version,
) -> bool {
    let _ = (req, host, version);
    false
}
