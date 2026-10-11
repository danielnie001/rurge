//! `#!MANAGED-CONFIG` and other header directives, and which text of a managed
//! profile is in effect.
//!
//! A managed profile is kept up to date from a URL. rurge never writes the
//! user's file, so what is downloaded is a copy kept in the data directory,
//! and every load chooses between the local file and that copy. The loader
//! never goes to the network: the copy is what the load's [`RemoteSource`]
//! has for the address.

use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::remote::{DEFAULT_INTERVAL, Remote, RemoteRef, RemoteRole, RemoteSource, origin};
use crate::span::Span;
use crate::text::{Directive, Origin, Profile, parse_str, unknown_directive};
use crate::value::parse_bool;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use url::Url;

/// The shortest interval a managed profile is updated at: a shorter one would
/// have the server asked every few seconds.
const SHORTEST_INTERVAL: Duration = Duration::from_secs(60);

/// How many times the walk along the copies follows a line to another address.
const MOST_CHANGES: usize = 8;

#[derive(Clone, PartialEq, Eq)]
pub struct ManagedConfig {
    pub url: String,
    pub interval: Duration,
    pub strict: bool,
}

/// The URL is left blank: the link a provider issues carries the
/// subscription token.
impl fmt::Debug for ManagedConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ManagedConfig {
            url: _,
            interval,
            strict,
        } = self;
        f.debug_struct("ManagedConfig")
            .field("url", &"***")
            .field("interval", interval)
            .field("strict", strict)
            .finish()
    }
}

impl ManagedConfig {
    /// Where the profile is downloaded from, as much of the link as may be
    /// shown: the link is a credential as a whole.
    pub(crate) fn shown(&self) -> String {
        match Url::parse(&self.url) {
            Ok(url) => origin(&url),
            Err(_) => "***".to_string(),
        }
    }
}

/// What a diagnostic says of a parameter it does not know: its name, when that
/// is a lower-case word like the ones the format has (`interval`, `strict`).
/// What follows the `=` can be a token, and so can a word that is no
/// `name=value` at all: a link pasted twice has its `=` somewhere in its
/// query, a Base64 string has it for padding.
fn describe(parameter: &str) -> String {
    match parameter.split_once('=') {
        Some((name, _))
            if (1..=32).contains(&name.len())
                && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') =>
        {
            format!("`{name}`")
        }
        _ => "(not written as a lower-case `name=value`)".to_string(),
    }
}

/// The value of `interval` or `strict` in a diagnostic, when it can be what
/// somebody wrote for that parameter: a short run of `plain` characters. A
/// link or a token that lost its space is glued to it otherwise.
fn quoted(value: &str, plain: impl Fn(char) -> bool) -> String {
    if (1..=20).contains(&value.len()) && value.chars().all(plain) {
        format!(" `{value}`")
    } else {
        String::new()
    }
}

/// The address a `#!MANAGED-CONFIG` line names: an http(s) link. A fragment
/// is never sent to a server, so it is no part of the address.
fn http_address(text: &str) -> Option<Url> {
    let mut url = Url::parse(text).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    url.set_fragment(None);
    Some(url)
}

/// A `#!MANAGED-CONFIG` line that is in force.
pub(crate) struct Line {
    pub(crate) config: ManagedConfig,
    /// The address as parsed, without its fragment.
    pub(crate) address: Url,
    pub(crate) span: Span,
}

