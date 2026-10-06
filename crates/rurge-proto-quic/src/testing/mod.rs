//! Loopback QUIC servers for the tests of this crate and of its dependants
//! (feature `testing`): scriptable Hysteria 2, TUIC and HTTP/3 CONNECT
//! (MASQUE, TrustTunnel) servers, on ports that can restart (`ServerPort`,
//! `rurge_net`'s). Never used by production code.

mod h3proxy;
mod hysteria2;
mod obfs;
mod tuic;

pub use h3proxy::{
    FakeH3Proxy, H3ProxyScript, RecordedH3Datagram, RecordedH3Request, RecordedUdpEnd,
};
pub use hysteria2::{
    FakeHysteria2, Hysteria2Script, RecordedAuth, RecordedTcp, RecordedUdp, UdpMessage,
};
pub use obfs::RecordedFragment;
pub use rurge_net::quic::testing::ServerPort;
pub use tuic::{
    FakeTuic, RecordedConnect, RecordedDissociate, RecordedTuicAuth, RecordedTuicPacket,
    TuicPacket, TuicScript,
};
