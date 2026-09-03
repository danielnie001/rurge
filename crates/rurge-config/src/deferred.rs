//! Sections that are parsed and kept but have no behaviour in this version.

use crate::text::{Entry, Profile};

const DEFERRED: &[&str] = &[
    "MITM",
    "URL Rewrite",
    "Header Rewrite",
    "Body Rewrite",
    "Map Local",
    "Script",
    "Panel",
    "SSID Setting",
    "Port Forwarding",
    "Ponte",
    "Testing",
    "DHCP",
    "Snell Server",
    "MTProto",
];

pub fn is_deferred(name: &str) -> bool {
    DEFERRED.iter().any(|d| d.eq_ignore_ascii_case(name))
        || (name.len() > 10 && name[..10].eq_ignore_ascii_case("WireGuard "))
        || (name.len() > 10 && name[..10].eq_ignore_ascii_case("Tailscale "))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeferredSection {
    pub name: String,
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeferredSections {
    pub sections: Vec<DeferredSection>,
}

impl DeferredSections {
    pub fn get(&self, name: &str) -> Option<&DeferredSection> {
        self.sections
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    }
    pub fn collect(profile: &Profile) -> DeferredSections {
        DeferredSections {
            sections: profile
                .sections
                .iter()
                .filter(|s| is_deferred(&s.name))
                .map(|s| DeferredSection {
                    name: s.name.clone(),
                    entries: s.active_entries().cloned().collect(),
                })
                .collect(),
        }
    }
}
