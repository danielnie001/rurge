//! Tests of the fixtures everything else is tested with: the memory device,
//! the app-side stack, the fake `Dialer` and the loopback servers.

mod app_stack;
mod device;
mod dialer;
mod dialer_udp;
mod hooks;
mod test_packets;

use crate::device::AppEnd;
use crate::testing::{AppStack, AppTcp};
use crate::tests::support::within;
use smoltcp::wire::IpCidr;
use std::net::{IpAddr, SocketAddr};

fn cidr(ip: &str, prefix: u8) -> IpCidr {
    IpCidr::new(ip.parse::<IpAddr>().unwrap().into(), prefix)
}

/// Two stacks on the two ends of a crossed pair: the first with the
/// adapter's own addresses (what the operating system has), the second with
/// 192.0.2.1 and 2001:db8::1 (somewhere to connect to).
fn stacks() -> (AppStack, AppStack) {
    let (a, b) = AppEnd::crossed(1500, 4096);
    let documentation = [cidr("192.0.2.1", 32), cidr("2001:db8::1", 128)];
    (AppStack::start(a), AppStack::start_with(b, &documentation))
}

/// A connection from `client` to `server`'s address `to`: both ends of it.
async fn connected(client: &AppStack, server: &AppStack, to: SocketAddr) -> (AppTcp, AppTcp) {
    let accepting = server.accept(to);
    let client = within("the connection to be made", client.connect(to))
        .await
        .expect("connect");
    let server = within("the connection to be accepted", accepting)
        .await
        .expect("accept");
    (client, server)
}
