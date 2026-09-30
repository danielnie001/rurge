//! `obfs=http`: the first client packet is an HTTP upgrade request with the
//! first payload as its body; the server's first packet starts with an
//! upgrade answer. Everything else is raw (phase 2 M6 design 3.2). The
//! template is our own, written from the protocol facts (M6-D3).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use std::sync::OnceLock;

/// The longest server answer head read before giving up.
pub(super) const MAX_RESPONSE_HEAD: usize = 8 * 1024;

/// `curl/7.<a>.<b>`: chosen once per process (`a` in 0..=50, `b` in 0..=1)
/// and reused by every connection, as the reference client does.
fn user_agent() -> &'static str {
    static AGENT: OnceLock<String> = OnceLock::new();
    AGENT.get_or_init(|| {
        let mut pick = [0u8; 2];
        // camouflage, not a secret: a fixed version is still a valid one
        let _ = getrandom::fill(&mut pick);
        format!("curl/7.{}.{}", pick[0] % 51, pick[1] % 2)
    })
}

/// The request head for a first payload of `len` bytes. `host` already
/// carries `:port` when the port is not 80.
pub(super) fn request_head(uri: &str, host: &str, len: usize) -> Vec<u8> {
    let mut key = [0u8; 16];
    let _ = getrandom::fill(&mut key);
    format!(
        "GET {uri} HTTP/1.1\r\n\
         Host: {host}\r\n\
         User-Agent: {}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {}\r\n\
         Content-Length: {len}\r\n\
         \r\n",
        user_agent(),
        STANDARD.encode(key)
    )
    .into_bytes()
}

/// Where the server's answer head ends (the index just past `\r\n\r\n`).
pub(super) fn head_end(data: &[u8]) -> Option<usize> {
    data.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
}

/// An upgrade (`101`) or any success; the reference client checks nothing,
/// but anything else is not an obfs server (a plain web server answering
/// `400` or `404`, say).
pub(super) fn is_upgrade_answer(head: &[u8]) -> bool {
    let line = head.split(|&b| b == b'\r').next().unwrap_or_default();
    let mut parts = line.splitn(3, |&b| b == b' ');
    let version = parts.next().unwrap_or_default();
    let code = parts.next().unwrap_or_default();
    let code = std::str::from_utf8(code)
        .ok()
        .and_then(|c| c.parse::<u16>().ok());
    version.starts_with(b"HTTP/1.") && matches!(code, Some(101 | 200..=299))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_head_has_the_facts_lines_in_order() {
        let head = String::from_utf8(request_head("/cdn?x=1", "edge.example:8388", 42)).unwrap();
        let lines: Vec<&str> = head.split("\r\n").collect();
        assert_eq!(lines[0], "GET /cdn?x=1 HTTP/1.1");
        assert_eq!(lines[1], "Host: edge.example:8388");
        let agent = lines[2].strip_prefix("User-Agent: curl/7.").unwrap();
        let (a, b) = agent.split_once('.').unwrap();
        assert!(a.parse::<u8>().unwrap() <= 50 && b.parse::<u8>().unwrap() <= 1);
        assert_eq!(lines[3], "Upgrade: websocket");
        assert_eq!(lines[4], "Connection: Upgrade");
        let key = lines[5].strip_prefix("Sec-WebSocket-Key: ").unwrap();
        assert_eq!(key.len(), 24);
        assert_eq!(STANDARD.decode(key).unwrap().len(), 16);
        assert_eq!(lines[6], "Content-Length: 42");
        assert_eq!(&lines[7..], ["", ""], "a blank line ends the head");
        // the agent is fixed for the process, the key is fresh every time
        let again = String::from_utf8(request_head("/", "h", 0)).unwrap();
        assert!(again.contains(lines[2]));
        assert!(!again.contains(lines[5]));
    }

    #[test]
    fn the_answer_head_ends_at_the_first_blank_line() {
        assert_eq!(head_end(b"HTTP/1.1 101 OK\r\n\r\nrest"), Some(19));
        assert_eq!(head_end(b"HTTP/1.1 101 OK\r\n\r"), None);
    }

    #[test]
    fn only_an_upgrade_or_a_success_is_an_obfs_answer() {
        for ok in [
            &b"HTTP/1.1 101 Switching Protocols\r\n\r\n"[..],
            b"HTTP/1.0 200 OK\r\n\r\n",
            b"HTTP/1.1 204\r\n\r\n",
        ] {
            assert!(is_upgrade_answer(ok), "{}", String::from_utf8_lossy(ok));
        }
        for no in [
            &b"HTTP/1.1 400 Bad Request\r\n\r\n"[..],
            b"HTTP/1.1 302 Found\r\n\r\n",
            b"SSH-2.0-OpenSSH\r\n\r\n",
            b"HTTP/1.1 1O1 x\r\n\r\n",
            b"\r\n\r\n",
        ] {
            assert!(!is_upgrade_answer(no), "{}", String::from_utf8_lossy(no));
        }
    }
}
