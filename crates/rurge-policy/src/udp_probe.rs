//! The UDP test (phase 2 M5 design 8.4; the manual's `test-udp` /
//! `proxy-test-udp`): one DNS question for `hostname`'s A records, through
//! the policy's own UDP carrier, to the server's port 53. Any answer to it
//! passes — a name the server does not know too; the result is how long the
//! answer took. It is shown, never kept: the groups pick by the TCP tests
//! alone (the manual: the latency test measures TCP).

use rurge_config::HostName;
use rurge_config::general::UdpTest;
use rurge_net::connector::{ConnectOpts, Target};
use rurge_proto::{OutboundRef, UdpSupport};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Where a UDP test asks.
const DNS_PORT: u16 = 53;

/// A question's ID: different from the last one's.
fn next_id() -> u16 {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos()) as u16;
    seed ^ NEXT.fetch_add(0x9e37, Ordering::Relaxed)
}

/// The question for `hostname`'s A records, with `id`; `None` for a name no
/// question can hold.
pub(crate) fn question(id: u16, hostname: &str) -> Option<Vec<u8>> {
    let name = hostname.trim_end_matches('.');
    if name.is_empty() || name.len() > 253 || !name.is_ascii() {
        return None;
    }
    // ID, recursion desired, one question
    let mut out = id.to_be_bytes().to_vec();
    out.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
    for label in name.split('.') {
        let len = u8::try_from(label.len())
            .ok()
            .filter(|l| (1..=63).contains(l))?;
        out.push(len);
        out.extend_from_slice(label.as_bytes());
    }
    // the root, then A, IN
    out.extend_from_slice(&[0, 0, 1, 0, 1]);
    Some(out)
}

/// Whether `datagram` answers the question `id`.
pub(crate) fn answers(datagram: &[u8], id: u16) -> bool {
    datagram.len() >= 12 && datagram[..2] == id.to_be_bytes() && datagram[2] & 0x80 != 0
}

/// The UDP test of `outbound` at `test`, within `timeout`.
pub async fn probe_udp(
    outbound: &OutboundRef,
    test: &UdpTest,
    timeout: Duration,
) -> Result<Duration, String> {
    let server = SocketAddr::new(IpAddr::V4(test.server), DNS_PORT);
    probe_udp_at(outbound, &test.hostname, server, timeout).await
}

/// `probe_udp` to a server on any port (the tests' loopback servers).
pub(crate) async fn probe_udp_at(
    outbound: &OutboundRef,
    hostname: &str,
    server: SocketAddr,
    timeout: Duration,
) -> Result<Duration, String> {
    if outbound.udp() == UdpSupport::Unsupported {
        return Err("the policy carries no UDP".to_string());
    }
    let id = next_id();
    let question =
        question(id, hostname).ok_or_else(|| format!("`{hostname}` cannot be asked for"))?;
    let to = Target::new(HostName::Ip(server.ip()), server.port());
    let exchange = async {
        let carrier = outbound
            .open_udp(&ConnectOpts { timeout })
            .await
            .map_err(|e| e.to_string())?;
        let sent = Instant::now();
        carrier
            .send_to(&question, &to)
            .await
            .map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 65536];
        loop {
            let (n, from) = carrier
                .recv_from(&mut buf)
                .await
                .map_err(|e| e.to_string())?;
            if from == to && answers(&buf[..n], id) {
                return Ok(sent.elapsed());
            }
        }
    };
    match tokio::time::timeout(timeout, exchange).await {
        Ok(outcome) => outcome,
        Err(_) => Err("udp test timed out".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_proto::{Direct, Reject, RejectKind};
    use std::sync::Arc;
    use tokio::net::UdpSocket;

    fn direct() -> OutboundRef {
        Arc::new(Direct::new(Arc::new(DirectConnector::new(Arc::new(
            SystemResolve,
        )))))
    }

    /// A name server on the loopback: answers every question after a wrong
    /// ID first, or never (`silent`).
    async fn server(silent: bool) -> SocketAddr {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            while let Ok((n, from)) = socket.recv_from(&mut buf).await {
                if silent {
                    continue;
                }
                let mut answer = buf[..n].to_vec();
                answer[2] |= 0x80;
                let mut other = answer.clone();
                other[0] ^= 0xff;
                let _ = socket.send_to(&other, from).await;
                let _ = socket.send_to(&answer, from).await;
            }
        });
        addr
    }

    #[test]
    fn the_question_asks_for_a_records() {
        assert_eq!(
            question(0x1234, "a.bc").unwrap(),
            [
                0x12, 0x34, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 1, b'a', 2, b'b', b'c', 0, 0, 1, 0, 1
            ]
        );
        assert_eq!(question(1, "apple.com.").unwrap().len(), 12 + 11 + 4);
        for bad in ["", "a..b", "bücher.example", &"a".repeat(64)] {
            assert_eq!(question(1, bad), None, "{bad}");
        }
        assert!(answers(
            &[0x12, 0x34, 0x81, 0x80, 0, 1, 0, 0, 0, 0, 0, 0],
            0x1234
        ));
        assert!(
            !answers(&[0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0], 0x1234),
            "a question"
        );
        assert!(!answers(
            &[0x12, 0x35, 0x81, 0x80, 0, 1, 0, 0, 0, 0, 0, 0],
            0x1234
        ));
    }

    /// The answer to our question counts, whatever came before it.
    #[tokio::test]
    async fn an_answer_is_timed() {
        let at = server(false).await;
        let took = probe_udp_at(&direct(), "apple.com", at, Duration::from_secs(5))
            .await
            .expect("an answer");
        assert!(took < Duration::from_secs(5));
    }

    /// No answer within the time is a failure, saying so; a policy without
    /// UDP is not asked at all.
    #[tokio::test]
    async fn silence_and_no_udp_fail() {
        let at = server(true).await;
        let started = Instant::now();
        let err = probe_udp_at(&direct(), "apple.com", at, Duration::from_millis(300))
            .await
            .unwrap_err();
        assert_eq!(err, "udp test timed out");
        assert!(started.elapsed() < Duration::from_secs(5));
        let reject: OutboundRef = Arc::new(Reject::new(RejectKind::Reject));
        assert_eq!(
            probe_udp_at(&reject, "apple.com", at, Duration::from_secs(1))
                .await
                .unwrap_err(),
            "the policy carries no UDP"
        );
    }
}