/// The `#!MANAGED-CONFIG` line of a header that is in force: the last one
/// that names an address.
pub(crate) fn find(header: &[Directive], diags: &mut Diagnostics) -> Option<Line> {
    let mut found = None;
    for d in header {
        if let Some(rest) = d.raw.strip_prefix("#!MANAGED-CONFIG") {
            let mut parts = rest.split_whitespace();
            let Some(url) = parts.next() else {
                diags.push(
                    Diagnostic::warning(
                        codes::W_INVALID_VALUE,
                        "#!MANAGED-CONFIG without a URL is ignored",
                    )
                    .at(d.span.clone()),
                );
                continue;
            };
            // what is not a link is not quoted: it can be a token
            let Some(address) = http_address(url) else {
                diags.push(
                    Diagnostic::warning(
                        codes::W_INVALID_VALUE,
                        "the #!MANAGED-CONFIG URL is not an http(s) URL; the line is ignored",
                    )
                    .at(d.span.clone()),
                );
                continue;
            };
            let mut cfg = ManagedConfig {
                url: url.to_string(),
                interval: DEFAULT_INTERVAL,
                strict: false,
            };
            for p in parts {
                match p.split_once('=') {
                    Some(("interval", v)) => match v.parse::<u64>() {
                        // as if it were not written
                        Ok(0) => {}
                        Ok(s) if s < SHORTEST_INTERVAL.as_secs() => {
                            cfg.interval = SHORTEST_INTERVAL;
                            diags.push(
                                Diagnostic::warning(
                                    codes::W_INVALID_VALUE,
                                    "interval below 60 seconds; 60 is used",
                                )
                                .at(d.span.clone()),
                            );
                        }
                        Ok(s) => cfg.interval = Duration::from_secs(s),
                        Err(_) => diags.push(
                            Diagnostic::warning(
                                codes::W_INVALID_VALUE,
                                format!("invalid interval{}", quoted(v, |c| c.is_ascii_digit())),
                            )
                            .at(d.span.clone()),
                        ),
                    },
                    Some(("strict", v)) => match parse_bool(v) {
                        Some(b) => cfg.strict = b,
                        None => diags.push(
                            Diagnostic::warning(
                                codes::W_INVALID_VALUE,
                                format!("invalid strict{}", quoted(v, |c| c.is_ascii_alphabetic())),
                            )
                            .at(d.span.clone()),
                        ),
                    },
                    _ => diags.push(
                        Diagnostic::warning(
                            codes::W_INVALID_VALUE,
                            format!("unknown MANAGED-CONFIG parameter {}", describe(p)),
                        )
                        .at(d.span.clone()),
                    ),
                }
            }
            found = Some(Line {
                config: cfg,
                address,
                span: d.span.clone(),
            });
        } else if d.raw.starts_with("#!FORBIDDEN-AUTO-UPGRADE") {
            // rurge never auto-upgrades profiles; nothing to do.
        } else {
            diags.push(
                Diagnostic::warning(codes::W_UNKNOWN_DIRECTIVE, unknown_directive(&d.raw))
                    .at(d.span.clone()),
            );
        }
    }
    found
}

pub fn parse_managed(header: &[Directive], diags: &mut Diagnostics) -> Option<ManagedConfig> {
    find(header, diags).map(|line| line.config)
}

/// A downloaded copy of the managed profile, read: its lines carry
/// [`Origin::Managed`] and stand in the file that holds it.
pub(crate) struct Downloaded {
    pub(crate) profile: Profile,
    /// What reading it had to say.
    pub(crate) diagnostics: Diagnostics,
    pub(crate) file: PathBuf,
}

/// What a load goes on with, and what choosing it came across.
pub(crate) struct Choice {
    /// The copy that is in effect; `None` when the local file is.
    pub(crate) copy: Option<Downloaded>,
    /// The remote files the walk was at, in order.
    pub(crate) remote: Vec<RemoteRef>,
    /// What is said of the choice, at the line of the local file.
    pub(crate) diagnostics: Diagnostics,
}

/// An address the walk was at.
struct Stop {
    address: Url,
    /// The file of its copy, when it has one.
    file: Option<PathBuf>,
}

/// How the walk along the copies ended.
enum End {
    /// The address has no copy: the reason, when the source gave one.
    NotFound(Option<String>),
    /// The copy names its own address: it is where the profile is served from.
    Found,
    /// The copy has no `#!MANAGED-CONFIG` line.
    NoLine,
    /// The line of the copy leads to an address that is too far, or one the
    /// walk was at.
    NoWayOut,
}

