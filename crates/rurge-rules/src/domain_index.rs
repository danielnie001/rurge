//! Exact / suffix domain index over reversed labels (M2 design §6.1).
//!
//! Keys are stored as `com.example.www` for `www.example.com` in one sorted
//! array; a lookup binary-searches every label-boundary prefix of the reversed
//! host, so it costs O(labels × log n) and the index costs one `Box<str>` per
//! distinct name.

/// Slot value meaning "no entry".
pub const NO_ENTRY: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomainMatchKind {
    Exact,
    Suffix,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DomainHit {
    /// Entry number given at build time; the smallest one among all hits wins.
    pub entry: u32,
    pub kind: DomainMatchKind,
}

#[derive(Clone, Debug, Default)]
pub struct DomainIndex {
    keys: Vec<Box<str>>,
    exact: Vec<u32>,
    suffix: Vec<u32>,
}

#[derive(Debug, Default)]
pub struct DomainIndexBuilder {
    items: Vec<(String, u32, DomainMatchKind)>,
}

/// `www.Example.com.` → `com.example.www`; `None` for an empty name.
pub fn reverse_labels(domain: &str) -> Option<String> {
    let d = domain.trim().trim_matches('.').to_ascii_lowercase();
    if d.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(d.len());
    for (i, label) in d.rsplit('.').enumerate() {
        if i > 0 {
            out.push('.');
        }
        out.push_str(label);
    }
    Some(out)
}

impl DomainIndexBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_exact(&mut self, domain: &str, entry: u32) {
        if let Some(k) = reverse_labels(domain) {
            self.items.push((k, entry, DomainMatchKind::Exact));
        }
    }

    pub fn add_suffix(&mut self, domain: &str, entry: u32) {
        if let Some(k) = reverse_labels(domain) {
            self.items.push((k, entry, DomainMatchKind::Suffix));
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn build(mut self) -> DomainIndex {
        self.items.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut keys: Vec<Box<str>> = Vec::new();
        let mut exact: Vec<u32> = Vec::new();
        let mut suffix: Vec<u32> = Vec::new();
        for (key, entry, kind) in self.items {
            let same = keys
                .last()
                .is_some_and(|last| last.as_ref() == key.as_str());
            if !same {
                keys.push(key.into_boxed_str());
                exact.push(NO_ENTRY);
                suffix.push(NO_ENTRY);
            }
            let slot = match kind {
                DomainMatchKind::Exact => exact.last_mut(),
                DomainMatchKind::Suffix => suffix.last_mut(),
            }
            .expect("slot pushed above");
            if *slot == NO_ENTRY || entry < *slot {
                *slot = entry;
            }
        }
        DomainIndex {
            keys,
            exact,
            suffix,
        }
    }
}

impl DomainIndex {
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    fn slot(&self, key: &str) -> Option<usize> {
        self.keys.binary_search_by(|k| k.as_ref().cmp(key)).ok()
    }

    /// The hit with the smallest entry number, or `None`.
    pub fn lookup(&self, host: &str) -> Option<DomainHit> {
        let full = reverse_labels(host)?;
        let mut best: Option<DomainHit> = None;
        let mut consider = |entry: u32, kind: DomainMatchKind| {
            if entry != NO_ENTRY && best.is_none_or(|b| entry < b.entry) {
                best = Some(DomainHit { entry, kind });
            }
        };
        for (i, b) in full.bytes().enumerate() {
            if b == b'.'
                && let Some(s) = self.slot(&full[..i])
            {
                consider(self.suffix[s], DomainMatchKind::Suffix);
            }
        }
        if let Some(s) = self.slot(&full) {
            consider(self.suffix[s], DomainMatchKind::Suffix);
            consider(self.exact[s], DomainMatchKind::Exact);
        }
        best
    }

    pub fn matches(&self, host: &str) -> bool {
        self.lookup(host).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(exact: &[(&str, u32)], suffix: &[(&str, u32)]) -> DomainIndex {
        let mut b = DomainIndexBuilder::new();
        for (d, e) in exact {
            b.add_exact(d, *e);
        }
        for (d, e) in suffix {
            b.add_suffix(d, *e);
        }
        b.build()
    }

    #[test]
    fn reverse_labels_normalizes() {
        assert_eq!(
            reverse_labels("www.Example.com."),
            Some("com.example.www".into())
        );
        assert_eq!(reverse_labels("localhost"), Some("localhost".into()));
        assert_eq!(reverse_labels(""), None);
        assert_eq!(reverse_labels("."), None);
    }

    #[test]
    fn exact_matches_only_the_name() {
        let idx = build(&[("Example.com", 0)], &[]);
        assert_eq!(
            idx.lookup("example.com"),
            Some(DomainHit {
                entry: 0,
                kind: DomainMatchKind::Exact
            })
        );
        assert_eq!(idx.lookup("www.example.com"), None);
        assert_eq!(idx.lookup("com"), None);
    }

    #[test]
    fn suffix_matches_name_and_subdomains_on_label_boundaries() {
        let idx = build(&[], &[("example.com", 1)]);
        assert!(idx.matches("example.com"));
        assert!(idx.matches("a.b.example.com"));
        assert!(idx.matches("EXAMPLE.COM."));
        assert!(!idx.matches("notexample.com"));
        assert!(!idx.matches("example.com.evil"));
        assert!(!idx.matches("com"));
    }

    #[test]
    fn tld_suffix_matches_everything_under_it() {
        let idx = build(&[], &[("com", 2)]);
        assert!(idx.matches("com"));
        assert!(idx.matches("x.com"));
        assert!(!idx.matches("x.org"));
    }

    #[test]
    fn smallest_entry_wins_across_kinds_and_duplicates() {
        let idx = build(&[("a.com", 7), ("a.com", 3)], &[("com", 5)]);
        assert_eq!(idx.lookup("a.com").map(|h| h.entry), Some(3));
        assert_eq!(idx.lookup("b.com").map(|h| h.entry), Some(5));
        assert_eq!(idx.len(), 2);
    }

    #[test]
    fn empty_index_matches_nothing() {
        let idx = DomainIndexBuilder::new().build();
        assert!(idx.is_empty());
        assert!(!idx.matches("example.com"));
        assert_eq!(idx.lookup(""), None);
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    fn norm(d: &str) -> String {
        d.trim().trim_matches('.').to_ascii_lowercase()
    }

    fn naive(entries: &[(String, bool)], host: &str) -> Option<u32> {
        let h = norm(host);
        let mut best: Option<u32> = None;
        for (i, (d, is_suffix)) in entries.iter().enumerate() {
            let d = norm(d);
            if d.is_empty() || h.is_empty() {
                continue;
            }
            let hit = if *is_suffix {
                h == d || h.ends_with(&format!(".{d}"))
            } else {
                h == d
            };
            if hit && best.is_none_or(|b| (i as u32) < b) {
                best = Some(i as u32);
            }
        }
        best
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(500))]
        #[test]
        fn index_agrees_with_naive(
            entries in prop::collection::vec(("[a-c]{1,3}(\\.[a-c]{1,3}){0,3}", any::<bool>()), 0..40),
            host in "[a-c]{1,3}(\\.[a-c]{1,3}){0,4}",
        ) {
            let mut b = DomainIndexBuilder::new();
            for (i, (d, s)) in entries.iter().enumerate() {
                if *s { b.add_suffix(d, i as u32) } else { b.add_exact(d, i as u32) }
            }
            let idx = b.build();
            prop_assert_eq!(idx.lookup(&host).map(|h| h.entry), naive(&entries, &host));
        }
    }
}
