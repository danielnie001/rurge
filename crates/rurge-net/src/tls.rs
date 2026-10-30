//! The trust anchors every rurge TLS client starts from.

use std::sync::{Arc, OnceLock};

/// The operating system's root certificates, or `webpki-roots` when none can
/// be loaded. Loaded once per process: reading the native store is slow.
pub fn root_store() -> Arc<rustls::RootCertStore> {
    static ROOTS: OnceLock<Arc<rustls::RootCertStore>> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            let native = rustls_native_certs::load_native_certs();
            let (added, _ignored) = roots.add_parsable_certificates(native.certs);
            if added == 0 {
                tracing::warn!(
                    errors = native.errors.len(),
                    "no native root certificates loaded; using webpki-roots"
                );
                roots
                    .roots
                    .extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            }
            Arc::new(roots)
        })
        .clone()
}

/// The operating system's root certificates **and** every root
/// `webpki-roots` ships, read once per process.
///
/// A decrypted connection verifies the origin it talks to against this store
/// (`phase 4 M2 design 9.3`): Windows fills its root store on demand, so a
/// chain whose root is not there yet — one the browser would have fetched —
/// would fail; the webpki set is the answer for those. The TLS of a policy's
/// outbound keeps `root_store()`.
pub fn union_root_store() -> Arc<rustls::RootCertStore> {
    static ROOTS: OnceLock<Arc<rustls::RootCertStore>> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            let native = rustls_native_certs::load_native_certs();
            let (added, _ignored) = roots.add_parsable_certificates(native.certs);
            if added == 0 {
                tracing::warn!(
                    errors = native.errors.len(),
                    "no native root certificates loaded; the union is webpki-roots alone"
                );
            }
            roots
                .roots
                .extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            Arc::new(roots)
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_is_never_empty_and_is_shared() {
        let a = root_store();
        assert!(!a.is_empty());
        assert!(Arc::ptr_eq(&a, &root_store()));
    }

    /// Every root `webpki-roots` ships is in the union, whatever the native
    /// store gave, and the union is read once (`phase 4 M2 design 9.3`).
    /// Reads the system's store, as `the_store_is_never_empty_and_is_shared`
    /// does: nothing is written.
    #[test]
    fn the_union_holds_every_webpki_root_and_is_shared() {
        let union = union_root_store();
        assert!(!union.is_empty());
        for anchor in webpki_roots::TLS_SERVER_ROOTS.iter() {
            assert!(
                union.roots.iter().any(|r| r.subject == anchor.subject),
                "{:?}",
                anchor.subject
            );
        }
        assert!(Arc::ptr_eq(&union, &union_root_store()));
    }
}
