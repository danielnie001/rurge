//! Host names as they are written onto the wire.

/// `rurge_config::hostname::to_ascii`, under the name the outbounds of this
/// crate call it by: the configuration layer converts a policy's server
/// with the same function, so it lives there.
pub(crate) fn to_ascii(name: &str) -> Option<String> {
    rurge_config::hostname::to_ascii(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What it does is tested where it lives; here, that the outbounds
    /// still reach it.
    #[test]
    fn it_is_the_configuration_layers_conversion() {
        for name in ["example.test", "bücher.example", "a\u{ff20}b.test", ""] {
            assert_eq!(
                to_ascii(name),
                rurge_config::hostname::to_ascii(name),
                "{name:?}"
            );
        }
        assert_eq!(
            to_ascii("bücher.example").as_deref(),
            Some("xn--bcher-kva.example")
        );
        assert_eq!(to_ascii("a\u{ff20}b.test"), None);
    }
}
