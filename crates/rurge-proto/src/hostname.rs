//! Host names as they are written onto the wire.

/// `name` in the form a proxy request carries: ASCII as it is, an IDN as its
/// A-labels. `None` unless the result (after any IDN conversion) is
/// non-empty and made only of ASCII letters, digits, `-`, `.` and `_` —
/// nothing that could end the host part of an authority (`@ / : ? # [ ] \`)
/// or break a line. The check runs on the converted form, not the input:
/// UTS-46 folds some fullwidth look-alikes (`＠` `／` `：`, ...) onto plain
/// ASCII `@ / :`, which a lenient upstream could then read as authority
/// syntax rather than as part of the host.
pub(crate) fn to_ascii(name: &str) -> Option<String> {
    let ascii = if name.is_ascii() {
        name.to_string()
    } else {
        idna::domain_to_ascii(name).ok()?
    };
    (!ascii.is_empty()
        && ascii
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_')))
    .then_some(ascii)
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
        // a DNS underscore label, a mix of hyphen and underscore, and an
        // already-punycode name: none of these need converting and none are
        // refused by the tightened alphabet below
        for good in [
            "_dmarc.example.test",
            "a-b.c_d.test",
            "xn--bcher-kva.example",
        ] {
            assert_eq!(to_ascii(good).as_deref(), Some(good), "{good:?}");
        }
    }

    #[test]
    fn nothing_unprintable_survives() {
        for bad in [
            "",
            "a.test\r\nX: 1",
            "a b.test",
            "a\0.test",
            "a\u{7f}.test",
            // idna's punycode encoder passes ASCII "basic" code points
            // through unchanged into the "xn--" label instead of encoding
            // them away, so a CR/LF mixed into a non-ASCII label survives
            // the IDN conversion intact (`domain_to_ascii("bü\r\ncher.example")`
            // == `Ok("xn--b\r\ncher-n2a.example")`): the byte-range check
            // below has to run on the converted output, not just refuse
            // non-ASCII input up front.
            "bü\r\ncher.example",
            "a.test\u{3000}b", // fullwidth (ideographic) space
            "a@b.test",
            "a/b.test",
            "a:8080.test",
            // UTS-46 maps these fullwidth look-alikes onto plain ASCII
            // `@` `/` `:`, which a lenient upstream authority parser (e.g. Go's
            // `net/url`) can then read as userinfo / path / port syntax.
            "a\u{ff20}b.test",    // fullwidth commercial at -> '@'
            "a\u{ff0f}b.test",    // fullwidth solidus -> '/'
            "a\u{ff1a}8080.test", // fullwidth colon -> ':'
            "a?b.test",
            "a#b.test",
            "a\\b.test",
        ] {
            assert_eq!(to_ascii(bad), None, "{bad:?}");
        }
    }
}
