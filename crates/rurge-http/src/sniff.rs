//! Judging what the first bytes of a connection are, and waiting for them
//! (phase 4 M1 design 6.2, 6.3).

use std::time::Duration;

use tokio::io::AsyncRead;
use tokio::time::{Instant, timeout_at};

use crate::prefix::Prefixed;
use crate::tls::{self, ClientHello};

/// How long a sniffable session's first bytes are waited for (phase 4 M1
/// design 6.3, M1-D5).
pub const SNIFF_WAIT: Duration = Duration::from_secs(15);

/// The most of a prefix that is read to judge it (design 6.2).
pub const MAX_PREFIX: usize = 64 * 1024;

/// What the bytes at the start of a connection are (design 6.2).
#[derive(Debug, PartialEq, Eq)]
pub enum Judged {
    /// Not enough to tell. Telling needs at least this many bytes in all,
    /// always more than there are and never more than `MAX_PREFIX` (P3 of the
    /// phase 4 / M1a plan).
    NeedMore(usize),
    Tls(ClientHello),
    Http,
    Other,
}

/// The most bytes a method token may have (design 6.2).
const MAX_METHOD: usize = 20;

/// Says what the bytes at the start of a connection are (phase 4 M1 design
/// 6.2), or how many bytes in all would tell.
pub fn judge(prefix: &[u8]) -> Judged {
    let Some(&first) = prefix.first() else {
        return Judged::NeedMore(1);
    };
    if first == 0x16 {
        return match tls::reassemble(prefix, MAX_PREFIX) {
            tls::Reassembled::Hello(hello) => Judged::Tls(hello),
            tls::Reassembled::NeedMore(n) => Judged::NeedMore(n),
            tls::Reassembled::Other => Judged::Other,
        };
    }
    judge_http(prefix)
}

fn judge_http(prefix: &[u8]) -> Judged {
    for (i, &byte) in prefix.iter().enumerate() {
        if byte == b' ' {
            let method = &prefix[..i];
            if i == 0 || i > MAX_METHOD {
                return Judged::Other;
            }
            return if method == b"PRI" {
                // The plaintext HTTP/2 preface, forwarded as it is.
                Judged::Other
            } else {
                Judged::Http
            };
        }
        let fits = byte.is_ascii_uppercase() || byte == b'-' || byte == b'_';
        if !fits || i >= MAX_METHOD {
            return Judged::Other;
        }
    }
    Judged::NeedMore(prefix.len() + 1)
}

/// How a sniff ended (design 6.3).
#[derive(Debug, PartialEq, Eq)]
pub enum Sniffed {
    Tls(ClientHello),
    Http,
    Other,
    /// The wait ran out first.
    Timeout,
    /// The peer closed its side before the bytes could be judged.
    Closed,
    /// Reading failed: the peer reset the connection, say (P4 of the phase 4 /
    /// M1a plan).
    Failed,
}

