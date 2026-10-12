//! `#!include` expansion. A local file is read from disk; the text of a remote
//! include is asked of a `RemoteSource`, which answers from what was
//! downloaded earlier: the expansion never goes to the network.

use super::{Entry, Origin, Profile, Section, parse_str};
use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::managed::parse_managed;
use crate::reach::{Reach, TakenIn};
use crate::remote::{DEFAULT_INTERVAL, Remote, RemoteRef, RemoteRole, RemoteSource};
use crate::span::Span;
use crate::value::{split_list, starts_with_ci};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

/// The most a local file that the downloaded copy of a managed profile takes
/// in may hold: what rurge downloads for one resource at most.
const MOST_A_COPY_TAKES_IN: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct IncludeOptions {
    pub base_dir: PathBuf,
    pub max_depth: usize,
    /// Total number of `#!include` targets that may be expanded across the
    /// whole call to `expand()`, bounding breadth (not just depth): a
    /// non-cyclic target included many times per level would otherwise
    /// expand as `branching^depth`.
    pub max_files: usize,
}

impl Default for IncludeOptions {
    fn default() -> Self {
        Self {
            base_dir: PathBuf::from("."),
            max_depth: 8,
            max_files: 200,
        }
    }
}

fn is_url(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

fn wildcard_prefix(section_name: &str) -> Option<&str> {
    let trimmed = section_name.trim_end();
    let prefix = trimmed.strip_suffix('*')?;
    (prefix.ends_with(' ')).then_some(prefix)
}

/// What the expansion is inside of: a file by its canonical path, a
/// downloaded text by its URL.
#[derive(PartialEq)]
enum Visiting {
    File(PathBuf),
    Remote(Url),
}

/// The text one `#!include` target named, with its own includes expanded.
struct Included {
    profile: Profile,
    /// The file the text is in, as a warning about it names it.
    file: PathBuf,
    /// Where the text stands among the remote files the expansion has come
    /// across, when it is a downloaded one.
    tracked: Option<usize>,
}

/// The remote files one expansion has come across.
#[derive(Default)]
struct Remotes {
    /// One per URL, in the order they were first asked for.
    refs: Vec<RemoteRef>,
    /// What the source said, so that a URL named twice is one text.
    answers: HashMap<Url, Remote>,
}

impl Remotes {
    /// What the source has for `url`, asked once per expansion. The text is
    /// kept here and lent: a URL named many times is not copied each time.
    fn ask(&mut self, source: &dyn RemoteSource, url: &Url) -> &Remote {
        self.answers
            .entry(url.clone())
            .or_insert_with(|| source.get(url))
    }

    /// The place of `url` among the files, which is added when it is new.
    fn track(&mut self, url: &Url, path: Option<PathBuf>, interval: Duration) -> usize {
        if let Some(at) = self.refs.iter().position(|r| &r.url == url) {
            return at;
        }
        self.refs.push(RemoteRef {
            url: url.clone(),
            role: RemoteRole::Include,
            path,
            interval,
            strict: false,
            lacks_section: false,
        });
        self.refs.len() - 1
    }
}

struct Expansion<'a> {
    opts: &'a IncludeOptions,
    remote: Option<&'a dyn RemoteSource>,
    diags: &'a mut Diagnostics,
    stack: Vec<Visiting>,
    /// Targets expanded so far, over the whole call.
    count: usize,
    remotes: Remotes,
}

/// Expands the `#!include` lines of `profile` in place, and gives back the
/// remote files it came across.
pub fn expand(
    profile: &mut Profile,
    opts: &IncludeOptions,
    remote: Option<&dyn RemoteSource>,
    diags: &mut Diagnostics,
) -> Vec<RemoteRef> {
    let mut run = Expansion {
        opts,
        remote,
        diags,
        stack: Vec::new(),
        count: 0,
        remotes: Remotes::default(),
    };
    if let Some(main) = &profile.main {
        run.stack.push(Visiting::File(canonical(main)));
    }
    run.expand(profile, 0);
    run.remotes.refs
}

fn canonical(p: &Path) -> PathBuf {
    #[cfg(test)]
    tests::looked_at(p);
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// The text of the file at `path`, which has to be an ordinary file of at most
/// `bound` bytes. A path below the profile's directory can name a device or a
/// FIFO, and opening one would read for ever or wait for a writer, so the
/// kind of the file is asked before it is opened. What is read is bounded
/// whatever the length of the file is said to be: a file can report none and
/// still give data.
fn read_ordinary_file(path: &Path, bound: u64) -> io::Result<String> {
    if !fs::metadata(path)?.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(bound.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > bound {
        return Err(io::Error::other(format!(
            "larger than {}",
            size_text(bound)
        )));
    }
    String::from_utf8(bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "stream did not contain valid UTF-8",
        )
    })
}

/// A size as a message says it: in MiB when it is a whole number of them.
fn size_text(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if bytes % MIB == 0 {
        format!("{} MiB", bytes / MIB)
    } else {
        format!("{bytes} bytes")
    }
}

