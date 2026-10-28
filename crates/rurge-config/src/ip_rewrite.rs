//! `[IP Rewrite]` (manual: Features › IP Rewrite): the lines as written,
//! `<address or CIDR> = <action>`, and the action a destination address gets.
//!
//! A line is read the way an `IP-CIDR` rule is (`rule::parse_kind`): first as
//! a network with its prefix, then as a single address, which is a `/32` or a
//! `/128`. A network written with host bits keeps them; matching looks at the
//! range from its network to its broadcast address, so the host bits do not
//! matter. A line whose address, prefix length or action cannot be read, or
//! that has anything else around the two sides, is a `W0012` and is left out;
//! every other line is kept, in the order written.
//!
//! The table is kept by family: an address can match only lines of its own
//! family, so "the first line from the top" holds within one family.

use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::text::Entry;
use crate::value::split_definition;
use ipnet::{Ipv4Net, Ipv6Net};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// What `[IP Rewrite]` does with a packet to a matching address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IpRewriteAction {
    Reflect,
    Reject,
    Drop,
}

/// `[IP Rewrite]`: the lines as written, kept by family.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct IpRewrite {
    v4: Vec<(Ipv4Net, IpRewriteAction)>,
    v6: Vec<(Ipv6Net, IpRewriteAction)>,
}

impl IpRewrite {
    /// The action of the first line of `dst`'s family whose prefix holds it.
    pub fn action(&self, dst: IpAddr) -> Option<IpRewriteAction> {
        match dst {
            IpAddr::V4(ip) => self
                .v4
                .iter()
                .find(|(net, _)| net.contains(&ip))
                .map(|(_, action)| *action),
            IpAddr::V6(ip) => self
                .v6
                .iter()
                .find(|(net, _)| net.contains(&ip))
                .map(|(_, action)| *action),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.v4.is_empty() && self.v6.is_empty()
    }
}

/// What one line names: an IPv4 or an IPv6 prefix.
enum Prefix {
    V4(Ipv4Net),
    V6(Ipv6Net),
}

const INVALID: &str =
    "[IP Rewrite]: a line that is not `<address or CIDR> = reflect | reject | drop` is ignored";

/// Reads the section's active lines in order: `<address or CIDR> = <action>`.
pub fn parse<'a>(
    entries: impl IntoIterator<Item = &'a Entry>,
    diags: &mut Diagnostics,
) -> IpRewrite {
    let mut table = IpRewrite::default();
    for entry in entries {
        let line = match split_definition(&entry.raw) {
            Some((prefix, action)) => prefix_of(prefix).zip(action_of(action)),
            None => None,
        };
        match line {
            Some((Prefix::V4(net), action)) => table.v4.push((net, action)),
            Some((Prefix::V6(net), action)) => table.v6.push((net, action)),
            None => diags
                .push(Diagnostic::warning(codes::W_INVALID_VALUE, INVALID).at(entry.span.clone())),
        }
    }
    table
}

fn prefix_of(text: &str) -> Option<Prefix> {
    if let Ok(net) = text.parse::<Ipv4Net>() {
        return Some(Prefix::V4(net));
    }
    if let Ok(ip) = text.parse::<Ipv4Addr>() {
        return Some(Prefix::V4(Ipv4Net::new(ip, 32).ok()?));
    }
    if let Ok(net) = text.parse::<Ipv6Net>() {
        return Some(Prefix::V6(net));
    }
    if let Ok(ip) = text.parse::<Ipv6Addr>() {
        return Some(Prefix::V6(Ipv6Net::new(ip, 128).ok()?));
    }
    None
}

fn action_of(text: &str) -> Option<IpRewriteAction> {
    match text.to_ascii_lowercase().as_str() {
        "reflect" => Some(IpRewriteAction::Reflect),
        "reject" => Some(IpRewriteAction::Reject),
        "drop" => Some(IpRewriteAction::Drop),
        _ => None,
    }
}
