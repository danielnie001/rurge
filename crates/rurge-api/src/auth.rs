//! `X-Key` / `?x-key=` authentication with a bounded ban table (M4 design
//! D7): five failures from one source within ten minutes ban it for ten
//! minutes; the table never grows past `MAX_TRACKED` sources.

use crate::App;
use crate::error::ApiError;
use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const BAN_FAILURES: usize = 5;
pub const BAN_WINDOW: Duration = Duration::from_secs(600);
pub const BAN_DURATION: Duration = Duration::from_secs(600);
const MAX_TRACKED: usize = 1024;

struct Entry {
    failures: VecDeque<Instant>,
    banned_until: Option<Instant>,
    touched: Instant,
}

pub struct AuthState {
    key: String,
    table: Mutex<HashMap<IpAddr, Entry>>,
}

impl AuthState {
    pub fn new(key: String) -> AuthState {
        AuthState {
            key,
            table: Mutex::new(HashMap::new()),
        }
    }

    pub fn key_matches(&self, presented: &str) -> bool {
        ct_eq(presented.as_bytes(), self.key.as_bytes())
    }

    pub fn is_banned(&self, ip: IpAddr, now: Instant) -> bool {
        let mut table = self.table.lock().expect("ban table");
        let Some(entry) = table.get_mut(&ip) else {
            return false;
        };
        match entry.banned_until {
            Some(until) if now < until => true,
            Some(_) => {
                entry.banned_until = None;
                entry.failures.clear();
                false
            }
            None => false,
        }
    }

    /// Records one failed attempt; `true` when this failure starts a ban.
    pub fn record_failure(&self, ip: IpAddr, now: Instant) -> bool {
        let mut table = self.table.lock().expect("ban table");
        if !table.contains_key(&ip) && table.len() >= MAX_TRACKED {
            // evict the least recently touched source
            if let Some(oldest) = table
                .iter()
                .min_by_key(|(_, e)| e.touched)
                .map(|(ip, _)| *ip)
            {
                table.remove(&oldest);
            }
        }
        let entry = table.entry(ip).or_insert_with(|| Entry {
            failures: VecDeque::new(),
            banned_until: None,
            touched: now,
        });
        entry.touched = now;
        while entry
            .failures
            .front()
            .is_some_and(|&f| now.duration_since(f) > BAN_WINDOW)
        {
            entry.failures.pop_front();
        }
        entry.failures.push_back(now);
        if entry.failures.len() >= BAN_FAILURES {
            entry.banned_until = Some(now + BAN_DURATION);
            entry.failures.clear();
            true
        } else {
            false
        }
    }
}

/// Equal-length inputs are compared without short-circuiting; the length
/// itself is not a secret.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn presented_key(req: &Request) -> Option<String> {
    if let Some(v) = req.headers().get("x-key")
        && let Ok(s) = v.to_str()
    {
        return Some(s.to_string());
    }
    req.uri().query().and_then(|q| {
        q.split('&')
            .find_map(|kv| kv.strip_prefix("x-key="))
            .map(str::to_string)
    })
}

pub async fn require_key(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    let now = Instant::now();
    let ip = peer.ip();
    if app.auth.is_banned(ip, now) {
        return ApiError::banned().into_response();
    }
    match presented_key(&req) {
        Some(k) if app.auth.key_matches(&k) => next.run(req).await,
        _ => {
            if app.auth.record_failure(ip, now) {
                tracing::warn!(%ip, "http-api: too many failed attempts; source banned for 10 minutes");
            }
            ApiError::unauthorized().into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, last])
    }

    #[test]
    fn five_failures_in_the_window_ban_for_ten_minutes() {
        let auth = AuthState::new("k".into());
        let t0 = Instant::now();
        for i in 0..4 {
            assert!(!auth.record_failure(ip(1), t0 + Duration::from_secs(i)));
            assert!(!auth.is_banned(ip(1), t0 + Duration::from_secs(i)));
        }
        assert!(
            auth.record_failure(ip(1), t0 + Duration::from_secs(4)),
            "fifth failure bans"
        );
        assert!(auth.is_banned(ip(1), t0 + Duration::from_secs(5)));
        // banned from t0+4s for BAN_DURATION: still banned one second before it ends
        assert!(auth.is_banned(ip(1), t0 + BAN_DURATION + Duration::from_secs(3)));
        assert!(
            !auth.is_banned(ip(1), t0 + BAN_DURATION + Duration::from_secs(5)),
            "ban expires"
        );
        assert!(!auth.is_banned(ip(2), t0), "other sources are unaffected");
    }

    #[test]
    fn failures_outside_the_window_do_not_count() {
        let auth = AuthState::new("k".into());
        let t0 = Instant::now();
        for i in 0..4 {
            auth.record_failure(ip(1), t0 + Duration::from_secs(i));
        }
        // the fifth comes after the window slid past the first four
        assert!(!auth.record_failure(ip(1), t0 + BAN_WINDOW + Duration::from_secs(10)));
        assert!(!auth.is_banned(ip(1), t0 + BAN_WINDOW + Duration::from_secs(10)));
    }

    #[test]
    fn table_is_bounded() {
        let auth = AuthState::new("k".into());
        let t0 = Instant::now();
        for i in 0..(MAX_TRACKED as u32 + 50) {
            let addr = IpAddr::from(std::net::Ipv4Addr::from(0x0a00_0000 + i));
            auth.record_failure(addr, t0 + Duration::from_millis(i as u64));
        }
        assert!(auth.table.lock().unwrap().len() <= MAX_TRACKED);
    }

    #[test]
    fn key_compare_is_exact() {
        let auth = AuthState::new("s3cret".into());
        assert!(auth.key_matches("s3cret"));
        assert!(!auth.key_matches("s3cre"));
        assert!(!auth.key_matches("s3cret "));
        assert!(!auth.key_matches("S3CRET"));
    }
}
