//! The session of a TCP connection through the TUN (phase 3 M3 design 5.6):
//! once the handshake with the app is over, the `Dialer` dials where the app
//! connected to, and relays between the two.

use crate::tcp::{TunStream, Tuple};
use rurge_config::{HostName, ListenerKind, SessionInfo};
use rurge_inbound::Dialer;
use std::sync::Arc;

pub(crate) async fn serve(stream: TunStream, dialer: Arc<dyn Dialer>, tuple: Tuple) {
    if stream.established().await.is_err() {
        return;
    }
    let session = SessionInfo {
        src: tuple.src,
        in_port: 0,
        listener: ListenerKind::Tun,
        ..SessionInfo::tcp(HostName::Ip(tuple.dst.ip()), tuple.dst.port())
    };
    // a rejection or a failure lets the stream go, which resets the connection
    if let Ok(dialed) = dialer.dial(session).await {
        dialer
            .relay(Box::new(stream), dialed.stream, dialed.handle)
            .await;
    }
}
