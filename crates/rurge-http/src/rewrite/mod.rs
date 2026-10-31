//! `[URL Rewrite]` and `[Header Rewrite]` at a request (`phase 4 M3 design 6`,
//! `7`): one generation's rules, what one request's evaluation of them came to,
//! and the counts `/v1/rurge/http` reports.
//!
//! Nothing here touches the network or a session: `url` works out what a
//! request's URL becomes or which answer a redirect or a rejection is,
//! `header` changes a header table in place, and `Rewrite` itself only builds
//! the two TLS configurations a rewritten request to an origin may need
//! (`phase 4 M3 design 8.2`). The engine calls these steps between choosing a
//! policy and dialling (`phase 4 M3 design 8.1`).
//!
//! `RewriteCounters` outlives every generation: one engine has one, reloads
//! keep counting (`phase 4 M3 design 12`).

mod header;
mod url;

use crate::mitm::OriginTls;
use rurge_config::HostName;
use rurge_config::mitm::MitmConfig;
use rurge_config::rewrite::RewriteConfig;
use rurge_proto::BuildError;
use rurge_proto::transport::tls::origin_client_config;
use rustls::RootCertStore;
use rustls::client::ClientConfig;
use rustls::pki_types::ServerName;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub(crate) use header::{Headers, HostChange, write_back_host};
pub(crate) use url::{UrlOutcome, candidates, redirect, retarget_request};

/// How long a rewritten request waits for its origin's TLS handshake: the same
/// 30 seconds `rurge-engine` gives a decrypted connection's origin side
/// (`HANDSHAKE_TIMEOUT`, `phase 4 M3 design 8.2`), written here because this
/// crate does not depend on `rurge-engine`.
pub(crate) const REWRITE_HANDSHAKE: Duration = Duration::from_secs(30);

/// What one request's evaluation of a rule left behind, for the request's
/// notes (`phase 4 M3 design 8.5`): what took effect, what was skipped, and
/// what failed. Nothing here carries a header value, a query string or the
/// gateway's address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Note {
    Done(String),
    Skipped(String),
    Failed(String),
}

impl Note {
    pub(crate) fn text(&self) -> &str {
        match self {
            Note::Done(text) | Note::Skipped(text) | Note::Failed(text) => text,
        }
    }
}

/// The counts `/v1/rurge/http` reports under `rewrite` (`phase 4 M3 design
/// 12`). `body` and `map_local` stay at zero until M3b.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RewriteCounts {
    pub url: u64,
    pub header: u64,
    pub body: u64,
    pub map_local: u64,
    pub skipped: u64,
    pub failed: u64,
}

/// One engine's counts since it started; a reload keeps them (`phase 4 M3
/// design 12`).
#[derive(Debug, Default)]
pub struct RewriteCounters {
    url: AtomicU64,
    header: AtomicU64,
    body: AtomicU64,
    map_local: AtomicU64,
    skipped: AtomicU64,
    failed: AtomicU64,
}

impl RewriteCounters {
    pub fn snapshot(&self) -> RewriteCounts {
        RewriteCounts {
            url: self.url.load(Ordering::Relaxed),
            header: self.header.load(Ordering::Relaxed),
            body: self.body.load(Ordering::Relaxed),
            map_local: self.map_local.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
        }
    }

    /// A request whose URL Rewrite rule took effect.
    pub(crate) fn url(&self) {
        self.url.fetch_add(1, Ordering::Relaxed);
    }

    /// A request whose headers took effect, at most once each.
    pub(crate) fn header(&self) {
        self.header.fetch_add(1, Ordering::Relaxed);
    }

    /// One rewrite that was skipped and left a note.
    pub(crate) fn skipped(&self) {
        self.skipped.fetch_add(1, Ordering::Relaxed);
    }

    /// A request that failed because a rewrite changed its framing.
    pub(crate) fn failed(&self) {
        self.failed.fetch_add(1, Ordering::Relaxed);
    }
}

