//! Host names as they are written onto the wire.

/// `name` in the form a proxy request carries: ASCII as it is, an IDN as its
/// A-labels. `None` when the result is empty or holds anything outside
/// `0x21..=0x7e` — a space, a control character, a line break — which could
/// break out of a request line or a header.
pub(crate) fn to_ascii(name: &str) -> Option<String> {
    let ascii = if name.is_ascii() {
        name.to_string()
    } else {
        idna::domain_to_ascii(name).ok()?
    };
    (!ascii.is_empty() && ascii.bytes().all(|b| (0x21..=0x7e).contains(&b))).then_some(ascii)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_names_pass_and_idn_names_become_a_labels() {
        assert_eq!(to_ascii("example.test").as_deref(), Some("example.test"));
        assert_eq!(
            to_ascii("bücher.example").as_deref(),
            Some("xn--bcher-kva.example")
        );
        assert_eq!(to_ascii("例子.test").as_deref(), Some("xn--fsqu00a.test"));
    }

    #[test]
    fn nothing_unprintable_survives() {
        for bad in ["", "a.test\r\nX: 1", "a b.test", "a\0.test", "a\u{7f}.test"] {
            assert_eq!(to_ascii(bad), None, "{bad:?}");
        }
    }
}