impl Expansion<'_> {
    /// What an `#!include` line may take in is up to the text the line is
    /// written in: the reach of its origin.
    fn expand(&mut self, profile: &mut Profile, depth: usize) {
        let mut appended: Vec<Section> = Vec::new();
        let mut remove_wildcards: Vec<usize> = Vec::new();

        for (idx, section) in profile.sections.iter_mut().enumerate() {
            let wildcard = wildcard_prefix(&section.name).map(str::to_string);
            let mut new_entries: Vec<Entry> = Vec::new();
            for entry in std::mem::take(&mut section.entries) {
                let Some(rest) = entry.raw.strip_prefix("#!include") else {
                    new_entries.push(entry);
                    continue;
                };
                let reach = entry.origin.reach();
                for target in split_list(rest) {
                    let Some(mut included) = self.take_in(&target, &entry.span, depth, reach)
                    else {
                        continue;
                    };
                    if let Some(prefix) = &wildcard {
                        let (matched, _): (Vec<Section>, Vec<Section>) = included
                            .profile
                            .sections
                            .into_iter()
                            .partition(|s| starts_with_ci(&s.name, prefix));
                        appended.extend(matched);
                    } else if let Some(s) = included.profile.section_mut(&section.name) {
                        new_entries.append(&mut s.entries);
                    } else {
                        let mut missing = Diagnostic::warning(
                            codes::W_INCLUDE_SECTION_MISSING,
                            format!(
                                "`{}` has no [{}] section",
                                included.file.display(),
                                section.name
                            ),
                        )
                        .at(entry.span.clone());
                        if let Some(at) = included.tracked {
                            // nobody can add the section to a downloaded file
                            missing = missing.with_hint(
                                "check that the address serves a profile with that section, or move the `#!include` line to a section it has",
                            );
                            self.remotes.refs[at].lacks_section = true;
                        }
                        self.diags.push(missing);
                    }
                }
            }
            section.entries = new_entries;
            if wildcard.is_some() {
                remove_wildcards.push(idx);
            }
        }

        for idx in remove_wildcards.into_iter().rev() {
            profile.sections.remove(idx);
        }
        profile.sections.extend(appended);
    }

    /// The text `target` names, expanded in turn, when the line it is written
    /// in (of the given `reach`) may take it in. `None` when there is nothing
    /// to take in; a diagnostic says why.
    fn take_in(
        &mut self,
        target: &str,
        span: &Span,
        depth: usize,
        reach: Reach,
    ) -> Option<Included> {
        if is_url(target) {
            return self.take_in_remote(target, span, depth);
        }
        // the first thing done with a path: nothing of this machine is
        // joined, resolved, opened or looked at for one the text may not name
        match reach.include(target) {
            Ok(how) => self.take_in_local(target, span, depth, how),
            Err(refusal) => {
                self.diags.push(
                    Diagnostic::warning(codes::W_REMOTE_LOCAL_ACCESS, refusal.message)
                        .with_hint(refusal.hint)
                        .at(span.clone()),
                );
                None
            }
        }
    }

    /// Whether the include may go on: it does not lead back to where it is
    /// in, is not nested too deep and is not one too many. `shown` is what a
    /// message calls it.
    fn admit(&mut self, key: &Visiting, shown: &str, span: &Span, depth: usize) -> bool {
        if self.stack.contains(key) {
            let message = match key {
                Visiting::File(_) => format!("include cycle: {shown}"),
                Visiting::Remote(_) => "include cycle through a remote include".to_string(),
            };
            self.diags
                .push(Diagnostic::error(codes::E_INCLUDE_CYCLE, message).at(span.clone()));
            return false;
        }
        if depth >= self.opts.max_depth {
            self.diags.push(
                Diagnostic::error(
                    codes::E_INCLUDE_CYCLE,
                    format!(
                        "include nesting deeper than {}: {shown}",
                        self.opts.max_depth
                    ),
                )
                .at(span.clone()),
            );
            return false;
        }
        if self.count >= self.opts.max_files {
            // Only the expansion that first crosses the limit reports
            // it; every further skip in this or any later section
            // just keeps incrementing past `max_files + 1` in silence.
            self.count += 1;
            if self.count == self.opts.max_files + 1 {
                self.diags.push(
                    Diagnostic::error(
                        codes::E_INCLUDE_CYCLE,
                        format!(
                            "include expansion limit of {} files exceeded",
                            self.opts.max_files
                        ),
                    )
                    .at(span.clone()),
                );
            }
            return false;
        }
        self.count += 1;
        true
    }

    /// A local file the line may include, taken in as `how` says: a file of
    /// the downloaded copy of a managed profile, or of a file it took in, must
    /// be an ordinary one of bounded size, and its lines are read as the
    /// copy's are and not as the user's own.
    fn take_in_local(
        &mut self,
        target: &str,
        span: &Span,
        depth: usize,
        how: TakenIn,
    ) -> Option<Included> {
        let path = if Path::new(target).is_absolute() {
            PathBuf::from(target)
        } else {
            self.opts.base_dir.join(target)
        };
        let key = Visiting::File(canonical(&path));
        if !self.admit(&key, &path.display().to_string(), span, depth) {
            return None;
        }
        let read = match how {
            TakenIn::AsOwnText => fs::read_to_string(&path),
            TakenIn::AsTheCopy => read_ordinary_file(&path, MOST_A_COPY_TAKES_IN),
        };
        let text = match read {
            Ok(t) => t,
            Err(e) => {
                self.diags.push(
                    Diagnostic::error(
                        codes::E_INCLUDE_NOT_FOUND,
                        format!("cannot read include `{}`: {e}", path.display()),
                    )
                    .at(span.clone()),
                );
                return None;
            }
        };
        let file: Arc<Path> = Arc::from(path.as_path());
        let origin = match how {
            TakenIn::AsOwnText => Origin::Include(file.clone()),
            TakenIn::AsTheCopy => Origin::ManagedInclude(file.clone()),
        };
        let (mut profile, sub_diags) = parse_str(&text, file.clone(), origin);
        self.diags.extend(sub_diags);
        self.stack.push(key);
        self.expand(&mut profile, depth + 1);
        self.stack.pop();
        Some(Included {
            profile,
            file: path,
            tracked: None,
        })
    }

    fn take_in_remote(&mut self, target: &str, span: &Span, depth: usize) -> Option<Included> {
        // the warning quotes nothing of the target: its path or its query can
        // be a token
        let Ok(mut url) = Url::parse(target) else {
            self.diags.push(
                Diagnostic::warning(
                    codes::W_INVALID_VALUE,
                    "the target of a remote include is not a valid URL",
                )
                .at(span.clone()),
            );
            return None;
        };
        // a fragment is never sent to a server: links that differ only in it
        // are one link
        url.set_fragment(None);
        let Some(source) = self.remote else {
            self.diags.push(
                Diagnostic::warning(
                    codes::W_REMOTE_INCLUDE_NOT_READ,
                    "remote include not read: no data directory",
                )
                .at(span.clone()),
            );
            return None;
        };
        let key = Visiting::Remote(url.clone());
        if !self.admit(&key, "a remote include", span, depth) {
            return None;
        }
        // The answer is lent by `self.remotes`, and the expansion below needs
        // `self` again: the text is parsed here, and what goes on is the
        // profile, its warnings and the path.
        let (mut profile, sub_diags, path) = match self.remotes.ask(source, &url) {
            Remote::Text { text, path, .. } => {
                let file: Arc<Path> = Arc::from(path.as_path());
                let (profile, sub_diags) = parse_str(text, file.clone(), Origin::Remote(file));
                (profile, sub_diags, path.clone())
            }
            Remote::Missing { why } => {
                let message = match why {
                    Some(why) => format!("remote include not downloaded yet: {why}"),
                    None => "remote include not downloaded yet".to_string(),
                };
                self.diags.push(
                    Diagnostic::warning(codes::W_RESOURCE_UNAVAILABLE, message).at(span.clone()),
                );
                self.remotes.track(&url, None, DEFAULT_INTERVAL);
                return None;
            }
        };
        self.diags.extend(sub_diags);
        // what is wrong in the line is not reported, as for a local include
        let interval = parse_managed(&profile.header, &mut Diagnostics::default())
            .map_or(DEFAULT_INTERVAL, |managed| managed.interval);
        let at = self.remotes.track(&url, Some(path.clone()), interval);
        self.stack.push(key);
        self.expand(&mut profile, depth + 1);
        self.stack.pop();
        Some(Included {
            profile,
            file: path,
            tracked: Some(at),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::fake::Fake;
    use crate::remote::{RemoteRef, RemoteRole, RemoteSource};
    use crate::text::{Origin, parse_str};
    use std::cell::RefCell;
    use std::fs;
    use std::sync::Arc;

    thread_local! {
        /// The paths the expansion of this thread asked the file system to
        /// resolve.
        static LOOKED_AT: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    }

    /// Notes that the expansion asked the file system about `p`.
    pub(super) fn looked_at(p: &Path) {
        LOOKED_AT.with(|log| log.borrow_mut().push(p.to_path_buf()));
    }

    /// The paths asked about since the last call.
    fn take_looked_at() -> Vec<PathBuf> {
        LOOKED_AT.with(|log| std::mem::take(&mut *log.borrow_mut()))
    }

    fn load(dir: &std::path::Path, main: &str) -> (Profile, Diagnostics) {
        let path = dir.join(main);
        let text = fs::read_to_string(&path).unwrap();
        let (mut p, mut d) = parse_str(&text, Arc::from(path.as_path()), Origin::Main);
        expand(
            &mut p,
            &IncludeOptions {
                base_dir: dir.to_path_buf(),
                max_depth: 8,
                max_files: 200,
            },
            None,
            &mut d,
        );
        (p, d)
    }

    #[test]
    fn single_multiple_and_mixed_includes() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.conf"), "[Proxy]\n#!include proxy.dconf\n[Rule]\n#!include a.dconf\nDEST-PORT,123,DIRECT\n#!include b.dconf, c.dconf\nFINAL,DIRECT\n").unwrap();
        fs::write(
            dir.path().join("proxy.dconf"),
            "[Proxy]\nP = direct\n[Rule]\nDOMAIN,ignored,DIRECT\n",
        )
        .unwrap();
        fs::write(dir.path().join("a.dconf"), "[Rule]\nDOMAIN,a,DIRECT\n").unwrap();
        fs::write(dir.path().join("b.dconf"), "[Rule]\nDOMAIN,b,DIRECT\n").unwrap();
        fs::write(dir.path().join("c.dconf"), "[Rule]\nDOMAIN,c,DIRECT\n").unwrap();
        let (p, d) = load(dir.path(), "main.conf");
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert_eq!(p.section("Proxy").unwrap().entries[0].raw, "P = direct");
        let rules: Vec<_> = p
            .section("Rule")
            .unwrap()
            .entries
            .iter()
            .map(|e| e.raw.as_str())
            .collect();
        assert_eq!(
            rules,
            [
                "DOMAIN,a,DIRECT",
                "DEST-PORT,123,DIRECT",
                "DOMAIN,b,DIRECT",
                "DOMAIN,c,DIRECT",
                "FINAL,DIRECT"
            ]
        );
        let e = &p.section("Rule").unwrap().entries[0];
        assert!(matches!(&e.origin, Origin::Include(path) if path.ends_with("a.dconf")));
        assert_eq!(e.span.line, 2);
    }

    #[test]
    fn wildcard_named_sections() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("main.conf"),
            "[Ruleset *]\n#!include shared.conf\n[Rule]\nRULE-SET,Streaming,DIRECT\nFINAL,DIRECT\n",
        )
        .unwrap();
        fs::write(dir.path().join("shared.conf"), "[Ruleset Streaming]\nDOMAIN-SUFFIX,netflix.com\n[Ruleset Music]\nDOMAIN-SUFFIX,spotify.com\n[General]\nipv6 = true\n").unwrap();
        let (p, d) = load(dir.path(), "main.conf");
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert!(p.section("Ruleset *").is_none());
        let names: Vec<_> = p
            .sections_with_prefix("Ruleset ")
            .map(|s| s.name.clone())
            .collect();
        assert_eq!(names, ["Ruleset Streaming", "Ruleset Music"]);
        assert!(p.section("General").is_none());
    }

    /// The three kinds of named section the manual lists each take in the
    /// sections of their own kind from the profile they name, and no others.
    #[test]
    fn a_wildcard_takes_the_named_sections_of_its_own_kind_only() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("shared.conf"),
            "[Ruleset Sets]\nDOMAIN,a.example\n[WireGuard Home]\nmtu = 1280\n[Tailscale Net]\nauth-key = k\n[General]\nipv6 = true\n",
        )
        .unwrap();
        for (kind, taken) in [
            ("Ruleset", "Ruleset Sets"),
            ("WireGuard", "WireGuard Home"),
            ("Tailscale", "Tailscale Net"),
        ] {
            fs::write(
                dir.path().join("main.conf"),
                format!("[{kind} *]\n#!include shared.conf\n[Rule]\nFINAL,DIRECT\n"),
            )
            .unwrap();
            let (p, d) = load(dir.path(), "main.conf");
            assert!(d.is_empty(), "{kind}: {:?}", d.into_vec());
            let names: Vec<&str> = p
                .sections
                .iter()
                .map(|s| s.name.as_str())
                .filter(|name| *name != "Rule")
                .collect();
            assert_eq!(names, [taken], "{kind}");
        }
    }

    #[test]
    fn errors_and_warnings() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.conf"), "[Rule]\n#!include missing.dconf\n#!include https://example.com/x.conf\n#!include nosec.dconf\n#!include loop.dconf\nFINAL,DIRECT\n").unwrap();
        fs::write(dir.path().join("nosec.dconf"), "[Proxy]\nP = direct\n").unwrap();
        fs::write(
            dir.path().join("loop.dconf"),
            "[Rule]\n#!include loop.dconf\n",
        )
        .unwrap();
        let (p, d) = load(dir.path(), "main.conf");
        let codes_seen: Vec<_> = d.iter().map(|x| x.code).collect();
        assert!(codes_seen.contains(&codes::E_INCLUDE_NOT_FOUND));
        assert!(codes_seen.contains(&codes::W_REMOTE_INCLUDE_NOT_READ));
        assert!(codes_seen.contains(&codes::W_INCLUDE_SECTION_MISSING));
        assert!(codes_seen.contains(&codes::E_INCLUDE_CYCLE));
        assert_eq!(
            p.section("Rule").unwrap().entries.last().unwrap().raw,
            "FINAL,DIRECT"
        );
    }

    /// The link of a remote include can carry a token: the warning says what
    /// was not read and why, and the span says where.
    #[test]
    fn a_remote_include_does_not_quote_its_target() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("main.conf"),
            "[Rule]\n#!include https://example.com/S3CR3T/x.conf?token=S3CR3T\nFINAL,DIRECT\n",
        )
        .unwrap();
        let (_, d) = load(dir.path(), "main.conf");
        let found: Vec<_> = d.iter().collect();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, codes::W_REMOTE_INCLUDE_NOT_READ);
        assert_eq!(
            found[0].message,
            "remote include not read: no data directory"
        );
        assert_eq!(found[0].span.as_ref().unwrap().line, 2);
    }

    #[test]
    fn breadth_expansion_is_bounded_by_max_files() {
        // l0 includes l1 six times, l1 includes l2 six times, l2 includes l3
        // six times; none of this is cyclic and none of it exceeds max_depth,
        // so without a total-expansion limit this would expand as 6^depth.
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("l3.conf"), "[Rule]\nDOMAIN,leaf,DIRECT\n").unwrap();
        let includes_l3: String = "#!include l3.conf\n".repeat(6);
        fs::write(dir.path().join("l2.conf"), format!("[Rule]\n{includes_l3}")).unwrap();
        let includes_l2: String = "#!include l2.conf\n".repeat(6);
        fs::write(dir.path().join("l1.conf"), format!("[Rule]\n{includes_l2}")).unwrap();
        let includes_l1: String = "#!include l1.conf\n".repeat(6);
        fs::write(
            dir.path().join("l0.conf"),
            format!("[Rule]\n{includes_l1}FINAL,DIRECT\n"),
        )
        .unwrap();

        let path = dir.path().join("l0.conf");
        let text = fs::read_to_string(&path).unwrap();
        let (mut p, mut d) = parse_str(&text, Arc::from(path.as_path()), Origin::Main);
        expand(
            &mut p,
            &IncludeOptions {
                base_dir: dir.path().to_path_buf(),
                max_depth: 8,
                max_files: 50,
            },
            None,
            &mut d,
        );
        let limit_diags: Vec<_> = d
            .iter()
            .filter(|x| x.code == codes::E_INCLUDE_CYCLE)
            .collect();
        assert_eq!(limit_diags.len(), 1, "{:?}", d.clone().into_vec());
        assert!(
            limit_diags[0]
                .message
                .contains("include expansion limit of 50 files exceeded")
        );
        let rule_count = p.section("Rule").unwrap().entries.len();
        assert!(rule_count <= 50 * 6 + 1, "entries: {rule_count}");
    }

    #[test]
    fn starts_with_ci_handles_non_ascii_without_panicking() {
        assert!(!starts_with_ci("中文中文中文", "WireGuard "));
        assert!(starts_with_ci("Ruleset 流媒体", "Ruleset "));
    }

    /// Expands `text` as the main profile in `dir`, within the limits given;
    /// the remote includes in it are answered by `source`, when there is one.
    fn expand_limited(
        dir: &Path,
        text: &str,
        source: Option<&Fake>,
        max_depth: usize,
        max_files: usize,
    ) -> (Profile, Diagnostics, Vec<RemoteRef>) {
        let main: Arc<Path> = Arc::from(dir.join("main.conf").as_path());
        let (mut p, mut d) = parse_str(text, main, Origin::Main);
        let remote = expand(
            &mut p,
            &IncludeOptions {
                base_dir: dir.to_path_buf(),
                max_depth,
                max_files,
            },
            source.map(|s| s as &dyn RemoteSource),
            &mut d,
        );
        (p, d, remote)
    }

    fn expand_in(
        dir: &Path,
        text: &str,
        source: Option<&Fake>,
    ) -> (Profile, Diagnostics, Vec<RemoteRef>) {
        expand_limited(dir, text, source, 8, 200)
    }

    /// `expand_in` for a profile that includes no local file.
    fn expand_text(text: &str, source: Option<&Fake>) -> (Profile, Diagnostics, Vec<RemoteRef>) {
        expand_in(Path::new("/profiles"), text, source)
    }

    fn raws<'a>(p: &'a Profile, section: &str) -> Vec<&'a str> {
        p.section(section)
            .unwrap()
            .entries
            .iter()
            .map(|e| e.raw.as_str())
            .collect()
    }

    /// What the diagnostics are, as (code, message, line).
    fn told(d: &Diagnostics) -> Vec<(&str, &str, Option<u32>)> {
        d.iter()
            .map(|x| (x.code, x.message.as_str(), x.span.as_ref().map(|s| s.line)))
            .collect()
    }

    #[test]
    fn a_remote_include_is_expanded_in_place() {
        let fake = Fake::new().text(
            "https://example.com/rules.conf",
            "[Rule]\nDOMAIN,a.example,DIRECT\nDOMAIN,b.example,DIRECT\n",
        );
        let (p, d, remote) = expand_text(
            "[Rule]\nDOMAIN,before.example,DIRECT\n#!include https://example.com/rules.conf\nDOMAIN,after.example,DIRECT\nFINAL,DIRECT\n",
            Some(&fake),
        );
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert_eq!(
            raws(&p, "Rule"),
            [
                "DOMAIN,before.example,DIRECT",
                "DOMAIN,a.example,DIRECT",
                "DOMAIN,b.example,DIRECT",
                "DOMAIN,after.example,DIRECT",
                "FINAL,DIRECT"
            ]
        );
        // a line of the downloaded text says so, and where it stands in its
        // own file
        let cached = fake.path_of("https://example.com/rules.conf");
        let entries = &p.section("Rule").unwrap().entries;
        assert_eq!(entries[0].origin, Origin::Main);
        assert_eq!(
            entries[1].origin,
            Origin::Remote(Arc::from(cached.as_path()))
        );
        assert_eq!(entries[1].span.file.as_ref(), cached.as_path());
        assert_eq!(entries[1].span.line, 2);
        assert_eq!(entries[2].span.line, 3);
        // and the load tracks where it came from
        assert_eq!(remote.len(), 1);
        assert_eq!(remote[0].url.as_str(), "https://example.com/rules.conf");
        assert_eq!(remote[0].role, RemoteRole::Include);
        assert_eq!(remote[0].path.as_deref(), Some(cached.as_path()));
        assert!(remote[0].found());
    }

    #[test]
    fn a_remote_include_fills_a_proxy_section() {
        let fake = Fake::new().text(
            "https://example.com/proxies.conf",
            "[Proxy]\nA = direct\nB = reject\n[Rule]\nDOMAIN,not-taken.example,DIRECT\n",
        );
        let (p, d, _) = expand_text(
            "[Proxy]\n#!include https://example.com/proxies.conf\nLocal = direct\n[Rule]\nFINAL,DIRECT\n",
            Some(&fake),
        );
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert_eq!(
            raws(&p, "Proxy"),
            ["A = direct", "B = reject", "Local = direct"]
        );
        assert_eq!(raws(&p, "Rule"), ["FINAL,DIRECT"]);
    }

    #[test]
    fn a_remote_include_in_a_wildcard_section_brings_the_named_sections() {
        let fake = Fake::new().text(
            "https://example.com/sets.conf",
            "[Ruleset Streaming]\nDOMAIN-SUFFIX,netflix.com\n[Ruleset Music]\nDOMAIN-SUFFIX,spotify.com\n[General]\nipv6 = true\n",
        );
        let (p, d, remote) = expand_text(
            "[Ruleset *]\n#!include https://example.com/sets.conf\n[Rule]\nRULE-SET,Streaming,DIRECT\nFINAL,DIRECT\n",
            Some(&fake),
        );
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert!(p.section("Ruleset *").is_none());
        let names: Vec<_> = p
            .sections_with_prefix("Ruleset ")
            .map(|s| s.name.clone())
            .collect();
        assert_eq!(names, ["Ruleset Streaming", "Ruleset Music"]);
        assert!(p.section("General").is_none());
        let cached = fake.path_of("https://example.com/sets.conf");
        assert_eq!(
            p.section("Ruleset Music").unwrap().entries[0].origin,
            Origin::Remote(Arc::from(cached.as_path()))
        );
        assert_eq!(remote.len(), 1);
        assert!(!remote[0].lacks_section);
    }

    #[test]
    fn a_remote_include_that_was_not_downloaded_says_so() {
        let fake = Fake::new()
            .missing("https://example.com/never.conf", None)
            .missing(
                "https://example.com/failed.conf",
                Some("connection refused"),
            );
        let (p, d, remote) = expand_text(
            "[Rule]
#!include https://example.com/never.conf
#!include https://example.com/failed.conf
#!include https://example.com/unknown.conf
FINAL,DIRECT
",
            Some(&fake),
        );
        let found = told(&d);
        assert_eq!(
            found,
            [
                (
                    codes::W_RESOURCE_UNAVAILABLE,
                    "remote include not downloaded yet",
                    Some(2)
                ),
                (
                    codes::W_RESOURCE_UNAVAILABLE,
                    "remote include not downloaded yet: connection refused",
                    Some(3)
                ),
                (
                    codes::W_RESOURCE_UNAVAILABLE,
                    "remote include not downloaded yet: the fake has no answer for it",
                    Some(4)
                ),
            ]
        );
        // nothing was taken in, and each is tracked to be downloaded
        assert_eq!(raws(&p, "Rule"), ["FINAL,DIRECT"]);
        assert_eq!(remote.len(), 3);
        for r in &remote {
            assert!(!r.found());
            assert_eq!(r.path, None);
            assert_eq!(r.role, RemoteRole::Include);
            assert_eq!(r.interval, Duration::from_secs(86400));
            assert!(!r.strict && !r.lacks_section);
        }
    }

    #[test]
    fn without_a_source_a_remote_include_says_so() {
        let (p, d, remote) = expand_text(
            "[Rule]
#!include https://example.com/rules.conf
FINAL,DIRECT
",
            None,
        );
        let found = told(&d);
        assert_eq!(
            found,
            [(
                codes::W_REMOTE_INCLUDE_NOT_READ,
                "remote include not read: no data directory",
                Some(2)
            )]
        );
        assert_eq!(raws(&p, "Rule"), ["FINAL,DIRECT"]);
        assert!(remote.is_empty());
        // a target that is no link is that, with a source or without
        let (_, d, remote) = expand_text("[Rule]\n#!include http://\nFINAL,DIRECT\n", None);
        let codes_seen: Vec<&str> = d.iter().map(|x| x.code).collect();
        assert_eq!(codes_seen, [codes::W_INVALID_VALUE]);
        assert!(remote.is_empty());
    }

    /// Each target of a line is taken on its own: a link that is not one, or
    /// one that is not downloaded yet, does not stop the next. The warning
    /// about a link that is not one does not quote it.
    #[test]
    fn the_targets_of_a_line_are_taken_one_by_one() {
        let fake = Fake::new()
            .missing("https://example.com/gone.conf", None)
            .text(
                "https://example.com/ok.conf",
                "[Rule]
DOMAIN,ok.example,DIRECT
",
            );
        let (p, d, remote) = expand_text(
            "[Rule]
#!include http://S3CR3T@, https://example.com/gone.conf, https://example.com/ok.conf
FINAL,DIRECT
",
            Some(&fake),
        );
        let found = told(&d);
        assert_eq!(
            found,
            [
                (
                    codes::W_INVALID_VALUE,
                    "the target of a remote include is not a valid URL",
                    Some(2)
                ),
                (
                    codes::W_RESOURCE_UNAVAILABLE,
                    "remote include not downloaded yet",
                    Some(2)
                ),
            ]
        );
        assert_eq!(
            raws(&p, "Rule"),
            ["DOMAIN,ok.example,DIRECT", "FINAL,DIRECT"]
        );
        // the target that is no URL is not asked for and is not tracked
        let urls: Vec<&str> = remote.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://example.com/gone.conf",
                "https://example.com/ok.conf"
            ]
        );
        assert_eq!(fake.asked().len(), 2);
    }

    #[test]
    fn a_remote_text_can_include_another_remote_text() {
        let fake = Fake::new()
            .text(
                "https://example.com/a.conf",
                "[Rule]\nDOMAIN,a1.example,DIRECT\n#!include https://example.com/b.conf\nDOMAIN,a2.example,DIRECT\n",
            )
            .text(
                "https://example.com/b.conf",
                "[Rule]\nDOMAIN,b.example,DIRECT\n",
            );
        let (p, d, remote) = expand_text(
            "[Rule]\n#!include https://example.com/a.conf\nFINAL,DIRECT\n",
            Some(&fake),
        );
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert_eq!(
            raws(&p, "Rule"),
            [
                "DOMAIN,a1.example,DIRECT",
                "DOMAIN,b.example,DIRECT",
                "DOMAIN,a2.example,DIRECT",
                "FINAL,DIRECT"
            ]
        );
        // in the order the load came across them
        let urls: Vec<&str> = remote.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            urls,
            ["https://example.com/a.conf", "https://example.com/b.conf"]
        );
        // and a line says which file it is in
        let in_b = &p.section("Rule").unwrap().entries[1];
        assert_eq!(
            in_b.span.file.as_ref(),
            fake.path_of("https://example.com/b.conf").as_path()
        );
        assert_eq!(in_b.span.line, 2);
    }

    /// What a downloaded text says is not for it to decide on this machine:
    /// a path in it is not read, whether it is relative or absolute and
    /// whether the file is there or not. The refusal comes before any read: a
    /// file that is not there is refused, it is not one that cannot be read.
    #[test]
    fn a_remote_text_cannot_include_a_local_file() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("local.dconf");
        fs::write(&local, "[Rule]\nDOMAIN,local-only.example,DIRECT\n").unwrap();
        let downloaded = format!(
            "[Rule]\nDOMAIN,remote.example,DIRECT\n#!include local.dconf\n#!include {}\n#!include absent.dconf\n",
            local.display()
        );
        let fake = Fake::new().text("https://example.com/r.conf", &downloaded);
        let (p, d, _) = expand_in(
            dir.path(),
            "[Rule]\n#!include https://example.com/r.conf\nFINAL,DIRECT\n",
            Some(&fake),
        );
        let cached = fake.path_of("https://example.com/r.conf");
        assert_eq!(d.len(), 3, "{:?}", d.clone().into_vec());
        for (found, line) in d.iter().zip([3, 4, 5]) {
            assert_eq!(found.code, codes::W_REMOTE_LOCAL_ACCESS);
            assert_eq!(
                found.message,
                "a downloaded file cannot include a local file; the line is ignored"
            );
            assert_eq!(
                found.hint,
                Some("include the local file from the local profile instead")
            );
            let span = found.span.as_ref().expect("a position");
            assert_eq!((span.file.as_ref(), span.line), (cached.as_path(), line));
        }
        assert_eq!(
            raws(&p, "Rule"),
            ["DOMAIN,remote.example,DIRECT", "FINAL,DIRECT"]
        );
        // and no path was resolved on its account: the main file is the only
        // one the expansion looked at
        assert_eq!(take_looked_at(), [dir.path().join("main.conf")]);
    }

    /// Expands `text` as the downloaded copy of a managed profile, the way
    /// the loader does: its lines are `Origin::Managed`, and the local file
    /// (here `main.conf` in `dir`) is the profile whose directory the
    /// relative paths start from.
    fn expand_copy(dir: &Path, text: &str) -> (Profile, Diagnostics) {
        let main: Arc<Path> = Arc::from(dir.join("main.conf").as_path());
        let cached: Arc<Path> = Arc::from(Path::new("/cache/1.conf"));
        let (mut p, mut d) = parse_str(text, cached.clone(), Origin::Managed(cached));
        p.main = Some(main);
        expand(
            &mut p,
            &IncludeOptions {
                base_dir: dir.to_path_buf(),
                max_depth: 8,
                max_files: 200,
            },
            None,
            &mut d,
        );
        (p, d)
    }

    /// The copy of a managed profile takes in a local file below the
    /// profile's directory, and the lines of that file are read as the copy's
    /// own are, whatever they include in turn: being included gives no
    /// rights.
    #[test]
    fn what_a_copy_takes_in_is_read_as_the_copy_is() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("sets")).unwrap();
        fs::write(
            dir.path().join("sets").join("one.dconf"),
            "[Rule]\nDOMAIN,one.example,DIRECT\n#!include ../two.dconf\n#!include two.dconf\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("two.dconf"),
            "[Rule]\nDOMAIN,two.example,DIRECT\n",
        )
        .unwrap();
        let (p, d) = expand_copy(
            dir.path(),
            "[Rule]\n#!include sets/one.dconf\n#!include sets/one.dconf\nFINAL,DIRECT\n",
        );
        // the include by `..` is refused, as the copy's own would be; the one
        // by a plain path is taken in. The file is taken in twice, which is
        // no cycle.
        let told: Vec<(&str, u32)> = d
            .iter()
            .map(|x| (x.code, x.span.as_ref().unwrap().line))
            .collect();
        assert_eq!(
            told,
            [
                (codes::W_REMOTE_LOCAL_ACCESS, 3),
                (codes::W_REMOTE_LOCAL_ACCESS, 3)
            ]
        );
        let one = Arc::<Path>::from(dir.path().join("sets/one.dconf").as_path());
        let two = Arc::<Path>::from(dir.path().join("two.dconf").as_path());
        let origins: Vec<(&str, Origin)> = p
            .section("Rule")
            .unwrap()
            .entries
            .iter()
            .map(|e| (e.raw.as_str(), e.origin.clone()))
            .collect();
        assert_eq!(
            origins,
            [
                (
                    "DOMAIN,one.example,DIRECT",
                    Origin::ManagedInclude(one.clone())
                ),
                (
                    "DOMAIN,two.example,DIRECT",
                    Origin::ManagedInclude(two.clone())
                ),
                ("DOMAIN,one.example,DIRECT", Origin::ManagedInclude(one)),
                ("DOMAIN,two.example,DIRECT", Origin::ManagedInclude(two)),
                (
                    "FINAL,DIRECT",
                    Origin::Managed(Arc::from(Path::new("/cache/1.conf")))
                ),
            ]
        );
    }

    /// The copy of a managed profile includes a plain relative path or its
    /// line is ignored, with the words of the copy: the path is not joined,
    /// resolved or read. The file system is asked about the main file and
    /// nothing else.
    #[test]
    fn a_copy_includes_nothing_but_a_plain_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let absolute = dir.path().join("absent.dconf");
        let (p, d) = expand_copy(
            dir.path(),
            &format!(
                "[Rule]\n#!include {}\n#!include ../up.dconf\n#!include C:\\x.dconf\n#!include \\\\127.0.0.1\\share\\x.dconf\n#!include nul\nFINAL,DIRECT\n",
                absolute.display()
            ),
        );
        let told: Vec<(&str, &str, Option<&str>, u32)> = d
            .iter()
            .map(|x| {
                (
                    x.code,
                    x.message.as_str(),
                    x.hint,
                    x.span.as_ref().unwrap().line,
                )
            })
            .collect();
        let refused = |line| {
            (
                codes::W_REMOTE_LOCAL_ACCESS,
                "the downloaded copy of a managed profile (or a file it includes) can include a local file only by a relative path below the profile's directory; the line is ignored",
                Some("name the file by a relative path without a drive, a leading slash or `..`"),
                line,
            )
        };
        assert_eq!(told, (2..=6).map(refused).collect::<Vec<_>>());
        assert_eq!(raws(&p, "Rule"), ["FINAL,DIRECT"]);
        assert_eq!(take_looked_at(), [dir.path().join("main.conf")]);

        // a plain path is looked for, and here it is not there
        let (_, d) = expand_copy(dir.path(), "[Rule]\n#!include a.dconf, ./b.dconf\n");
        let told: Vec<(&str, u32)> = d
            .iter()
            .map(|x| (x.code, x.span.as_ref().unwrap().line))
            .collect();
        assert_eq!(
            told,
            [
                (codes::E_INCLUDE_NOT_FOUND, 2),
                (codes::E_INCLUDE_NOT_FOUND, 2)
            ]
        );
    }

    /// An ordinary file is read up to the bound and no further, and anything
    /// else is none: a directory, or what is not there.
    #[test]
    fn an_ordinary_file_is_read_up_to_the_bound_and_no_further() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.dconf");
        fs::write(&file, "0123456789").unwrap();
        assert_eq!(read_ordinary_file(&file, 10).unwrap(), "0123456789");
        let too_long = read_ordinary_file(&file, 9).unwrap_err();
        assert_eq!(too_long.to_string(), "larger than 9 bytes");
        // the bound the loader uses is said in MiB
        assert_eq!(size_text(MOST_A_COPY_TAKES_IN), "64 MiB");
        let not_a_file = read_ordinary_file(dir.path(), 10).unwrap_err();
        assert_eq!(not_a_file.to_string(), "not a regular file");
        let missing = read_ordinary_file(&dir.path().join("absent"), 10).unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
        // and text is text, as it is for a file the user included
        fs::write(&file, [0xff, 0xfe]).unwrap();
        assert_eq!(
            read_ordinary_file(&file, 10).unwrap_err().to_string(),
            fs::read_to_string(&file).unwrap_err().to_string()
        );
    }

    #[test]
    fn a_cycle_of_remote_includes_is_an_error_that_names_no_link() {
        let fake = Fake::new()
            .text(
                "https://example.com/a.conf",
                "[Rule]\nDOMAIN,a.example,DIRECT\n#!include https://example.com/b.conf\n",
            )
            .text(
                "https://example.com/b.conf",
                "[Rule]\nDOMAIN,b.example,DIRECT\n#!include https://example.com/a.conf\n",
            )
            .text(
                "https://example.com/self.conf",
                "[Rule]\n#!include https://example.com/self.conf\n",
            );
        let (p, d, remote) = expand_text(
            "[Rule]\n#!include https://example.com/a.conf\n#!include https://example.com/self.conf\nFINAL,DIRECT\n",
            Some(&fake),
        );
        let errors: Vec<(&str, &str, u32)> = d
            .iter()
            .map(|x| (x.code, x.message.as_str(), x.span.as_ref().unwrap().line))
            .collect();
        // a → b → a stops where b asks for a again, self.conf where it asks
        // for itself
        assert_eq!(
            errors,
            [
                (
                    codes::E_INCLUDE_CYCLE,
                    "include cycle through a remote include",
                    3
                ),
                (
                    codes::E_INCLUDE_CYCLE,
                    "include cycle through a remote include",
                    2
                ),
            ]
        );
        assert_eq!(
            d.iter()
                .next()
                .unwrap()
                .span
                .as_ref()
                .unwrap()
                .file
                .as_ref(),
            fake.path_of("https://example.com/b.conf").as_path()
        );
        assert_eq!(
            raws(&p, "Rule"),
            [
                "DOMAIN,a.example,DIRECT",
                "DOMAIN,b.example,DIRECT",
                "FINAL,DIRECT"
            ]
        );
        assert_eq!(remote.len(), 3);
    }

    /// A chain of remote includes is nested no deeper than a chain of local
    /// ones, and the file that is one too deep is not even asked for.
    #[test]
    fn remote_includes_are_nested_no_deeper_than_local_ones() {
        let mut fake = Fake::new();
        for n in 0..10 {
            fake = fake.text(
                &format!("https://example.com/r{n}.conf"),
                &format!(
                    "[Rule]\nDOMAIN,r{n}.example,DIRECT\n#!include https://example.com/r{}.conf\n",
                    n + 1
                ),
            );
        }
        let (p, d, remote) = expand_text(
            "[Rule]\n#!include https://example.com/r0.conf\nFINAL,DIRECT\n",
            Some(&fake),
        );
        let errors: Vec<&str> = d.iter().map(|x| x.message.as_str()).collect();
        assert_eq!(errors, ["include nesting deeper than 8: a remote include"]);
        assert_eq!(raws(&p, "Rule").len(), 9);
        assert_eq!(remote.len(), 8);
        assert_eq!(fake.asked().len(), 8);
    }

    /// The number of files an expansion takes in counts the local ones and the
    /// remote ones together.
    #[test]
    fn local_and_remote_includes_count_against_one_total() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake::new();
        for n in 1..=3 {
            fs::write(
                dir.path().join(format!("l{n}.dconf")),
                format!("[Rule]\nDOMAIN,l{n}.example,DIRECT\n"),
            )
            .unwrap();
            fake = fake.text(
                &format!("https://example.com/r{n}.conf"),
                &format!("[Rule]\nDOMAIN,r{n}.example,DIRECT\n"),
            );
        }
        let (p, d, _) = expand_limited(
            dir.path(),
            "[Rule]\n#!include l1.dconf\n#!include https://example.com/r1.conf\n#!include l2.dconf\n#!include https://example.com/r2.conf\n#!include l3.dconf\n#!include https://example.com/r3.conf\nFINAL,DIRECT\n",
            Some(&fake),
            8,
            4,
        );
        let errors: Vec<&str> = d.iter().map(|x| x.message.as_str()).collect();
        assert_eq!(errors, ["include expansion limit of 4 files exceeded"]);
        assert_eq!(
            raws(&p, "Rule"),
            [
                "DOMAIN,l1.example,DIRECT",
                "DOMAIN,r1.example,DIRECT",
                "DOMAIN,l2.example,DIRECT",
                "DOMAIN,r2.example,DIRECT",
                "FINAL,DIRECT"
            ]
        );
        assert_eq!(fake.asked().len(), 2);
    }

    #[test]
    fn a_remote_include_in_a_local_include_is_expanded() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("shared.dconf"),
            "[Rule]\nDOMAIN,shared.example,DIRECT\n#!include https://example.com/r.conf\n",
        )
        .unwrap();
        let fake = Fake::new().text(
            "https://example.com/r.conf",
            "[Rule]\nDOMAIN,r.example,DIRECT\n",
        );
        let (p, d, remote) = expand_in(
            dir.path(),
            "[Rule]\n#!include shared.dconf\nFINAL,DIRECT\n",
            Some(&fake),
        );
        assert!(d.is_empty(), "{:?}", d.into_vec());
        let entries = &p.section("Rule").unwrap().entries;
        assert_eq!(
            entries.iter().map(|e| e.raw.as_str()).collect::<Vec<_>>(),
            [
                "DOMAIN,shared.example,DIRECT",
                "DOMAIN,r.example,DIRECT",
                "FINAL,DIRECT"
            ]
        );
        assert!(matches!(&entries[0].origin, Origin::Include(_)));
        assert!(matches!(&entries[1].origin, Origin::Remote(_)));
        assert_eq!(remote.len(), 1);
    }

    /// A page of HTML from a portal or an error page is no profile: it has
    /// no section, so there is nothing to take in and the copy is flagged.
    /// The warning names the file the text is in, not the link.
    #[test]
    fn a_remote_text_without_the_section_is_flagged() {
        let fake = Fake::new()
            .text(
                "https://example.com/page.html",
                "<html>\nwelcome to the portal\n</html>\n",
            )
            .text("https://example.com/proxies.conf", "[Proxy]\nA = direct\n")
            .text("https://example.com/empty.conf", "[Rule]\n");
        let (p, d, remote) = expand_text(
            "[Rule]\n#!include https://example.com/page.html\n#!include https://example.com/proxies.conf\n#!include https://example.com/empty.conf\nFINAL,DIRECT\n",
            Some(&fake),
        );
        let missing: Vec<(String, Option<u32>)> = d
            .iter()
            .filter(|x| x.code == codes::W_INCLUDE_SECTION_MISSING)
            .map(|x| (x.message.clone(), x.span.as_ref().map(|s| s.line)))
            .collect();
        let named = |url: &str| format!("`{}` has no [Rule] section", fake.path_of(url).display());
        assert_eq!(
            missing,
            [
                (named("https://example.com/page.html"), Some(2)),
                (named("https://example.com/proxies.conf"), Some(3)),
            ]
        );
        assert_eq!(raws(&p, "Rule"), ["FINAL,DIRECT"]);
        // a section with nothing in it is still the section
        let lacks: Vec<bool> = remote.iter().map(|r| r.lacks_section).collect();
        assert_eq!(lacks, [true, true, false]);
    }

    /// A fragment is never sent to a server, so links that differ only in it
    /// are one link: one remote file, and the source is asked for the link
    /// without it.
    #[test]
    fn a_fragment_is_not_part_of_a_link() {
        let fake = Fake::new().text(
            "https://example.com/r.conf",
            "[Proxy]\nA = direct\n[Rule]\nDOMAIN,a.example,DIRECT\n",
        );
        let (p, d, remote) = expand_text(
            "[Proxy]\n#!include https://example.com/r.conf#one\n[Rule]\n#!include https://example.com/r.conf#two\nFINAL,DIRECT\n",
            Some(&fake),
        );
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert_eq!(raws(&p, "Proxy"), ["A = direct"]);
        assert_eq!(
            raws(&p, "Rule"),
            ["DOMAIN,a.example,DIRECT", "FINAL,DIRECT"]
        );
        assert_eq!(remote.len(), 1);
        assert_eq!(remote[0].url.as_str(), "https://example.com/r.conf");
        assert_eq!(remote[0].url.fragment(), None);
        let asked = fake.asked();
        let asked: Vec<&str> = asked.iter().map(Url::as_str).collect();
        assert_eq!(asked, ["https://example.com/r.conf"]);
    }

    /// And so a text that includes itself under another fragment includes
    /// itself: it is a cycle, found where the text asks for itself again.
    #[test]
    fn a_text_that_includes_itself_under_another_fragment_is_a_cycle() {
        let fake = Fake::new().text(
            "https://example.com/self.conf",
            "[Rule]\nDOMAIN,self.example,DIRECT\n#!include https://example.com/self.conf#again\n",
        );
        let (p, d, remote) = expand_text(
            "[Rule]\n#!include https://example.com/self.conf#first\nFINAL,DIRECT\n",
            Some(&fake),
        );
        let found: Vec<(&str, &str, &Path, u32)> = d
            .iter()
            .map(|x| {
                let span = x.span.as_ref().expect("a position");
                (x.code, x.message.as_str(), span.file.as_ref(), span.line)
            })
            .collect();
        assert_eq!(
            found,
            [(
                codes::E_INCLUDE_CYCLE,
                "include cycle through a remote include",
                fake.path_of("https://example.com/self.conf").as_path(),
                3
            )]
        );
        assert_eq!(
            raws(&p, "Rule"),
            ["DOMAIN,self.example,DIRECT", "FINAL,DIRECT"]
        );
        assert_eq!(remote.len(), 1);
        assert_eq!(fake.asked().len(), 1);
    }

    /// A downloaded file is not the user's to edit, so the hint for a section
    /// it lacks does not tell them to add it, as it does for a file of their
    /// own.
    #[test]
    fn a_downloaded_text_without_the_section_gets_a_hint_that_can_be_followed() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("nosec.dconf"), "[Proxy]\nP = direct\n").unwrap();
        let fake = Fake::new().text("https://example.com/nosec.conf", "[Proxy]\nA = direct\n");
        let (_, d, _) = expand_in(
            dir.path(),
            "[Rule]\n#!include nosec.dconf\n#!include https://example.com/nosec.conf\nFINAL,DIRECT\n",
            Some(&fake),
        );
        let found: Vec<(&str, u32, Option<&str>)> = d
            .iter()
            .map(|x| (x.code, x.span.as_ref().expect("a position").line, x.hint))
            .collect();
        assert_eq!(
            found,
            [
                (
                    codes::W_INCLUDE_SECTION_MISSING,
                    2,
                    codes::hint(codes::W_INCLUDE_SECTION_MISSING)
                ),
                (
                    codes::W_INCLUDE_SECTION_MISSING,
                    3,
                    Some(
                        "check that the address serves a profile with that section, or move the `#!include` line to a section it has"
                    )
                ),
            ]
        );
        // the local one has a hint, and it is not the other
        assert_ne!(found[0].2, None);
        assert_ne!(found[0].2, found[1].2);
    }

    /// A wildcard section takes what matches; a text with nothing that matches
    /// is a text without sections of that kind, not one that lacks a section.
    #[test]
    fn a_wildcard_that_matches_nothing_is_not_a_lack() {
        let fake = Fake::new().text("https://example.com/g.conf", "[General]\nipv6 = true\n");
        let (p, d, remote) = expand_text(
            "[Ruleset *]\n#!include https://example.com/g.conf\n[Rule]\nFINAL,DIRECT\n",
            Some(&fake),
        );
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert!(p.sections_with_prefix("Ruleset ").next().is_none());
        assert!(!remote[0].lacks_section);
    }

    /// A line that is wrong in a downloaded text is reported in the file the
    /// text is kept in, at the line it has there, where the user can open it.
    #[test]
    fn a_problem_in_a_remote_text_is_reported_in_its_file() {
        let fake = Fake::new().text(
            "https://example.com/r.conf",
            "a line before any section\n[Rule]\nDOMAIN,a.example,DIRECT #!REQUIREMENT\nDOMAIN,b.example,DIRECT\n",
        );
        let (p, d, remote) = expand_text(
            "[Rule]\n#!include https://example.com/r.conf\nFINAL,DIRECT\n",
            Some(&fake),
        );
        let cached = fake.path_of("https://example.com/r.conf");
        let found: Vec<(&str, &Path, u32)> = d
            .iter()
            .map(|x| {
                let span = x.span.as_ref().expect("a position");
                (x.code, span.file.as_ref(), span.line)
            })
            .collect();
        assert_eq!(
            found,
            [
                (codes::W_LINE_OUTSIDE_SECTION, cached.as_path(), 1),
                (codes::E_REQUIREMENT_SYNTAX, cached.as_path(), 3),
            ]
        );
        assert_eq!(remote[0].path.as_deref(), Some(cached.as_path()));
        // the line that could be read is where the file has it
        let line_four = &p.section("Rule").unwrap().entries[0];
        assert_eq!(line_four.raw, "DOMAIN,b.example,DIRECT");
        assert_eq!(
            (line_four.span.file.as_ref(), line_four.span.line),
            (cached.as_path(), 4)
        );
    }

    /// How often a file is updated is up to the file when it says so in a
    /// `#!MANAGED-CONFIG` line of its own; what else the line says is not for
    /// an included file to decide, and what is wrong in it is not reported.
    #[test]
    fn the_interval_of_a_remote_include_comes_from_its_own_managed_config_line() {
        let fake = Fake::new()
            .text(
                "https://example.com/with.conf",
                "#!MANAGED-CONFIG https://elsewhere.example/p.conf interval=7200 strict=true\n[Rule]\nDOMAIN,a.example,DIRECT\n",
            )
            .text(
                "https://example.com/without.conf",
                "[Rule]\nDOMAIN,b.example,DIRECT\n",
            )
            .text(
                "https://example.com/broken.conf",
                "#!MANAGED-CONFIG https://elsewhere.example/p.conf interval=soon unknown\n[Rule]\nDOMAIN,c.example,DIRECT\n",
            );
        let (p, d, remote) = expand_text(
            "[Rule]\n#!include https://example.com/with.conf\n#!include https://example.com/without.conf\n#!include https://example.com/broken.conf\nFINAL,DIRECT\n",
            Some(&fake),
        );
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert_eq!(raws(&p, "Rule").len(), 4);
        // the link in the line is not one the load came across
        let urls: Vec<&str> = remote.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://example.com/with.conf",
                "https://example.com/without.conf",
                "https://example.com/broken.conf"
            ]
        );
        let intervals: Vec<u64> = remote.iter().map(|r| r.interval.as_secs()).collect();
        assert_eq!(intervals, [7200, 86400, 86400]);
        assert!(remote.iter().all(|r| !r.strict));
    }

    /// The line of an included text is read as the line of any profile is: an
    /// interval below the shortest is the shortest, and 0 says nothing. What
    /// is wrong in the line is not reported, as for a local include.
    #[test]
    fn the_interval_of_a_remote_include_is_at_least_sixty_seconds() {
        let fake = Fake::new()
            .text(
                "https://example.com/short.conf",
                "#!MANAGED-CONFIG https://elsewhere.example/p.conf interval=30\n[Rule]\nDOMAIN,a.example,DIRECT\n",
            )
            .text(
                "https://example.com/zero.conf",
                "#!MANAGED-CONFIG https://elsewhere.example/p.conf interval=0\n[Rule]\nDOMAIN,b.example,DIRECT\n",
            );
        let (_, d, remote) = expand_text(
            "[Rule]\n#!include https://example.com/short.conf\n#!include https://example.com/zero.conf\nFINAL,DIRECT\n",
            Some(&fake),
        );
        assert!(d.is_empty(), "{:?}", d.into_vec());
        let intervals: Vec<u64> = remote.iter().map(|r| r.interval.as_secs()).collect();
        assert_eq!(intervals, [60, 86400]);
    }

    /// One link named in several sections is one remote file, asked for once
    /// and flagged when any of them lacks the section it is named for.
    #[test]
    fn the_same_link_named_twice_is_one_file_and_either_lack_counts() {
        let link = "https://example.com/r.conf";
        for (text, lacks) in [
            (
                "[Proxy]\nA = direct\n[Rule]\nDOMAIN,a.example,DIRECT\n",
                false,
            ),
            ("[Rule]\nDOMAIN,a.example,DIRECT\n", true),
            ("[Proxy]\nA = direct\n", true),
        ] {
            let fake = Fake::new().text(link, text);
            let (_, d, remote) = expand_text(
                &format!("[Proxy]\n#!include {link}\n[Rule]\n#!include {link}\nFINAL,DIRECT\n"),
                Some(&fake),
            );
            assert_eq!(remote.len(), 1, "{text}");
            assert_eq!(remote[0].lacks_section, lacks, "{text}");
            assert_eq!(
                d.iter()
                    .filter(|x| x.code == codes::W_INCLUDE_SECTION_MISSING)
                    .count(),
                usize::from(lacks),
                "{text}"
            );
            assert_eq!(fake.asked().len(), 1, "{text}");
        }
    }

    /// A cycle is a cycle however the link that closes it is written: the
    /// scheme and the host do not care about case, and a port that is the
    /// scheme's own is no port.
    #[test]
    fn a_cycle_is_found_whatever_the_spelling_of_the_link_that_closes_it() {
        let fake = Fake::new()
            .text(
                "https://example.com/a.conf",
                "[Rule]\nDOMAIN,a.example,DIRECT\n#!include https://example.com/b.conf\n",
            )
            .text(
                "https://example.com/b.conf",
                "[Rule]\nDOMAIN,b.example,DIRECT\n#!include HTTPS://EXAMPLE.com:443/a.conf\n",
            );
        let (p, d, remote) = expand_text(
            "[Rule]\n#!include https://example.com/a.conf\nFINAL,DIRECT\n",
            Some(&fake),
        );
        // it stops where b asks for a again, in b's file, and a is not taken
        // in a second time
        let found: Vec<(&str, &str, &Path, u32)> = d
            .iter()
            .map(|x| {
                let span = x.span.as_ref().expect("a position");
                (x.code, x.message.as_str(), span.file.as_ref(), span.line)
            })
            .collect();
        assert_eq!(
            found,
            [(
                codes::E_INCLUDE_CYCLE,
                "include cycle through a remote include",
                fake.path_of("https://example.com/b.conf").as_path(),
                3
            )]
        );
        assert_eq!(
            raws(&p, "Rule"),
            [
                "DOMAIN,a.example,DIRECT",
                "DOMAIN,b.example,DIRECT",
                "FINAL,DIRECT"
            ]
        );
        assert_eq!(remote.len(), 2);
        assert_eq!(fake.asked().len(), 2);
    }

    /// One link written two ways is one remote file: one entry among the ones
    /// the load came across, and one question to the source.
    #[test]
    fn a_link_written_two_ways_is_one_remote_file() {
        let fake = Fake::new().text(
            "https://example.com/r.conf",
            "[Proxy]\nA = direct\n[Rule]\nDOMAIN,a.example,DIRECT\n",
        );
        let (p, d, remote) = expand_text(
            "[Proxy]\n#!include https://example.com/r.conf\n[Rule]\n#!include HTTPS://Example.COM:443/r.conf\nFINAL,DIRECT\n",
            Some(&fake),
        );
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert_eq!(raws(&p, "Proxy"), ["A = direct"]);
        assert_eq!(
            raws(&p, "Rule"),
            ["DOMAIN,a.example,DIRECT", "FINAL,DIRECT"]
        );
        assert_eq!(remote.len(), 1);
        assert_eq!(remote[0].url.as_str(), "https://example.com/r.conf");
        assert_eq!(fake.asked().len(), 1);
    }
}