/// One generation's rewrite rules and the TLS it dials an origin with
/// (`phase 4 M3 design 8.2`): built once per generation, shared by every
/// request of it.
pub struct Rewrite {
    url: Vec<UrlRule>,
    header: Vec<HeaderRule>,
    config: Arc<ClientConfig>,
    h1: Arc<ClientConfig>,
    counters: Arc<RewriteCounters>,
}

/// One `[URL Rewrite]` line, ready to evaluate.
struct UrlRule {
    pattern: rurge_config::rule::Pattern,
    replacement: rurge_config::dialect::Template,
    mode: rurge_config::rewrite::UrlMode,
    gateway: bool,
}

/// One `[Header Rewrite]` line, with its name and value already read as the
/// header table takes them.
struct HeaderRule {
    direction: rurge_config::rewrite::Direction,
    pattern: rurge_config::rule::Pattern,
    /// The field name as the line wrote it, for the note.
    name: String,
    action: HeaderRuleAction,
}

enum HeaderRuleAction {
    Add {
        header: http::HeaderName,
        value: http::HeaderValue,
    },
    Del {
        header: http::HeaderName,
    },
    Replace {
        header: http::HeaderName,
        value: http::HeaderValue,
    },
    ReplaceRegex {
        header: http::HeaderName,
        regex: rurge_config::rule::Pattern,
        template: rurge_config::dialect::Template,
    },
}

impl HeaderRuleAction {
    fn verb(&self) -> &'static str {
        match self {
            HeaderRuleAction::Add { .. } => "header-add",
            HeaderRuleAction::Del { .. } => "header-del",
            HeaderRuleAction::Replace { .. } => "header-replace",
            HeaderRuleAction::ReplaceRegex { .. } => "header-replace-regex",
        }
    }

    fn header(&self) -> &http::HeaderName {
        match self {
            HeaderRuleAction::Add { header, .. }
            | HeaderRuleAction::Del { header }
            | HeaderRuleAction::Replace { header, .. }
            | HeaderRuleAction::ReplaceRegex { header, .. } => header,
        }
    }
}

impl Rewrite {
    /// The rules of this generation, or `None` when neither section has a
    /// usable line. `roots` is the trust of the origin side, as `Mitm::build`
    /// takes it; the two configurations are `[MITM]`'s own, so a plain
    /// `http://` rewritten to `https://` needs no certificate authority
    /// (`phase 4 M3 design 8.2`).
    pub fn build(
        cfg: &RewriteConfig,
        mitm: &MitmConfig,
        roots: Arc<RootCertStore>,
        counters: Arc<RewriteCounters>,
    ) -> Result<Option<Rewrite>, BuildError> {
        if cfg.is_empty() {
            return Ok(None);
        }
        // what the origin side is offered, as `Mitm::build` offers it: `h2`
        // beside `http/1.1` when this generation decrypts HTTP/2, and a
        // configuration that offers `http/1.1` alone for an upgrade or a
        // tunnel (`phase 4 M3 design 8.2`)
        let alpn: &[&[u8]] = if mitm.h2 {
            &[b"h2", b"http/1.1"]
        } else {
            &[b"http/1.1"]
        };
        let config = origin_client_config(Arc::clone(&roots), mitm.skip_server_cert_verify, alpn)?;
        let h1 = if mitm.h2 {
            origin_client_config(roots, mitm.skip_server_cert_verify, &[b"http/1.1"])?
        } else {
            Arc::clone(&config)
        };
        Ok(Some(Rewrite {
            url: cfg.url.iter().map(url_rule).collect(),
            header: cfg.header.iter().filter_map(header_rule).collect(),
            config,
            h1,
            counters,
        }))
    }

    /// This generation's counters, the ones `/v1/rurge/http` reads.
    pub(crate) fn counters(&self) -> &RewriteCounters {
        &self.counters
    }

