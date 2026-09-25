//! One connectivity test of a policy (phase 2 M3 design 6.1, M3-D8): through
//! the policy's own outbound — never the pooled `HttpClient`, whose pool
//! would decide on its own which request opens a connection — two `HEAD`s on
//! one connection, the second one timed.

use bytes::Bytes;
use http::Request;
use http::header::{CONNECTION, HOST, USER_AGENT};
use http_body_util::Empty;
use hyper_util::rt::TokioIo;
use rurge_config::HostName;
use rurge_net::connector::{BoxedStream, ConnectOpts, Target};
use rurge_proto::OutboundRef;
use rustls::RootCertStore;
use rustls::pki_types::ServerName;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;
use url::{Host, Url};

/// What one test found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probed {
    /// The response head came back. `score` is the second `HEAD`'s time on
    /// the kept-alive connection (`reused`), or, when the connection could
    /// not be kept, the whole first round trip from the start of the dial.
    Passed { score: Duration, reused: bool },
    /// Why not; nothing of the test URL in it (a subscription line may have
    /// set the URL).
    Failed(String),
}

/// Stops the connection task however the test ends.
struct Driver(JoinHandle<()>);

impl Drop for Driver {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Tests `outbound` against `url` (`http` or `https`), within `timeout`
/// overall. `roots` verify an `https` URL's certificate. Any status counts:
/// the response head coming back is the test.
pub async fn probe(
    outbound: &OutboundRef,
    url: &Url,
    timeout: Duration,
    roots: Arc<RootCertStore>,
) -> Probed {
    match tokio::time::timeout(timeout, run(outbound, url, timeout, roots)).await {
        Ok(Ok(passed)) => passed,
        Ok(Err(why)) => Probed::Failed(why),
        Err(_) => Probed::Failed("timed out".to_string()),
    }
}

async fn run(
    outbound: &OutboundRef,
    url: &Url,
    timeout: Duration,
    roots: Arc<RootCertStore>,
) -> Result<Probed, String> {
    let tls = match url.scheme() {
        "http" => false,
        "https" => true,
        _ => return Err("the test URL is neither http nor https".to_string()),
    };
    let (host, server_name) = match url.host() {
        Some(Host::Domain(d)) => (HostName::parse(d), d.to_string()),
        Some(Host::Ipv4(ip)) => (HostName::Ip(ip.into()), ip.to_string()),
        Some(Host::Ipv6(ip)) => (HostName::Ip(ip.into()), ip.to_string()),
        None => return Err("the test URL has no host".to_string()),
    };
    let port = url
        .port_or_known_default()
        .ok_or("the test URL has no port")?;
    let started = Instant::now();
    let stream = outbound
        .connect_tcp(&Target::new(host, port), &ConnectOpts { timeout })
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let stream: BoxedStream = if tls {
        let name = ServerName::try_from(server_name)
            .map_err(|_| "the test URL's host is no TLS server name".to_string())?;
        let config = client_config(roots)?;
        Box::new(
            TlsConnector::from(config)
                .connect(name, stream)
                .await
                .map_err(|e| format!("tls: {e}"))?,
        )
    } else {
        stream
    };
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|e| format!("http: {e}"))?;
    let _driver = Driver(tokio::spawn(async move {
        let _ = connection.await;
    }));
    let authority = match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_string(),
    };
    let head = || {
        Request::head(url[url::Position::BeforePath..url::Position::AfterQuery].to_string())
            .header(HOST, &authority)
            .header(USER_AGENT, concat!("rurge/", env!("CARGO_PKG_VERSION")))
            .body(Empty::<Bytes>::new())
            .expect("a HEAD request")
    };
    let first = sender
        .send_request(head())
        .await
        .map_err(|e| format!("http: {e}"))?;
    let whole = started.elapsed();
    let closing = first.headers().get_all(CONNECTION).iter().any(|v| {
        v.to_str()
            .is_ok_and(|v| v.to_ascii_lowercase().contains("close"))
    });
    drop(first);
    if closing || sender.ready().await.is_err() {
        return Ok(Probed::Passed {
            score: whole,
            reused: false,
        });
    }
    let second = Instant::now();
    match sender.send_request(head()).await {
        Ok(_) => Ok(Probed::Passed {
            score: second.elapsed(),
            reused: true,
        }),
        // the server let the connection go after all
        Err(e) if e.is_closed() || e.is_incomplete_message() || e.is_canceled() => {
            Ok(Probed::Passed {
                score: whole,
                reused: false,
            })
        }
        Err(e) => Err(format!("http: {e}")),
    }
}

