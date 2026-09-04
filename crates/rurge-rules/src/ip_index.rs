//! Longest-prefix IP index over two prefix tries (M2 design §6.1).

use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use prefix_trie::PrefixMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[derive(Clone, Debug, Default)]
pub struct IpIndex {
    v4: PrefixMap<Ipv4Net, u32>,
    v6: PrefixMap<Ipv6Net, u32>,
    len: usize,
}

#[derive(Debug, Default)]
pub struct IpIndexBuilder {
    index: IpIndex,
}

impl IpIndexBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_v4(&mut self, net: Ipv4Net, entry: u32) {
        let net = net.trunc();
        match self.index.v4.get(&net) {
            Some(existing) if *existing <= entry => {}
            _ => {
                if self.index.v4.insert(net, entry).is_none() {
                    self.index.len += 1;
                }
            }
        }
    }

    pub fn add_v6(&mut self, net: Ipv6Net, entry: u32) {
        let net = net.trunc();
        match self.index.v6.get(&net) {
            Some(existing) if *existing <= entry => {}
            _ => {
                if self.index.v6.insert(net, entry).is_none() {
                    self.index.len += 1;
                }
            }
        }
    }

    pub fn add(&mut self, net: IpNet, entry: u32) {
        match net {
            IpNet::V4(n) => self.add_v4(n, entry),
            IpNet::V6(n) => self.add_v6(n, entry),
        }
    }

    pub fn build(self) -> IpIndex {
        self.index
    }
}

impl IpIndex {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Entry of the longest prefix containing `ip`.
    pub fn lookup(&self, ip: IpAddr) -> Option<u32> {
        match ip {
            IpAddr::V4(v4) => self.lookup_v4(v4),
            IpAddr::V6(v6) => self.lookup_v6(v6),
        }
    }

    pub fn lookup_v4(&self, ip: Ipv4Addr) -> Option<u32> {
        let host = Ipv4Net::new(ip, 32).expect("/32 is a valid prefix length");
        self.v4.get_lpm(&host).map(|(_, e)| *e)
    }

    pub fn lookup_v6(&self, ip: Ipv6Addr) -> Option<u32> {
        let host = Ipv6Net::new(ip, 128).expect("/128 is a valid prefix length");
        self.v6.get_lpm(&host).map(|(_, e)| *e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(nets: &[(&str, u32)]) -> IpIndex {
        let mut b = IpIndexBuilder::new();
        for (n, e) in nets {
            b.add(n.parse().unwrap(), *e);
        }
        b.build()
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn longest_prefix_wins() {
        let idx = build(&[("10.0.0.0/8", 0), ("10.1.0.0/16", 1), ("10.1.2.0/24", 2)]);
        assert_eq!(idx.lookup(ip("10.1.2.3")), Some(2));
        assert_eq!(idx.lookup(ip("10.1.9.9")), Some(1));
        assert_eq!(idx.lookup(ip("10.9.9.9")), Some(0));
        assert_eq!(idx.lookup(ip("11.0.0.1")), None);
        assert_eq!(idx.len(), 3);
    }

    #[test]
    fn v6_and_v4_are_separate_tries() {
        let idx = build(&[("fd00::/8", 7), ("::1/128", 8), ("0.0.0.0/0", 9)]);
        assert_eq!(idx.lookup(ip("fd12::1")), Some(7));
        assert_eq!(idx.lookup(ip("::1")), Some(8));
        assert_eq!(idx.lookup(ip("2001:db8::1")), None);
        assert_eq!(idx.lookup(ip("203.0.113.1")), Some(9));
        assert_eq!(idx.lookup(ip("::ffff:203.0.113.1")), None);
    }

    #[test]
    fn host_bits_are_ignored_and_duplicates_keep_the_smallest_entry() {
        let idx = build(&[
            ("192.168.1.77/24", 5),
            ("192.168.1.0/24", 3),
            ("192.168.1.0/24", 9),
        ]);
        assert_eq!(idx.len(), 1);
        assert_eq!(idx.lookup(ip("192.168.1.200")), Some(3));
    }

    #[test]
    fn empty_index() {
        let idx = IpIndexBuilder::new().build();
        assert!(idx.is_empty());
        assert_eq!(idx.lookup(ip("1.1.1.1")), None);
        assert_eq!(idx.lookup(ip("::1")), None);
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    fn naive(nets: &[(Ipv4Net, u32)], ip: Ipv4Addr) -> Option<u32> {
        nets.iter()
            .filter(|(n, _)| n.contains(&ip))
            .max_by(|(a, ea), (b, eb)| {
                a.prefix_len().cmp(&b.prefix_len()).then(eb.cmp(ea)) // among equal prefixes the smallest entry wins
            })
            .map(|(_, e)| *e)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(500))]
        #[test]
        fn lookup_agrees_with_naive(
            nets in prop::collection::vec((any::<u32>(), 0u8..=32), 0..50),
            probe in any::<u32>(),
        ) {
            let nets: Vec<(Ipv4Net, u32)> = nets
                .iter()
                .enumerate()
                .map(|(i, (addr, len))| (Ipv4Net::new(Ipv4Addr::from(*addr), *len).unwrap().trunc(), i as u32))
                .collect();
            let mut b = IpIndexBuilder::new();
            for (n, e) in &nets {
                b.add_v4(*n, *e);
            }
            let idx = b.build();
            let ip = Ipv4Addr::from(probe);
            prop_assert_eq!(idx.lookup_v4(ip), naive(&nets, ip));
        }
    }
}