    /// The TLS a request that a rewrite sent to `name` dials with (`phase 4 M3
    /// design 8.2`): this generation's two configurations and the 30 seconds a
    /// handshake may take.
    pub(crate) fn origin_tls(&self, name: ServerName<'static>) -> OriginTls {
        OriginTls {
            config: Arc::clone(&self.config),
            h1: Arc::clone(&self.h1),
            name,
            wait: REWRITE_HANDSHAKE,
        }
    }

    /// Whether any rule acts on a response.
    pub(crate) fn has_response_rules(&self) -> bool {
        self.header
            .iter()
            .any(|rule| rule.direction == rurge_config::rewrite::Direction::Response)
    }

    /// Evaluates the URL rules in order against a request's candidate URLs
    /// (`phase 4 M3 design 6.2`, the phase 4 / M3a plan's P9). `gateway` is
    /// asked only by a `302` / `307` rule whose replacement holds the
    /// placeholder and only once that rule matched.
    pub(crate) fn url(
        &self,
        candidates: &[String],
        gateway: &dyn Fn() -> Option<Ipv4Addr>,
        notes: &mut Vec<Note>,
    ) -> UrlOutcome {
        url::evaluate(self, candidates, gateway, notes)
    }

    /// Evaluates the header rules of one direction against a URL and a header
    /// table (`phase 4 M3 design 7.2` – `7.4`, the phase 4 / M3a plan's P9).
    pub(crate) fn headers(
        &self,
        direction: rurge_config::rewrite::Direction,
        url: &str,
        headers: &mut http::HeaderMap,
        notes: &mut Vec<Note>,
    ) -> Headers {
        header::evaluate(self, direction, url, headers, notes)
    }
}

/// `text` with every non-overlapping match of `pattern` replaced by
/// `template`'s expansion, from left to right (`phase 4 M3 design 5.5`).
///
/// `Err` when the expression hits its backtracking limit: fancy-regex's own
/// `replace_all` would panic there, which is why this is written out
/// (`the phase 4 / M3a plan's execution record row 6`).
pub(super) fn replace_all(
    pattern: &rurge_config::rule::Pattern,
    template: &rurge_config::dialect::Template,
    text: &str,
) -> Result<String, ()> {
    let groups = pattern.regex.captures_len() - 1;
    let mut out = String::with_capacity(text.len());
    let mut end = 0;
    for captures in pattern.regex.captures_iter(text) {
        let captures = captures.map_err(|_| ())?;
        let whole = captures.get(0).expect("every match is group zero");
        out.push_str(&text[end..whole.start()]);
        template.expand(
            &|group| captures.get(group).map(|found| found.as_str()),
            groups,
            &mut out,
        );
        end = whole.end();
    }
    out.push_str(&text[end..]);
    Ok(out)
}

/// One `[URL Rewrite]` line, ready to evaluate. Nothing in it can fail: the
/// expression was compiled at load time and the replacement is just text.
fn url_rule(rule: &rurge_config::rewrite::UrlRewrite) -> UrlRule {
    UrlRule {
        pattern: rule.pattern.clone(),
        replacement: rule.replacement.clone(),
        mode: rule.mode,
        gateway: rule.gateway,
    }
}

/// One `[Header Rewrite]` line, with its name and value read as the header
/// table takes them. The loader has checked both, so a rule that still does not
/// read is one this build cannot carry: it is dropped, and only the field name
/// is said.
fn header_rule(rule: &rurge_config::rewrite::HeaderRewrite) -> Option<HeaderRule> {
    use rurge_config::rewrite::HeaderAction;

    let action = match &rule.action {
        HeaderAction::Add { name, value } => HeaderRuleAction::Add {
            header: header_name(name)?,
            value: header_value(name, value)?,
        },
        HeaderAction::Del { name } => HeaderRuleAction::Del {
            header: header_name(name)?,
        },
        HeaderAction::Replace { name, value } => HeaderRuleAction::Replace {
            header: header_name(name)?,
            value: header_value(name, value)?,
        },
        HeaderAction::ReplaceRegex {
            name,
            regex,
            template,
        } => HeaderRuleAction::ReplaceRegex {
            header: header_name(name)?,
            regex: regex.clone(),
            template: template.clone(),
        },
    };
    Some(HeaderRule {
        direction: rule.direction,
        pattern: rule.pattern.clone(),
        name: rule.action.name().to_string(),
        action,
    })
}

