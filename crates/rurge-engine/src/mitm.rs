//! What outlives a config generation of MITM (`phase 4 M2 design 5.4`): the
//! last `Authority` a generation put in use. A generation that is built with
//! the same material takes it on rather than reading the CA again, so that its
//! cached leaves and its TLS sessions are the ones the connections before it
//! were served with.

use rurge_http::mitm::Authority;
use std::sync::{Arc, Mutex};

/// The last `Authority` a published generation had (`phase 4 M2 design 5.4`,
/// the phase 4 / M2a plan's P17). It is remembered when a generation is put in
/// use, never when one is only built: a generation that a failed reload leaves
/// unused leaves nothing behind.
#[derive(Default)]
pub struct MitmShared {
    last: Mutex<Option<Arc<Authority>>>,
}

impl MitmShared {
    pub fn new() -> MitmShared {
        MitmShared::default()
    }

    /// Replaces what is remembered: the generation being published signs with
    /// this authority from now on.
    pub fn remember(&self, authority: &Arc<Authority>) {
        *self.last.lock().expect("the last authority") = Some(Arc::clone(authority));
    }

    /// The authority of the last published generation that had one, or `None`
    /// when none had. A generation with the same material takes it on
    /// (`Runtime::build`); `http-api-tls` is to read it as well (`phase 4 M2
    /// design 14.1`).
    pub fn last_live(&self) -> Option<Arc<Authority>> {
        self.last.lock().expect("the last authority").clone()
    }
}
