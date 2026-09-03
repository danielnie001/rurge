//! Sections that are parsed and kept but have no behaviour in this version.

use crate::text::{Entry, Profile};
use crate::value::starts_with_ci;

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
        || starts_with_ci(name, "WireGuard ")
        || starts_with_ci(name, "Tailscale ")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{Origin, parse_str};
    use std::path::Path;
    use std::sync::Arc;

    #[test]
    fn prefix_and_membership_matching() {
        assert!(is_deferred("MITM"));
        assert!(is_deferred("mitm"));
        assert!(is_deferred("WireGuard home"));
        assert!(is_deferred("WireGuard "));
        assert!(!is_deferred("WireGuard"));
        assert!(!is_deferred("Rule"));
        assert!(!is_deferred("General"));
        assert!(!is_deferred("Proxy"));
    }

    #[test]
    fn collect_keeps_only_active_entries_and_get_is_case_insensitive() {
        let (mut profile, d) = parse_str(
            "[MITM]\nhostname = *.example.com\nskip-server-cert-verify = true\n",
            Arc::from(Path::new("d.conf")),
            Origin::Main,
        );
        assert!(d.is_empty(), "{:?}", d.into_vec());
        profile.section_mut("MITM").unwrap().entries[1].disabled = true;
        let deferred = DeferredSections::collect(&profile);
        assert_eq!(deferred.sections.len(), 1);
        let mitm = deferred.get("mitm").unwrap();
        assert_eq!(mitm.entries.len(), 1);
        assert_eq!(mitm.entries[0].raw, "hostname = *.example.com");
    }

    #[test]
    fn non_ascii_names_do_not_panic() {
        assert!(!is_deferred("中文中文中文"));
        assert!(is_deferred("WireGuard 家里"));
    }
}
