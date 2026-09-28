//! Cryptokey routing (manual: `allowed-ips`): the peer a destination goes
//! to is the one with the longest prefix covering it, IPv4 and IPv6 in
//! tables of their own; and a peer may only send from what routes to it.

use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use prefix_trie::PrefixMap;
use std::net::IpAddr;

#[derive(Clone, Debug, Default)]
pub struct Routes {
    v4: PrefixMap<Ipv4Net, usize>,
    v6: PrefixMap<Ipv6Net, usize>,
}

impl Routes {
    /// `allowed` is each peer's `allowed-ips`, in peer order. A prefix two
    /// peers both list goes to the later one, as with `wg`.
    pub fn new<'a>(allowed: impl IntoIterator<Item = &'a [IpNet]>) -> Routes {
        let mut routes = Routes::default();
        for (peer, nets) in allowed.into_iter().enumerate() {
            for net in nets {
                match net.trunc() {
                    IpNet::V4(net) => routes.v4.insert(net, peer),
                    IpNet::V6(net) => routes.v6.insert(net, peer),
                };
            }
        }
        routes
    }

    /// The peer `ip` is routed to.
    pub fn lookup(&self, ip: IpAddr) -> Option<usize> {
        match ip {
            IpAddr::V4(v4) => self.v4.get_lpm(&Ipv4Net::from(v4)).map(|(_, p)| *p),
            IpAddr::V6(v6) => self.v6.get_lpm(&Ipv6Net::from(v6)).map(|(_, p)| *p),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routes(peers: &[&[&str]]) -> Routes {
        let nets: Vec<Vec<IpNet>> = peers
            .iter()
            .map(|p| p.iter().map(|n| n.parse().unwrap()).collect())
            .collect();
        Routes::new(nets.iter().map(Vec::as_slice))
    }

    fn at(r: &Routes, ip: &str) -> Option<usize> {
        r.lookup(ip.parse().unwrap())
    }

    #[test]
    fn the_longest_prefix_picks_the_peer() {
        let r = routes(&[
            &["0.0.0.0/0", "::/0"],
            &["10.0.0.0/8"],
            &["10.2.0.0/16", "fd00::/64"],
        ]);
        assert_eq!(at(&r, "192.0.2.1"), Some(0));
        assert_eq!(at(&r, "10.1.0.1"), Some(1));
        assert_eq!(at(&r, "10.2.9.9"), Some(2));
        assert_eq!(at(&r, "2001:db8::1"), Some(0));
        assert_eq!(at(&r, "fd00::7"), Some(2));
    }

    #[test]
    fn the_families_have_tables_of_their_own() {
        let r = routes(&[&["10.0.0.0/8"]]);
        assert_eq!(at(&r, "10.0.0.1"), Some(0));
        assert_eq!(
            at(&r, "::ffff:10.0.0.1"),
            None,
            "an IPv6 address, whatever it maps"
        );
        assert_eq!(at(&r, "192.0.2.1"), None);
        let r = routes(&[&["::/0"]]);
        assert_eq!(at(&r, "192.0.2.1"), None);
    }

    #[test]
    fn a_prefix_two_peers_list_goes_to_the_later_one() {
        let r = routes(&[&["10.0.0.0/24"], &["10.0.0.9/24"]]);
        assert_eq!(at(&r, "10.0.0.1"), Some(1));
    }
}
