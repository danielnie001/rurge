//! Where the loader gets the text of what a profile names by a URL.
//!
//! The loader never goes to the network. Whoever loads a profile hands it a
//! [`RemoteSource`], which answers from what was downloaded earlier, and the
//! loader reports in `SourceInfo::remote` what it asked for, so that the
//! caller can download what is missing and keep the rest up to date.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime};
use url::Url;

/// How often a remote file is updated when its own text does not say: an
/// included file without a `#!MANAGED-CONFIG` line, and a `#!MANAGED-CONFIG`
/// line without `interval=`.
pub(crate) const DEFAULT_INTERVAL: Duration = Duration::from_secs(86400);

/// Where the loader asks for the text a URL in a profile names.
pub trait RemoteSource: Send + Sync {
    /// What `url` was last downloaded as. The URL is the parsed value, so that
    /// the source and the loader mean the same thing by it.
    ///
    /// When there is no text the answer is [`Remote::Missing`], and its `why`
    /// is shown as it is in a warning, which is printed and logged: it is one
    /// line, and it never holds the URL, nor any other link (the target of a
    /// redirect included), nor a credential, nor the text of a response body
    /// as such. It can be the message of a diagnostic about a downloaded text
    /// (`does not load: …`), which names what any diagnostic names (a policy,
    /// a group, a section) and quotes no value.
    fn get(&self, url: &Url) -> Remote;
}

/// What a [`RemoteSource`] has for a URL.
#[derive(Clone)]
pub enum Remote {
    /// The downloaded text. `path` is the file it is kept in: a diagnostic
    /// about one of its lines points there, where the user can open it.
    /// `fetched_at` is when it was last downloaded or confirmed up to date.
    Text {
        text: String,
        path: PathBuf,
        fetched_at: SystemTime,
    },
    /// Nothing to read: not downloaded yet, or every attempt failed and no
    /// copy exists. `why` is shown as it is in a warning, which is printed and
    /// logged: it is one line, and it never holds the URL, nor any other link
    /// (the target of a redirect included), nor a credential, nor the text of
    /// a response body as such. A link can be a token, and a body is whatever
    /// the server sent. It can be the message of a diagnostic about a
    /// downloaded text (`does not load: …`), which names what any diagnostic
    /// names (a policy, a group, a section) and quotes no value.
    Missing { why: Option<String> },
}

/// The text of a profile can hold credentials: `Debug` says how long it is.
impl fmt::Debug for Remote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Remote::Text {
                text,
                path,
                fetched_at,
            } => f
                .debug_struct("Text")
                .field("text", &format_args!("{} bytes", text.len()))
                .field("path", path)
                .field("fetched_at", fetched_at)
                .finish(),
            Remote::Missing { why } => f.debug_struct("Missing").field("why", why).finish(),
        }
    }
}

/// What a remote file is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteRole {
    /// The profile that is kept up to date from the `#!MANAGED-CONFIG` line
    /// that is in force.
    Managed,
    /// A profile on the way to it: its own `#!MANAGED-CONFIG` line names
    /// another URL. It is read from the copy that exists and not updated.
    ManagedHop,
    /// A file a `#!include` line names.
    Include,
}

/// A remote file one load came across: one per URL, in the order the load
/// first came to it.
#[derive(Clone, PartialEq, Eq)]
pub struct RemoteRef {
    /// The link as parsed. `Debug` shows `scheme://host[:port]/…` and nothing
    /// more of it: its user name, password, path and query can be the token a
    /// provider issued.
    pub url: Url,
    /// The role of the first line that named it.
    pub role: RemoteRole,
    /// The file the text is in, when this load got the text.
    pub path: Option<PathBuf>,
    /// How often it is updated.
    pub interval: Duration,
    /// Whether the profile wants nothing served once an update is overdue and
    /// failing. Only the managed profile can ask for it.
    pub strict: bool,
    /// An included text that lacks a section some `#!include` line takes it
    /// for: not the profile it is meant to be, so a copy like that must not be
    /// adopted.
    pub lacks_section: bool,
}