/// The field name a line wrote, as the header table takes it.
fn header_name(name: &str) -> Option<http::HeaderName> {
    match http::HeaderName::from_bytes(name.as_bytes()) {
        Ok(name) => Some(name),
        Err(_) => {
            tracing::debug!(
                name,
                "a [Header Rewrite] rule names a field this build cannot carry; the rule is dropped"
            );
            None
        }
    }
}

/// The value a line wrote, as the header table takes it. Nothing of the value
/// is said, and it stays out of every note (`phase 4 M3 design 8.5`).
fn header_value(name: &str, value: &str) -> Option<http::HeaderValue> {
    match http::HeaderValue::from_bytes(value.as_bytes()) {
        Ok(value) => Some(value),
        Err(_) => {
            tracing::debug!(
                name,
                "a [Header Rewrite] rule carries a value this build cannot take; the rule is dropped"
            );
            None
        }
    }
}

/// The rewrite rules of a profile text holding only the two sections, read as
/// the loader reads a profile: a line that does not read is a test that fails
/// here, not a rule that quietly does nothing.
#[cfg(test)]
pub(crate) fn config_of(lines: &str) -> RewriteConfig {
    let (cfg, diags) = read_rules(lines);
    let unexpected: Vec<&str> = diags
        .iter()
        .filter(|d| d.code != "W0041")
        .map(|d| d.message.as_str())
        .collect();
    assert!(
        unexpected.is_empty(),
        "the rules read cleanly but for a framing warning: {lines}: {unexpected:?}"
    );
    cfg
}

/// The two sections of `lines` read as the loader reads them, with what it
/// said about them. A rule on `Content-Length` or `Transfer-Encoding` brings a
/// `W0041` and is kept (`phase 4 M3 design 13`), so a case about framing gets
/// that one warning and no other.
#[cfg(test)]
fn read_rules(lines: &str) -> (RewriteConfig, Vec<rurge_config::Diagnostic>) {
    use rurge_config::text::{Origin, Section, parse_str};
    use std::path::Path;

    let text = format!("{lines}\n");
    let (profile, mut diags) = parse_str(&text, Arc::from(Path::new("rewrite.conf")), Origin::Main);
    let cfg = RewriteConfig {
        url: rurge_config::rewrite::parse_url(
            profile
                .section("URL Rewrite")
                .into_iter()
                .flat_map(Section::active_entries),
            &mut diags,
        ),
        header: rurge_config::rewrite::parse_header(
            profile
                .section("Header Rewrite")
                .into_iter()
                .flat_map(Section::active_entries),
            &mut diags,
        ),
        body: Vec::new(),
    };
    (cfg, diags.into_vec())
}

/// A trust store holding one CA made in memory: the side of a case that checks
/// certificates never reads a system store (`phase 4 M3 design 16`).
#[cfg(test)]
pub(crate) fn test_roots() -> Arc<RootCertStore> {
    let ca =
        crate::mitm::testing::generated(&rcgen::PKCS_ECDSA_P256_SHA256, "rurge test rewrite CA");
    let mut roots = RootCertStore::empty();
    roots.add(ca.cert.clone()).expect("a trust anchor");
    Arc::new(roots)
}

