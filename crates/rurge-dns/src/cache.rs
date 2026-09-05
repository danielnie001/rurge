//! DNS cache (design §7.4): LRU keyed by lowercase name, TTL = the answer's
//! smallest record TTL, optimistic refresh (an expired entry is still served
//! while one background refresh runs), negative caching for empty answers.

use lru::LruCache;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::Duration;
use tokio::time::Instant;

pub const DEFAULT_CAPACITY: usize = 2000;
pub const NEGATIVE_TTL: Duration = Duration::from_secs(30);
pub const REFRESH_RETRY_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedAddrs {
    pub v4: Vec<Ipv4Addr>,
    pub v6: Vec<Ipv6Addr>,
    /// TTL the answer carried (smallest record TTL).
    pub ttl: Duration,
    /// The AAAA family was asked when this entry was recorded. A `false` entry
    /// holds no v6 answer *because none was requested*, so it must not be
    /// served to a caller that wants AAAA.
    pub v6_queried: bool,
    /// Upstream name that answered, for `cache_snapshot`.
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheHit {
    Fresh(CachedAddrs),
    /// Expired: serve it and refresh in the background (`begin_refresh`).
    Stale(CachedAddrs),
    /// A fresh negative entry: answer "empty" without asking upstream.
    Negative,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheEntry {
    pub name: String,
    pub v4: Vec<Ipv4Addr>,
    pub v6: Vec<Ipv6Addr>,
    pub expires_in: Option<Duration>,
    pub stale: bool,
    pub negative: bool,
    pub source: String,
}

#[derive(Clone, Debug)]
struct Entry {
    /// `None` = negative entry.
    addrs: Option<CachedAddrs>,
    expires_at: Instant,
    refreshing: bool,
    last_refresh: Option<Instant>,
}

pub struct DnsCache {
    inner: Mutex<LruCache<String, Entry>>,
}

impl DnsCache {
    pub fn new(capacity: usize) -> DnsCache {
        let cap = NonZeroUsize::new(capacity.max(1)).expect("capacity >= 1");
        DnsCache {
            inner: Mutex::new(LruCache::new(cap)),
        }
    }

    pub fn get(&self, name: &str) -> Option<CacheHit> {
        let now = Instant::now();
        let mut cache = self.inner.lock().expect("dns cache lock");
        let decision = match cache.get(name) {
            None => return None,
            Some(e) => match &e.addrs {
                None if now < e.expires_at => Some(CacheHit::Negative),
                None => None,
                Some(a) if now < e.expires_at => Some(CacheHit::Fresh(a.clone())),
                Some(a) => Some(CacheHit::Stale(a.clone())),
            },
        };
        if decision.is_none() {
            // An expired negative entry is a plain miss.
            cache.pop(name);
        }
        decision
    }

    pub fn put(&self, name: &str, addrs: CachedAddrs) {
        let mut cache = self.inner.lock().expect("dns cache lock");
        if addrs.ttl.is_zero() {
            cache.pop(name);
            return;
        }
        let expires_at = Instant::now() + addrs.ttl;
        cache.put(
            name.to_string(),
            Entry {
                addrs: Some(addrs),
                expires_at,
                refreshing: false,
                last_refresh: None,
            },
        );
    }

    pub fn put_negative(&self, name: &str) {
        let mut cache = self.inner.lock().expect("dns cache lock");
        cache.put(
            name.to_string(),
            Entry {
                addrs: None,
                expires_at: Instant::now() + NEGATIVE_TTL,
                refreshing: false,
                last_refresh: None,
            },
        );
    }

    /// Claims the single background refresh slot of a stale entry.
    pub fn begin_refresh(&self, name: &str) -> bool {
        let now = Instant::now();
        let mut cache = self.inner.lock().expect("dns cache lock");
        let Some(e) = cache.get_mut(name) else {
            return false;
        };
        if e.refreshing {
            return false;
        }
        if let Some(last) = e.last_refresh
            && now.duration_since(last) < REFRESH_RETRY_INTERVAL
        {
            return false;
        }
        e.refreshing = true;
        e.last_refresh = Some(now);
        true
    }

    /// Releases the refresh slot without replacing the entry (a successful
    /// refresh calls `put`, which replaces it).
    pub fn end_refresh(&self, name: &str) {
        let mut cache = self.inner.lock().expect("dns cache lock");
        if let Some(e) = cache.get_mut(name) {
            e.refreshing = false;
        }
    }

    pub fn flush(&self) {
        self.inner.lock().expect("dns cache lock").clear();
    }

    pub fn len(&self) -> usize {
        self.inner.lock().expect("dns cache lock").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn snapshot(&self) -> Vec<CacheEntry> {
        let now = Instant::now();
        let cache = self.inner.lock().expect("dns cache lock");
        let mut out: Vec<CacheEntry> = cache
            .iter()
            .map(|(name, e)| {
                let expires_in = if now < e.expires_at {
                    Some(e.expires_at - now)
                } else {
                    None
                };
                match &e.addrs {
                    Some(a) => CacheEntry {
                        name: name.clone(),
                        v4: a.v4.clone(),
                        v6: a.v6.clone(),
                        expires_in,
                        stale: expires_in.is_none(),
                        negative: false,
                        source: a.source.clone(),
                    },
                    None => CacheEntry {
                        name: name.clone(),
                        v4: Vec::new(),
                        v6: Vec::new(),
                        expires_in,
                        stale: expires_in.is_none(),
                        negative: true,
                        source: "negative".to_string(),
                    },
                }
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::advance;

    fn addrs(ttl_secs: u64) -> CachedAddrs {
        CachedAddrs {
            v4: vec!["10.0.0.1".parse().unwrap()],
            v6: vec![],
            ttl: Duration::from_secs(ttl_secs),
            v6_queried: false,
            source: "udp://1.1.1.1:53".to_string(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn fresh_then_stale() {
        let c = DnsCache::new(10);
        c.put("a.com", addrs(10));
        assert_eq!(c.get("a.com"), Some(CacheHit::Fresh(addrs(10))));
        advance(Duration::from_secs(11)).await;
        assert_eq!(c.get("a.com"), Some(CacheHit::Stale(addrs(10))));
        let snap = c.snapshot();
        assert_eq!(snap.len(), 1);
        assert!(snap[0].stale && snap[0].expires_in.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn negative_entries_expire_into_misses() {
        let c = DnsCache::new(10);
        c.put_negative("nx.com");
        assert_eq!(c.get("nx.com"), Some(CacheHit::Negative));
        assert!(c.snapshot()[0].negative);
        advance(NEGATIVE_TTL + Duration::from_secs(1)).await;
        assert_eq!(c.get("nx.com"), None);
        assert!(c.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn zero_ttl_is_not_cached_and_replaces_old_entries() {
        let c = DnsCache::new(10);
        c.put("a.com", addrs(10));
        c.put("a.com", addrs(0));
        assert_eq!(c.get("a.com"), None);
    }

    #[tokio::test(start_paused = true)]
    async fn lru_evicts_the_least_recently_used() {
        let c = DnsCache::new(2);
        c.put("a.com", addrs(10));
        c.put("b.com", addrs(10));
        assert!(c.get("a.com").is_some()); // touch a
        c.put("c.com", addrs(10)); // evicts b
        assert!(c.get("b.com").is_none());
        assert!(c.get("a.com").is_some() && c.get("c.com").is_some());
        assert_eq!(c.len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_slot_is_single_and_rate_limited() {
        let c = DnsCache::new(10);
        c.put("a.com", addrs(1));
        advance(Duration::from_secs(2)).await;
        assert!(c.begin_refresh("a.com"));
        assert!(!c.begin_refresh("a.com"), "already refreshing");
        c.end_refresh("a.com");
        assert!(!c.begin_refresh("a.com"), "retry within 60 s is refused");
        advance(REFRESH_RETRY_INTERVAL).await;
        assert!(c.begin_refresh("a.com"));
        c.put("a.com", addrs(5)); // a successful refresh replaces the entry and clears the slot
        advance(Duration::from_secs(6)).await;
        assert!(c.begin_refresh("a.com"));
        assert!(!c.begin_refresh("missing.com"));
    }

    #[tokio::test(start_paused = true)]
    async fn flush_and_snapshot_order() {
        let c = DnsCache::new(10);
        c.put("b.com", addrs(10));
        c.put("a.com", addrs(10));
        let names: Vec<String> = c.snapshot().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["a.com", "b.com"]);
        c.flush();
        assert!(c.is_empty());
    }
}