/// Reads `stream` until its first bytes can be judged, the peer closes its
/// side, reading fails, or `wait` runs out. What was read stays in `stream`,
/// to be read back by whoever reads it next.
pub async fn sniff<S: AsyncRead + Unpin>(stream: &mut Prefixed<S>, wait: Duration) -> Sniffed {
    let deadline = Instant::now() + wait;
    // How many bytes in all the last judge asked for; nothing is judged again
    // until that many are there, so a client that drips one byte at a time
    // does not make the prefix be read over and over (P3 of the phase 4 / M1a
    // plan).
    let mut enough = 0;
    loop {
        if stream.buffered().len() >= enough {
            match judge(stream.buffered()) {
                Judged::NeedMore(n) => enough = n,
                Judged::Tls(hello) => return Sniffed::Tls(hello),
                Judged::Http => return Sniffed::Http,
                Judged::Other => return Sniffed::Other,
            }
        }
        // A fill that the deadline cancels has read nothing: the prefix keeps
        // every byte that came, and no byte comes twice.
        match timeout_at(deadline, stream.fill()).await {
            Err(_) => return Sniffed::Timeout,
            Ok(Ok(0)) => return Sniffed::Closed,
            Ok(Ok(_)) => {}
            Ok(Err(_)) => return Sniffed::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf, duplex};

    use crate::tls::tests::{handshake_bytes, hello_records, into_records};

    use super::*;

    #[test]
    fn more_than_64_kib_is_other() {
        // A handshake header that says its message is 65536 bytes long is
        // over the most that is read.
        let head = [0x16, 0x03, 0x01, 0x00, 0x04, 1, 0x01, 0x00, 0x00];
        assert_eq!(judge(&head), Judged::Other);

        // A handshake header of a message that is exactly the most that is
        // read, carried one byte at a time: the prefix ends before the
        // message does, and the next byte to wait for is past that most.
        let mut drip = Vec::new();
        let header = [1u8, 0x00, 0xff, 0xfc];
        for i in 0..(MAX_PREFIX - 4) / 6 {
            let byte = if i < header.len() { header[i] } else { 0 };
            drip.extend_from_slice(&[0x16, 0x03, 0x01, 0x00, 0x01, byte]);
        }
        drip.extend_from_slice(&[0x16, 0x03, 0x01, 0x00]);
        assert_eq!(drip.len(), MAX_PREFIX);
        assert_eq!(judge(&drip), Judged::Other);
    }

    #[test]
    fn another_record_type_midway_is_other() {
        let records = hello_records("sni.test", &[b"h2"]);
        let message = handshake_bytes(&records);
        let half = message.len() / 2;
        let mut split = into_records(&message, &[half]);
        split[5 + half] = 0x17;
        assert_eq!(judge(&split), Judged::Other);
    }

    #[test]
    fn a_slow_drip_is_not_judged_again_until_it_can_be() {
        let records = hello_records("sni.test", &[b"h2"]);
        let message = handshake_bytes(&records);
        assert_eq!(records.len(), 5 + message.len(), "the hello is one record");

        assert_eq!(judge(&records[..3]), Judged::NeedMore(5));
        assert_eq!(judge(&records[..6]), Judged::NeedMore(9));
        assert_eq!(judge(&records[..9]), Judged::NeedMore(records.len()));
    }

    #[test]
    fn method_tokens_are_http() {
        let twenty = b"ABCDEFGHIJKLMNOPQRST ";
        for request in [
            &b"GET "[..],
            &b"POST "[..],
            &b"M-SEARCH "[..],
            &b"PROPFIND "[..],
            &b"X_CUSTOM "[..],
            twenty,
        ] {
            assert_eq!(judge(request), Judged::Http, "{request:?}");
        }
    }

    #[test]
    fn pri_is_other() {
        assert_eq!(judge(b"PRI * HTTP/2.0\r\n"), Judged::Other);
    }

    #[test]
    fn what_is_not_a_method_is_other() {
        for request in [
            &b"get "[..],
            &b"ABCDEFGHIJKLMNOPQRSTU "[..],
            &b" GET "[..],
            &b"GET\t"[..],
        ] {
            assert_eq!(judge(request), Judged::Other, "{request:?}");
        }
    }

    #[test]
    fn half_a_method_needs_more() {
        assert_eq!(judge(b""), Judged::NeedMore(1));
        assert_eq!(judge(b"G"), Judged::NeedMore(2));
        assert_eq!(judge(b"POS"), Judged::NeedMore(4));
    }

    #[tokio::test]
    async fn sniff_judges_once_enough_has_come() {
        let records = hello_records("sni.test", &[b"h2", b"http/1.1"]);
        let (client, mut peer) = duplex(1 << 16);
        let mut prefix = Prefixed::new(client);
        let third = records.len() / 3;
        let writes = async {
            for chunk in [
                &records[..third],
                &records[third..2 * third],
                &records[2 * third..],
            ] {
                peer.write_all(chunk).await.expect("write");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        let (sniffed, ()) = tokio::join!(sniff(&mut prefix, Duration::from_secs(5)), writes);
        match sniffed {
            Sniffed::Tls(hello) => assert_eq!(hello.sni.as_deref(), Some("sni.test")),
            other => panic!("expected TLS, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn sniff_returns_at_once_on_other() {
        let (client, mut peer) = duplex(64);
        peer.write_all(b"hello").await.expect("write");
        let mut prefix = Prefixed::new(client);
        let started = std::time::Instant::now();
        assert_eq!(
            sniff(&mut prefix, Duration::from_secs(5)).await,
            Sniffed::Other
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn sniff_gives_up_at_the_wait() {
        let (client, _peer) = duplex(64);
        let mut prefix = Prefixed::new(client);
        let started = std::time::Instant::now();
        assert_eq!(
            sniff(&mut prefix, Duration::from_millis(200)).await,
            Sniffed::Timeout
        );
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(200), "gave up after {took:?}");
        assert!(took < Duration::from_secs(5), "gave up after {took:?}");
        assert!(prefix.buffered().is_empty());
    }

    #[tokio::test]
    async fn sniff_sees_the_peer_close() {
        let (client, mut peer) = duplex(64);
        peer.write_all(b"GE").await.expect("write");
        peer.shutdown().await.expect("shutdown");
        let mut prefix = Prefixed::new(client);
        assert_eq!(
            sniff(&mut prefix, Duration::from_secs(5)).await,
            Sniffed::Closed
        );
        assert_eq!(prefix.buffered(), b"GE");
    }

    struct Resetting;

    impl AsyncRead for Resetting {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "the peer reset the connection",
            )))
        }
    }

    #[tokio::test]
    async fn sniff_sees_a_read_error() {
        let mut prefix = Prefixed::new(Resetting);
        assert_eq!(
            sniff(&mut prefix, Duration::from_secs(5)).await,
            Sniffed::Failed
        );
    }

    #[tokio::test]
    async fn the_prefix_survives_the_sniff() {
        let records = hello_records("sni.test", &[b"h2"]);
        let (client, mut peer) = duplex(1 << 16);
        peer.write_all(&records).await.expect("write");
        peer.write_all(b"later").await.expect("write");
        peer.shutdown().await.expect("shutdown");
        let mut prefix = Prefixed::new(client);
        assert!(matches!(
            sniff(&mut prefix, Duration::from_secs(5)).await,
            Sniffed::Tls(_)
        ));
        let mut all = Vec::new();
        prefix.read_to_end(&mut all).await.expect("read");
        let mut want = records.clone();
        want.extend_from_slice(b"later");
        assert_eq!(all, want);
    }
}