impl RemoteRef {
    /// Whether this load got the text.
    pub fn found(&self) -> bool {
        self.path.is_some()
    }
}

/// A source that asks the one it wraps once per URL and answers the same from
/// then on. A load comes back to an address more than once, from the walk along
/// a managed profile and from the includes of the text in effect, and what the
/// wrapped source reads can change in between (a download lands on the disk):
/// a load must read one text for an address.
pub(crate) struct Memo<'a> {
    source: &'a dyn RemoteSource,
    answers: Mutex<HashMap<Url, Remote>>,
}

impl<'a> Memo<'a> {
    pub(crate) fn new(source: &'a dyn RemoteSource) -> Memo<'a> {
        Memo {
            source,
            answers: Mutex::new(HashMap::new()),
        }
    }
}

impl RemoteSource for Memo<'_> {
    fn get(&self, url: &Url) -> Remote {
        let mut answers = self.answers.lock().unwrap_or_else(PoisonError::into_inner);
        answers
            .entry(url.clone())
            .or_insert_with(|| self.source.get(url))
            .clone()
    }
}

/// Where a link goes, without its user name, password, path and query: all
/// that a `Debug`, a label or a log line may show of a link, whose other parts
/// can be the token a provider issued.
pub fn origin(url: &Url) -> String {
    format!(
        "{}://{}/…",
        url.scheme(),
        &url[url::Position::BeforeHost..url::Position::AfterPort]
    )
}

