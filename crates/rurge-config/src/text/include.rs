//! `#!include` expansion for local files. Remote includes are handled by the
//! resource manager in a later milestone; here they only produce a warning.

use super::{Entry, Origin, Profile, Section, parse_str};
use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::value::split_list;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct IncludeOptions {
    pub base_dir: PathBuf,
    pub max_depth: usize,
}

impl Default for IncludeOptions {
    fn default() -> Self {
        Self {
            base_dir: PathBuf::from("."),
            max_depth: 8,
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

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

pub fn expand(profile: &mut Profile, opts: &IncludeOptions, diags: &mut Diagnostics) {
    let mut stack: Vec<PathBuf> = Vec::new();
    if let Some(main) = &profile.main {
        stack.push(canonical(main));
    }
    expand_inner(profile, opts, diags, &mut stack, 0);
}

fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

fn expand_inner(
    profile: &mut Profile,
    opts: &IncludeOptions,
    diags: &mut Diagnostics,
    stack: &mut Vec<PathBuf>,
    depth: usize,
) {
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
            for target in split_list(rest) {
                if is_url(&target) {
                    diags.push(
                        Diagnostic::warning(
                            codes::W_REMOTE_INCLUDE_UNSUPPORTED,
                            format!("remote include not supported yet: {target}"),
                        )
                        .at(entry.span.clone()),
                    );
                    continue;
                }
                let path = if Path::new(&target).is_absolute() {
                    PathBuf::from(&target)
                } else {
                    opts.base_dir.join(&target)
                };
                let canon = canonical(&path);
                if stack.contains(&canon) {
                    diags.push(
                        Diagnostic::error(
                            codes::E_INCLUDE_CYCLE,
                            format!("include cycle: {}", path.display()),
                        )
                        .at(entry.span.clone()),
                    );
                    continue;
                }
                if depth >= opts.max_depth {
                    diags.push(
                        Diagnostic::error(
                            codes::E_INCLUDE_CYCLE,
                            format!(
                                "include nesting deeper than {}: {}",
                                opts.max_depth,
                                path.display()
                            ),
                        )
                        .at(entry.span.clone()),
                    );
                    continue;
                }
                let text = match fs::read_to_string(&path) {
                    Ok(t) => t,
                    Err(e) => {
                        diags.push(
                            Diagnostic::error(
                                codes::E_INCLUDE_NOT_FOUND,
                                format!("cannot read include `{}`: {e}", path.display()),
                            )
                            .at(entry.span.clone()),
                        );
                        continue;
                    }
                };
                let file: Arc<Path> = Arc::from(path.as_path());
                let (mut sub, sub_diags) =
                    parse_str(&text, file.clone(), Origin::Include(file.clone()));
                diags.extend(sub_diags);
                stack.push(canon);
                expand_inner(&mut sub, opts, diags, stack, depth + 1);
                stack.pop();

                if let Some(prefix) = &wildcard {
                    let (matched, _): (Vec<Section>, Vec<Section>) = sub
                        .sections
                        .into_iter()
                        .partition(|s| starts_with_ci(&s.name, prefix));
                    appended.extend(matched);
                } else if let Some(s) = sub.section_mut(&section.name) {
                    new_entries.append(&mut s.entries);
                } else {
                    diags.push(
                        Diagnostic::warning(
                            codes::W_INCLUDE_SECTION_MISSING,
                            format!("`{}` has no [{}] section", path.display(), section.name),
                        )
                        .at(entry.span.clone()),
                    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{Origin, parse_str};
    use std::fs;
    use std::sync::Arc;

    fn load(dir: &std::path::Path, main: &str) -> (Profile, Diagnostics) {
        let path = dir.join(main);
        let text = fs::read_to_string(&path).unwrap();
        let (mut p, mut d) = parse_str(&text, Arc::from(path.as_path()), Origin::Main);
        expand(
            &mut p,
            &IncludeOptions {
                base_dir: dir.to_path_buf(),
                max_depth: 8,
            },
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
        assert!(codes_seen.contains(&codes::W_REMOTE_INCLUDE_UNSUPPORTED));
        assert!(codes_seen.contains(&codes::W_INCLUDE_SECTION_MISSING));
        assert!(codes_seen.contains(&codes::E_INCLUDE_CYCLE));
        assert_eq!(
            p.section("Rule").unwrap().entries.last().unwrap().raw,
            "FINAL,DIRECT"
        );
    }
}
