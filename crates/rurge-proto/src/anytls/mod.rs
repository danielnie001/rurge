//! `anytls` outbound (manual: Policies › AnyTLS). Frames and padding live
//! here; the session layer and the outbound join them in the next commit.

#[allow(dead_code)] // until the session layer uses it (next commit)
pub(crate) mod frame;
#[allow(dead_code)] // until the session layer uses it (next commit)
pub(crate) mod padding;