impl fmt::Debug for RemoteRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let RemoteRef {
            url,
            role,
            path,
            interval,
            strict,
            lacks_section,
        } = self;
        f.debug_struct("RemoteRef")
            .field("url", &origin(url))
            .field("role", role)
            .field("path", path)
            .field("interval", interval)
            .field("strict", strict)
            .field("lacks_section", lacks_section)
            .finish()
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::Mutex;

    /// An instant `n` hours after one well in the past: the tests give a copy
    /// its time and a local file its modification time hours apart, never
    /// "now" against "now".
    pub(crate) fn hours(n: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + n * 3600)
    }

    /// A source that answers from a table, and remembers what it was asked.
    #[derive(Default)]
    pub(crate) struct Fake {
        answers: HashMap<Url, Remote>,
        asked: Mutex<Vec<Url>>,
    }

    impl Fake {
        pub(crate) fn new() -> Fake {
            Fake::default()
        }

        /// `url` was downloaded as `text`, which is kept in a file of its own.
        pub(crate) fn text(self, url: &str, text: &str) -> Fake {
            let path = PathBuf::from(format!("/cache/{}.conf", self.answers.len() + 1));
            self.text_in(url, text, &path)
        }

        pub(crate) fn text_in(mut self, url: &str, text: &str, path: &Path) -> Fake {
            self.answers.insert(
                Url::parse(url).unwrap(),
                Remote::Text {
                    text: text.to_string(),
                    path: path.to_path_buf(),
                    fetched_at: SystemTime::UNIX_EPOCH,
                },
            );
            self
        }

        /// The text of `url` was downloaded, or last confirmed up to date, at
        /// `at`.
        pub(crate) fn fetched(mut self, url: &str, at: SystemTime) -> Fake {
            match self.answers.get_mut(&Url::parse(url).unwrap()) {
                Some(Remote::Text { fetched_at, .. }) => *fetched_at = at,
                _ => panic!("no text for {url}"),
            }
            self
        }

        pub(crate) fn missing(mut self, url: &str, why: Option<&str>) -> Fake {
            self.answers.insert(
                Url::parse(url).unwrap(),
                Remote::Missing {
                    why: why.map(str::to_string),
                },
            );
            self
        }

        /// The file the text of `url` is kept in.
        pub(crate) fn path_of(&self, url: &str) -> PathBuf {
            match &self.answers[&Url::parse(url).unwrap()] {
                Remote::Text { path, .. } => path.clone(),
                Remote::Missing { .. } => panic!("no text for {url}"),
            }
        }

        /// Every URL the loader asked for, in order.
        pub(crate) fn asked(&self) -> Vec<Url> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl RemoteSource for Fake {
        fn get(&self, url: &Url) -> Remote {
            self.asked.lock().unwrap().push(url.clone());
            self.answers.get(url).cloned().unwrap_or(Remote::Missing {
                why: Some("the fake has no answer for it".to_string()),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The link is a credential as a whole: only where it goes is shown.
    #[test]
    fn a_link_is_shown_as_where_it_goes_and_nothing_more() {
        for (link, shown) in [
            ("https://example.com/a/b?c=d", "https://example.com/…"),
            (
                "http://al1ce:pw@example.com:8080/x?t=1#f",
                "http://example.com:8080/…",
            ),
            ("https://[::1]:8443/x", "https://[::1]:8443/…"),
            ("HTTPS://Example.COM/x", "https://example.com/…"),
        ] {
            let linked = RemoteRef {
                url: Url::parse(link).unwrap(),
                role: RemoteRole::Include,
                path: None,
                interval: DEFAULT_INTERVAL,
                strict: false,
                lacks_section: false,
            };
            let debug = format!("{linked:?}");
            assert!(debug.contains(&format!("url: {shown:?}")), "{debug}");
            for part in ["a/b", "al1ce", "pw", "c=d", "t=1", "/x"] {
                assert!(!debug.contains(part), "{part}: {debug}");
            }
        }
    }

    /// A source whose answer is not the same twice, as the disk is when a
    /// download lands between two asks: nothing the first time, a text after.
    struct Changing(std::sync::atomic::AtomicUsize);

    impl RemoteSource for Changing {
        fn get(&self, _: &Url) -> Remote {
            match self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                0 => Remote::Missing {
                    why: Some("not yet".to_string()),
                },
                n => Remote::Text {
                    text: format!("answer {n}"),
                    path: PathBuf::from("/cache/1.conf"),
                    fetched_at: SystemTime::UNIX_EPOCH,
                },
            }
        }
    }

    /// What `memo` has for `url`, as a word: the reason or the text.
    fn said(memo: &Memo<'_>, url: &Url) -> String {
        match memo.get(url) {
            Remote::Missing { why } => why.unwrap_or_default(),
            Remote::Text { text, .. } => text,
        }
    }

    /// An address keeps the answer it got first, however often it is asked
    /// for, and the source is asked once for each: an answer that said nothing
    /// is as good as any other.
    #[test]
    fn a_source_behind_a_memo_answers_the_same_for_the_rest_of_its_life() {
        let source = Changing(Default::default());
        let memo = Memo::new(&source);
        let a = Url::parse("https://example.com/a.conf").unwrap();
        let b = Url::parse("https://example.com/b.conf").unwrap();
        assert_eq!(said(&memo, &a), "not yet");
        assert_eq!(said(&memo, &b), "answer 1");
        assert_eq!(said(&memo, &a), "not yet");
        assert_eq!(said(&memo, &b), "answer 1");
        assert_eq!(source.0.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    /// The text of a profile can hold credentials.
    #[test]
    fn a_downloaded_text_is_not_shown_by_debug() {
        let text = Remote::Text {
            text: "password = S3CR3T".to_string(),
            path: PathBuf::from("/cache/1.conf"),
            fetched_at: SystemTime::UNIX_EPOCH,
        };
        let shown = format!("{text:?}");
        assert!(!shown.contains("S3CR3T"), "{shown}");
        assert!(shown.contains("17 bytes"), "{shown}");
        let missing = Remote::Missing {
            why: Some("connection refused".to_string()),
        };
        assert!(format!("{missing:?}").contains("connection refused"));
    }
}