/// A copy the walk found, with its own line.
struct Held {
    copy: Downloaded,
    line: Option<Line>,
}

struct Walk {
    /// The addresses the walk was at, in order.
    stops: Vec<Stop>,
    end: End,
    /// The last copy found.
    last: Option<Held>,
    /// When the newest of the copies on the way was fetched.
    newest: Option<SystemTime>,
}

/// Follows the lines of the copies from `start`: an address is asked for, the
/// line of its copy is read, and when that names another address the walk goes
/// on there.
fn walk(start: &Url, source: &dyn RemoteSource) -> Walk {
    let mut stops: Vec<Stop> = Vec::new();
    let mut last: Option<Held> = None;
    let mut newest: Option<SystemTime> = None;
    let mut at = start.clone();
    let end = loop {
        match source.get(&at) {
            Remote::Missing { why } => {
                stops.push(Stop {
                    address: at,
                    file: None,
                });
                break End::NotFound(why);
            }
            Remote::Text {
                text,
                path,
                fetched_at,
            } => {
                let file: Arc<Path> = Arc::from(path.as_path());
                let (profile, diagnostics) = parse_str(&text, file.clone(), Origin::Managed(file));
                // what is wrong in the line is told once, if the copy is in
                // effect, when its profile is read
                let line = find(&profile.header, &mut Diagnostics::default());
                let named = line.as_ref().map(|line| line.address.clone());
                newest = newest.max(Some(fetched_at));
                stops.push(Stop {
                    address: at.clone(),
                    file: Some(path.clone()),
                });
                last = Some(Held {
                    copy: Downloaded {
                        profile,
                        diagnostics,
                        file: path,
                    },
                    line,
                });
                match named {
                    None => break End::NoLine,
                    Some(next) if next == at => break End::Found,
                    Some(next)
                        if stops.len() > MOST_CHANGES
                            || stops.iter().any(|stop| stop.address == next) =>
                    {
                        break End::NoWayOut;
                    }
                    Some(next) => at = next,
                }
            }
        }
    };
    Walk {
        stops,
        end,
        last,
        newest,
    }
}

