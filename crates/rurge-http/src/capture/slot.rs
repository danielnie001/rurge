//! ...

/// Takes the request's body as it is polled into `Idle`.
#[derive(Clone)]
pub(crate) struct RequestTap;

impl RequestTap {
    pub(crate) fn chunk(&self, _data: &[u8]) {}
}
