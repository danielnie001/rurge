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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_is_never_empty_and_is_shared() {
        let a = root_store();
        assert!(!a.is_empty());
        assert!(Arc::ptr_eq(&a, &root_store()));
    }
}