fn client_config(roots: Arc<RootCertStore>) -> Result<Arc<rustls::ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("tls: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    // the connection is spoken over HTTP/1, with no room for h2
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_net::testing::TestServer;
    use rurge_proto::testing::TlsFixture;
    use rurge_proto::{Direct, Reject, RejectKind};

    fn direct() -> OutboundRef {
        Arc::new(Direct::new(Arc::new(DirectConnector::new(Arc::new(
            SystemResolve,
        )))))
    }

    fn heads(server: &TestServer, path: &str) -> usize {
        server
            .requests()
            .iter()
            .filter(|r| r.path == path && r.method == "HEAD")
            .count()
    }

    #[tokio::test]
    async fn the_second_head_on_a_kept_connection_is_timed() {
        let server = TestServer::spawn().await;
        server.set("/ok", "hi");
        let probed = probe(
            &direct(),
            &server.url("/ok"),
            Duration::from_secs(5),
            rurge_net::tls::root_store(),
        )
        .await;
        assert!(
            matches!(probed, Probed::Passed { reused: true, .. }),
            "{probed:?}"
        );
        assert_eq!(heads(&server, "/ok"), 2);
    }

    /// A server that closes after the first answer is measured by that
    /// answer, dial included.
    #[tokio::test]
    async fn a_connection_that_is_not_kept_gives_the_first_round_trip() {
        let server = TestServer::spawn().await;
        server.set("/once", "");
        server.set_header("/once", "connection", "close");
        let probed = probe(
            &direct(),
            &server.url("/once"),
            Duration::from_secs(5),
            rurge_net::tls::root_store(),
        )
        .await;
        assert!(
            matches!(probed, Probed::Passed { reused: false, .. }),
            "{probed:?}"
        );
        assert_eq!(heads(&server, "/once"), 1);
    }

    /// Any status is an answer: the test is about the way there.
    #[tokio::test]
    async fn any_status_passes() {
        let server = TestServer::spawn().await;
        server.set("/gone", "");
        server.set_status("/gone", 404);
        let probed = probe(
            &direct(),
            &server.url("/gone"),
            Duration::from_secs(5),
            rurge_net::tls::root_store(),
        )
        .await;
        assert!(matches!(probed, Probed::Passed { .. }), "{probed:?}");
    }

    /// An `https` URL is tested inside the TLS connection it opened, the
    /// second request on the same session.
    #[tokio::test]
    async fn https_is_tested_on_one_tls_connection() {
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let server = TestServer::spawn_tls_with(fixture.acceptor(false)).await;
        server.set("/ok", "");
        let probed = probe(
            &direct(),
            &server.url("/ok"),
            Duration::from_secs(5),
            fixture.roots(),
        )
        .await;
        assert!(
            matches!(probed, Probed::Passed { reused: true, .. }),
            "{probed:?}"
        );
        assert_eq!(heads(&server, "/ok"), 2);
        // a certificate nobody vouches for fails the test
        let untrusted = probe(
            &direct(),
            &server.url("/ok"),
            Duration::from_secs(5),
            Arc::new(RootCertStore::empty()),
        )
        .await;
        assert!(
            matches!(&untrusted, Probed::Failed(why) if why.starts_with("tls: ")),
            "{untrusted:?}"
        );
    }

    #[tokio::test]
    async fn a_failure_says_which_step_failed() {
        let server = TestServer::spawn().await;
        server.set("/slow", "");
        server.set_delay("/slow", Duration::from_secs(5));
        let slow = probe(
            &direct(),
            &server.url("/slow"),
            Duration::from_millis(200),
            rurge_net::tls::root_store(),
        )
        .await;
        assert_eq!(slow, Probed::Failed("timed out".to_string()));
        let reject: OutboundRef = Arc::new(Reject::new(RejectKind::Reject));
        let rejected = probe(
            &reject,
            &server.url("/slow"),
            Duration::from_secs(5),
            rurge_net::tls::root_store(),
        )
        .await;
        assert!(
            matches!(&rejected, Probed::Failed(why) if why.starts_with("connect: ")),
            "{rejected:?}"
        );
        let ftp = probe(
            &direct(),
            &Url::parse("ftp://127.0.0.1/").unwrap(),
            Duration::from_secs(5),
            rurge_net::tls::root_store(),
        )
        .await;
        assert_eq!(
            ftp,
            Probed::Failed("the test URL is neither http nor https".to_string())
        );
    }
}