/// One generation built from the two sections of `lines`, trusted by a CA made
/// in memory.
#[cfg(test)]
pub(crate) fn rewrite_of(lines: &str) -> Rewrite {
    Rewrite::build(
        &config_of(lines),
        &MitmConfig::default(),
        test_roots(),
        Arc::new(RewriteCounters::default()),
    )
    .expect("a generation")
    .expect("the lines make rules")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_gives_none_without_lines() {
        let rewritten = Rewrite::build(
            &RewriteConfig::default(),
            &MitmConfig::default(),
            test_roots(),
            Arc::new(RewriteCounters::default()),
        )
        .expect("no rules read cleanly");
        assert!(rewritten.is_none());
    }

    #[test]
    fn build_makes_the_two_tls_configs() {
        let cfg = config_of("[URL Rewrite]\n^http://a\\.test http://b.test header");
        let mitm = MitmConfig {
            h2: true,
            ..MitmConfig::default()
        };
        let rewritten = Rewrite::build(
            &cfg,
            &mitm,
            test_roots(),
            Arc::new(RewriteCounters::default()),
        )
        .expect("a generation")
        .expect("the line makes a generation");
        let name = ServerName::try_from("a.test").expect("a name");
        let origin = rewritten.origin_tls(name.clone());
        assert_eq!(
            origin.config.alpn_protocols,
            [b"h2".to_vec(), b"http/1.1".to_vec()]
        );
        assert_eq!(origin.h1.alpn_protocols, [b"http/1.1".to_vec()]);
        assert_eq!(origin.wait, Duration::from_secs(30));
        assert_eq!(origin.name, name);

        let rewritten = Rewrite::build(
            &cfg,
            &MitmConfig::default(),
            test_roots(),
            Arc::new(RewriteCounters::default()),
        )
        .expect("a generation")
        .expect("the line makes a generation");
        let origin = rewritten.origin_tls(name.clone());
        assert_eq!(origin.config.alpn_protocols, [b"http/1.1".to_vec()]);
        assert_eq!(origin.h1.alpn_protocols, [b"http/1.1".to_vec()]);

        // rustls refuses an empty store of roots, so a generation cannot be
        // built from one: every case that builds an engine hands its roots in
        // (the phase 4 / M3a plan's execution record row 20).
        let empty = Arc::new(RootCertStore::empty());
        assert!(
            Rewrite::build(
                &cfg,
                &MitmConfig::default(),
                empty,
                Arc::new(RewriteCounters::default())
            )
            .is_err()
        );
    }

    #[test]
    fn counters_count_and_snapshot() {
        let counters = RewriteCounters::default();
        counters.url();
        counters.header();
        counters.header();
        counters.skipped();
        counters.failed();
        assert_eq!(
            counters.snapshot(),
            RewriteCounts {
                url: 1,
                header: 2,
                body: 0,
                map_local: 0,
                skipped: 1,
                failed: 1,
            }
        );
    }

    /// Notes name the rule that acted, never a header value and never a query
    /// string (`phase 4 M3 design 8.5`, the phase 4 / M3a plan's P11).
    #[test]
    fn notes_never_carry_values_or_queries() {
        use rurge_config::rewrite::Direction;

        let rewrite = rewrite_of(
            "[URL Rewrite]\n^http://a\\.test http://b.test header\n\
             [Header Rewrite]\n\
             ^http://a\\.test header-add X-Token v4lue-x\n\
             ^http://a\\.test header-replace-regex X-Token v4lue t3mpl",
        );
        let url = "http://a.test/p?tok=q1";
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::HOST, http::HeaderValue::from_static("a.test"));
        let mut notes = Vec::new();
        // the request side of the headers comes first, then the URL rules
        rewrite.headers(Direction::Request, url, &mut headers, &mut notes);
        rewrite.url(&[url.to_string()], &|| None, &mut notes);
        assert!(!notes.is_empty());
        for note in &notes {
            let text = note.text();
            for leak in ["q1", "v4lue-x", "t3mpl"] {
                assert!(!text.contains(leak), "{text}");
            }
        }
        assert_eq!(
            notes.last().map(Note::text),
            Some("URL Rewrite: header by ^http://a\\.test -> http://b.test/p")
        );
    }
}