/// Chooses the text of the managed profile named by `local`, the line of the
/// local file. `has_section` is whether the local file has a section of its
/// own, and `modified` when it was last changed, when that can be told.
///
/// The local file is in effect when no copy was found, and otherwise only when
/// it has something to say and was changed after every copy on the way was
/// fetched, not in the future: then the user edited it since the last update.
/// A file that is only the line that points to the address, or whose time is
/// not to be trusted, never keeps the copy out.
pub(crate) fn choose(
    local: &Line,
    has_section: bool,
    modified: Option<SystemTime>,
    source: &dyn RemoteSource,
) -> Choice {
    let Walk {
        stops,
        end,
        last,
        newest,
    } = walk(&local.address, source);
    let local_in_effect = last.is_none()
        || (has_section
            && modified.zip(newest).is_some_and(|(modified, newest)| {
                modified > newest && modified <= SystemTime::now()
            }));

    let mut diagnostics = Diagnostics::default();
    let mut say = |d: Diagnostic| diagnostics.push(d.at(local.span.clone()));
    if let End::NotFound(why) = &end {
        say(Diagnostic::warning(
            codes::W_RESOURCE_UNAVAILABLE,
            match why {
                Some(why) => format!("managed profile not downloaded yet: {why}"),
                None => "managed profile not downloaded yet".to_string(),
            },
        ));
    }
    let first = stops.first().and_then(|stop| stop.file.as_deref());
    if let Some(held) = &last {
        if local_in_effect {
            say(Diagnostic::info(
                codes::I_MANAGED_SOURCE,
                "managed profile: the local file was changed after the last update; it is in effect until the next one",
            ));
        } else {
            say(Diagnostic::info(
                codes::I_MANAGED_SOURCE,
                format!(
                    "managed profile: the downloaded copy is in effect (`{}`)",
                    held.copy.file.display()
                ),
            ));
            if stops.len() > 1
                && let Some(first) = first
            {
                say(Diagnostic::info(
                    codes::I_MANAGED_SOURCE,
                    format!(
                        "managed profile: the address in the local file leads to another one (see the #!MANAGED-CONFIG line of `{}`); the local file still names the old address",
                        first.display()
                    ),
                ));
            }
            match end {
                End::NoLine => say(Diagnostic::warning(
                    codes::W_MANAGED_NOT_UPDATED,
                    "the downloaded managed profile has no #!MANAGED-CONFIG line; it is no longer updated",
                )
                .with_hint(
                    "to keep it updated, the profile the address serves has to carry a #!MANAGED-CONFIG line",
                )),
                End::NoWayOut => say(Diagnostic::warning(
                    codes::W_MANAGED_NOT_UPDATED,
                    "the #!MANAGED-CONFIG lines of the downloaded profiles lead through too many addresses; the last copy found is in effect",
                )
                .with_hint(
                    "write the address the profile is finally served from into the #!MANAGED-CONFIG line of the local file",
                )),
                End::NotFound(_) | End::Found => {}
            }
        }
    }

    // The profile is updated from the last address the walk was at, by the
    // line that names it: the local file's while that is in effect, the last
    // copy's own otherwise (it names its own address, or the one that has no
    // copy yet, or leads nowhere). A copy with no line names none, and then
    // the addresses are stops on the way and nothing more.
    let updated = local_in_effect || !matches!(end, End::NoLine);
    let line = last
        .as_ref()
        .filter(|_| !local_in_effect)
        .and_then(|held| held.line.as_ref())
        .unwrap_or(local);
    let (interval, strict) = (line.config.interval, line.config.strict);
    let count = stops.len();
    let remote = stops
        .into_iter()
        .enumerate()
        .map(|(at, stop)| {
            let managed = updated && at + 1 == count;
            RemoteRef {
                url: stop.address,
                role: if managed {
                    RemoteRole::Managed
                } else {
                    RemoteRole::ManagedHop
                },
                path: stop.file,
                interval: if managed { interval } else { DEFAULT_INTERVAL },
                strict: managed && strict,
                lacks_section: false,
            }
        })
        .collect();
    Choice {
        copy: last.filter(|_| !local_in_effect).map(|held| held.copy),
        remote,
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{Origin, parse_str};
    use std::path::Path;
    use std::sync::Arc;

    fn header(text: &str) -> (Option<ManagedConfig>, Diagnostics) {
        let (p, mut d) = parse_str(text, Arc::from(Path::new("m.conf")), Origin::Main);
        let m = parse_managed(&p.header, &mut d);
        (m, d)
    }

    const ADDRESS: &str = "https://x.test/y";

    /// The line of a local file, which the tests of the choice hand over.
    fn local_line() -> Line {
        let (p, _) = parse_str(
            &format!("#!MANAGED-CONFIG {ADDRESS}\n[Rule]\nFINAL,REJECT\n"),
            Arc::from(Path::new("m.conf")),
            Origin::Main,
        );
        find(&p.header, &mut Diagnostics::default()).expect("a line")
    }

    /// `n` hours after one well in the past.
    fn hours(n: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + n * 3600)
    }

    fn downloaded_at(fetched: SystemTime) -> crate::remote::fake::Fake {
        crate::remote::fake::Fake::new()
            .text(ADDRESS, "[Rule]\nFINAL,DIRECT\n")
            .fetched(ADDRESS, fetched)
    }

    /// The local file wins only when it is provably newer than the copy: a
    /// file whose time cannot be read is not, so the choice cannot freeze the
    /// updates of a profile whose file cannot be asked.
    #[test]
    fn a_local_file_whose_time_cannot_be_read_does_not_keep_the_copy_out() {
        let fake = downloaded_at(hours(5));
        let local = local_line();
        assert!(choose(&local, true, None, &fake).copy.is_some());
        assert!(choose(&local, true, Some(hours(4)), &fake).copy.is_some());
        assert!(choose(&local, true, Some(hours(10)), &fake).copy.is_none());
    }

    /// The copy is read as a profile of its own: spans name the file that holds
    /// it and its lines carry it as their origin, which says it is the
    /// managed profile's copy and not a text some `#!include` brought in.
    #[test]
    fn the_lines_of_a_copy_carry_the_copy_as_their_origin() {
        let fake = downloaded_at(hours(5));
        let choice = choose(&local_line(), true, Some(hours(1)), &fake);
        let copy = choice.copy.expect("the copy is in effect");
        assert_eq!(copy.file, fake.path_of(ADDRESS));
        let entry = &copy.profile.section("Rule").unwrap().entries[0];
        assert_eq!(
            entry.origin,
            Origin::Managed(Arc::from(copy.file.as_path()))
        );
        assert_eq!(entry.span.file.as_ref(), copy.file.as_path());
    }

    #[test]
    fn managed_directive() {
        let (m, d) = header(
            "#!MANAGED-CONFIG http://test.com/surge.conf interval=60 strict=true\n#!FORBIDDEN-AUTO-UPGRADE smart-group\n[General]\n",
        );
        assert!(d.is_empty(), "{:?}", d.into_vec());
        let m = m.unwrap();
        assert_eq!(m.url, "http://test.com/surge.conf");
        assert_eq!(m.interval, Duration::from_secs(60));
        assert!(m.strict);
        let (m, _) = header("#!MANAGED-CONFIG https://x/y\n[General]\n");
        let m = m.unwrap();
        assert_eq!(m.interval, Duration::from_secs(86400));
        assert!(!m.strict);
        let (m, d) = header("#!SOMETHING-ELSE\n[General]\n");
        assert!(m.is_none());
        assert_eq!(d.iter().next().unwrap().code, codes::W_UNKNOWN_DIRECTIVE);
    }

    fn messages(d: Diagnostics) -> Vec<String> {
        d.into_vec().into_iter().map(|d| d.message).collect()
    }

    /// What the line makes of the interval it is given, and what it says.
    fn interval_of(written: &str) -> (u64, Vec<String>) {
        let (m, d) = header(&format!(
            "#!MANAGED-CONFIG https://x.test/y{written}\n[General]\n"
        ));
        (m.unwrap().interval.as_secs(), messages(d))
    }

    /// An update every few seconds would hammer a server: 60 seconds is the
    /// shortest, and the warning stands at the line.
    #[test]
    fn an_interval_below_sixty_seconds_is_sixty() {
        let floor = "interval below 60 seconds; 60 is used";
        for written in [" interval=1", " interval=59"] {
            assert_eq!(interval_of(written), (60, vec![floor.to_string()]));
        }
        let (_, d) = header("#!MANAGED-CONFIG https://x.test/y interval=59\n[General]\n");
        let d = d.into_vec();
        assert_eq!(d[0].code, codes::W_INVALID_VALUE);
        assert_eq!(d[0].span.as_ref().map(|span| span.line), Some(1));
    }

    #[test]
    fn sixty_seconds_and_more_are_taken_as_written() {
        for (written, seconds) in [
            (" interval=60", 60),
            (" interval=61", 61),
            (" interval=3600", 3600),
        ] {
            assert_eq!(interval_of(written), (seconds, vec![]), "{written}");
        }
    }

    /// 0 says nothing: the line is read as if `interval` were not written,
    /// which leaves a number written before it alone.
    #[test]
    fn an_interval_of_zero_reads_as_not_written() {
        for (written, seconds) in [
            ("", 86400),
            (" interval=0", 86400),
            (" interval=3600 interval=0", 3600),
        ] {
            assert_eq!(interval_of(written), (seconds, vec![]), "{written}");
        }
    }

    /// Only a link to a server can be downloaded from. A line whose URL is
    /// something else is no managed profile, and its text, which can be a
    /// token, is not quoted. An earlier line that is fine stays in force.
    #[test]
    fn a_url_that_is_not_http_or_https_is_no_managed_profile() {
        let said = "the #!MANAGED-CONFIG URL is not an http(s) URL; the line is ignored";
        for url in [
            "ftp://example.com/S3CR3T.conf",
            "example.com/S3CR3T.conf",
            "file:///S3CR3T/profile.conf",
            "//example.com/S3CR3T.conf",
            "S3CR3T",
            "https://",
        ] {
            let (m, d) = header(&format!(
                "#!MANAGED-CONFIG {url} interval=3600\n[General]\n"
            ));
            assert!(m.is_none(), "{url}");
            assert_eq!(messages(d), [said], "{url}");
        }
        let (m, d) = header(
            "#!MANAGED-CONFIG https://x.test/y interval=600\n#!MANAGED-CONFIG ftp://x.test/z\n[General]\n",
        );
        let m = m.expect("the first line stays in force");
        assert_eq!(m.url, "https://x.test/y");
        assert_eq!(messages(d), [said]);
    }

    /// A directive in the header is named as one in a section is: by its
    /// run of letters, digits, `-` and `_`, if that can be a name.
    #[test]
    fn an_unknown_directive_is_named_only_when_that_can_be_a_name() {
        let longest = "A".repeat(32);
        let too_long = "A".repeat(33);
        let (m, d) = header(&format!(
            "#!{longest} x\n#!{too_long} x\n#!MANAGED_CONFIG=https://x.test/S3CR3T\n#! MANAGED-CONFIG https://x.test/S3CR3T\n[General]\n"
        ));
        assert!(m.is_none());
        assert_eq!(
            messages(d),
            [
                format!("unknown directive ignored: #!{longest}"),
                "unknown directive ignored".to_string(),
                "unknown directive ignored: #!MANAGED_CONFIG".to_string(),
                "unknown directive ignored: #!".to_string(),
            ]
        );
    }

    /// The value of a parameter the line knows is quoted when it is what
    /// somebody could have meant: a number, a word.
    #[test]
    fn an_invalid_interval_or_strictness_is_quoted_only_when_it_is_plain() {
        let (_, d) = header(
            "#!MANAGED-CONFIG https://x.test/y interval=99999999999999999999 interval=1h interval=60https://x.test/S3CR3T strict=maybe strict=1https://x.test/S3CR3T strict=\n[General]\n",
        );
        assert_eq!(
            messages(d),
            [
                "invalid interval `99999999999999999999`",
                "invalid interval",
                "invalid interval",
                "invalid strict `maybe`",
                "invalid strict",
                "invalid strict",
            ]
        );
    }

    /// What a word the line does not know is called: its name, when it is
    /// a lower-case word like the ones the format has (`interval`, `strict`);
    /// a token, a Base64 string whose padding is an `=` or a link pasted twice
    /// is not.
    #[test]
    fn an_unknown_parameter_is_named_only_when_it_is_a_lower_case_word() {
        let longest = "a".repeat(32);
        let too_long = "a".repeat(33);
        let (m, d) = header(&format!(
            "#!MANAGED-CONFIG https://x.test/y foo=1 foo-bar=2 {longest}=3 {too_long}=4 Foo=5 foo1=6 foo_bar=7 aGVsbG8= =8 bare https://x.test/S3CR3T?a=b token=S3CR3T\n[General]\n"
        ));
        assert!(m.is_some());
        let anonymous =
            "unknown MANAGED-CONFIG parameter (not written as a lower-case `name=value`)";
        assert_eq!(
            messages(d),
            [
                "unknown MANAGED-CONFIG parameter `foo`".to_string(),
                "unknown MANAGED-CONFIG parameter `foo-bar`".to_string(),
                format!("unknown MANAGED-CONFIG parameter `{longest}`"),
                anonymous.to_string(),
                anonymous.to_string(),
                anonymous.to_string(),
                anonymous.to_string(),
                anonymous.to_string(),
                anonymous.to_string(),
                anonymous.to_string(),
                anonymous.to_string(),
                "unknown MANAGED-CONFIG parameter `token`".to_string(),
            ]
        );
    }
}
