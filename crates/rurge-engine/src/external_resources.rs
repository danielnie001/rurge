//! The external resources as the control plane shows them, and the way to ask
//! for an update of one or of all of them.
//!
//! Two holders keep them: [`ProfileSources`], which lives as long as the
//! process and downloads the managed profile and the remote includes, and the
//! resource manager of the generation in use, which holds the sets, the
//! subscriptions and the GeoIP databases. A key (`resource_key`) names a source
//! in either holder, and in both when one address is a source of the profile
//! and a set at once.

use crate::engine::Engine;
use crate::profile::ProfileSources;
use crate::reload::catch_up;
use rurge_net::resource::{ResourceKind, ResourceManager, ResourceStatus};
use rurge_rules::SetStatus;
use rurge_rules::set_format::SetKind;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout_at};

/// What an update says of a key whose answer is not in when the time is up.
const TIMED_OUT: &str = "timed out";

/// What an update says of a key that was listed a moment ago and is not kept
/// any more.
const NO_LONGER_KEPT: &str = "the resource is no longer kept";

/// What an update says of a key whose task did not come back with an answer.
const DID_NOT_FINISH: &str = "the update did not finish";

/// One external resource as the control plane shows it.
#[derive(Clone, Debug)]
pub struct ExternalResource {
    /// How it is called: `ResourceStatus::shown()`.
    pub path: String,
    /// `ruleset`, `domainset`, `policy-group`, `geoip`, `profile` or `include`.
    pub kind: &'static str,
    pub key: String,
    pub local: bool,
    pub ready: bool,
    /// UNIX seconds of the last successful check, 0 when there has been none;
    /// `None` for a local resource.
    pub updated_at: Option<u64>,
    pub updating: bool,
    /// Why the last update failed, while it is the last thing that happened.
    /// The managed profile says why sessions are refused when no failed
    /// update says it: before the first attempt of a run has ended, and
    /// offline, where none is made.
    pub error: Option<String>,
    /// The engine refuses sessions on account of this resource.
    pub suspended: bool,
    /// What the last load of the file could not use. Only a rule set and a
    /// domain set have it, which are read line by line. The numbers are those
    /// of the content the set was last compiled from: after an update they
    /// stay those of the content before it until the set has had its turn
    /// (the sets of a generation are compiled one at a time), and a set that
    /// has never been compiled says none.
    pub unused: Option<UnusedLines>,
}

/// What the last load of a set's file could not use, as the registry counted
/// it. All zero for a set that has not loaded, and for one that was read
/// whole.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UnusedLines {
    /// Lines that are neither a rule nor a name: they are not in the set.
    pub skipped: usize,
    /// Valid lines beyond the limit of a million entries: not in the set.
    pub truncated: usize,
    /// Rule lines that carry a parameter nobody knows: the line is in the
    /// set, the parameter is not.
    pub unknown_params: usize,
}

/// How the control plane calls a kind of resource. What has no name is not an
/// external resource to it: it is not listed and cannot be updated by its key
/// (the hosts file is watched, not downloaded).
fn type_name(kind: ResourceKind) -> Option<&'static str> {
    match kind {
        ResourceKind::RuleSet => Some("ruleset"),
        ResourceKind::DomainSet => Some("domainset"),
        ResourceKind::PolicyGroup => Some("policy-group"),
        ResourceKind::GeoIp => Some("geoip"),
        ResourceKind::Profile => Some("profile"),
        ResourceKind::Include => Some("include"),
        ResourceKind::System => None,
    }
}

/// The statuses that are listed, with the name of their kind, in the order they
/// are listed in: the sources of the profile first, the managed profile before
/// the includes, then the resources of the generation by name.
fn in_listing_order<'a>(
    own: &'a [ResourceStatus],
    held: &'a [ResourceStatus],
) -> Vec<(&'static str, &'a ResourceStatus)> {
    let named = |statuses: &'a [ResourceStatus]| -> Vec<(&'static str, &'a ResourceStatus)> {
        statuses
            .iter()
            .filter_map(|status| Some((type_name(status.kind)?, status)))
            .collect()
    };
    let mut listing = named(own);
    // stable: the includes stay in the order they are tracked in
    listing.sort_by_key(|(_, status)| status.kind != ResourceKind::Profile);
    let mut rest = named(held);
    rest.sort_by_cached_key(|(_, status)| (status.shown(), status.key.clone()));
    listing.extend(rest);
    listing
}

/// The kind of set a resource of this kind is read as, if it is one: the
/// kinds that are read line by line.
fn set_kind(kind: ResourceKind) -> Option<SetKind> {
    match kind {
        ResourceKind::RuleSet => Some(SetKind::RuleSet),
        ResourceKind::DomainSet => Some(SetKind::DomainSet),
        _ => None,
    }
}

/// What the registry says the last load of the set read from the resource
/// `key` as a set of this `kind` could not use. One address written in two
/// ways is two sets of one resource: the one that has loaded the latest
/// version speaks. None of the sets, because none has been made for the
/// resource, is none of the lines.
fn unused_lines(sets: &[SetStatus], key: &str, kind: SetKind) -> UnusedLines {
    sets.iter()
        .filter(|set| set.kind == kind && set.key.as_deref() == Some(key))
        .max_by_key(|set| set.version)
        .map(|set| UnusedLines {
            skipped: set.skipped,
            truncated: set.truncated,
            unknown_params: set.unknown_params,
        })
        .unwrap_or_default()
}

/// Whether `statuses` list a resource with this key.
fn lists(statuses: &[ResourceStatus], key: &str) -> bool {
    statuses
        .iter()
        .any(|status| status.key == key && type_name(status.kind).is_some())
}

/// What a holder says of an update of a key that was listed in it.
fn answer(updated: Option<Result<(), String>>) -> Result<(), String> {
    updated.unwrap_or_else(|| Err(NO_LONGER_KEPT.to_string()))
}

/// The update of `key` in the holders that list it, together: the first error
/// of the two, the profile's first, else `Ok`.
async fn update_in(
    own: Option<Arc<ProfileSources>>,
    held: Option<Arc<ResourceManager>>,
    key: &str,
) -> Result<(), String> {
    let (own, held) = tokio::join!(
        async {
            match &own {
                Some(sources) => answer(sources.update(key).await),
                None => Ok(()),
            }
        },
        async {
            match &held {
                Some(manager) => answer(manager.update_now(key).await),
                None => Ok(()),
            }
        }
    );
    own.and(held)
}

impl Engine {
    /// Every external resource there is, in the order the control plane lists
    /// them: the sources of the profile (`sources`, `None` for a caller that
    /// has none) with the managed profile first, then the resources of the
    /// generation in use by name. The managed profile says it is `suspended`
    /// while the engine refuses sessions on account of its update. A rule set
    /// and a domain set say what the last load of their file could not use.
    pub fn external_resources(&self, sources: Option<&ProfileSources>) -> Vec<ExternalResource> {
        let own = sources.map(ProfileSources::statuses).unwrap_or_default();
        // one generation for the resources and for the sets read from them
        let runtime = self.runtime();
        let held = runtime.stack.resources.statuses();
        let sets = runtime.stack.registry.statuses();
        let refused = self.suspended();
        in_listing_order(&own, &held)
            .into_iter()
            .map(|(kind, status)| {
                // sessions are refused on account of the managed profile
                let refused = refused
                    .as_ref()
                    .filter(|_| status.kind == ResourceKind::Profile);
                ExternalResource {
                    path: status.shown(),
                    kind,
                    key: status.key.clone(),
                    local: status.local,
                    ready: status.ready,
                    updated_at: (!status.local).then(|| {
                        status
                            .fetched_at
                            .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
                            .map_or(0, |since| since.as_secs())
                    }),
                    updating: status.updating,
                    error: status.last_error.clone().or_else(|| refused.cloned()),
                    suspended: refused.is_some(),
                    unused: set_kind(status.kind)
                        .map(|kind| unused_lines(&sets, &status.key, kind)),
                }
            })
            .collect()
    }

    /// Updates the resource `key` names, or every resource that is not a
    /// local file (`key` is `None`), all at once, and waits for the answers for
    /// `limit` at the most. A key that is not answered by then is
    /// `Err("timed out")` and the others keep their answers. `None` when `key`
    /// names nothing that is listed. A local file is read again and answers
    /// `Ok` at once. An address that is a source of the profile and a set at
    /// once is one key, updated in both holders, and answers the first error
    /// of the two, the profile's first. The answers are in the order of the
    /// listing.
    ///
    /// The request awaits the attempts and nothing after them: not the
    /// recompilation of a set, not the reload that follows a source of the
    /// profile that changed. Dropping the future ends the waiting and nothing
    /// else; the downloads are the manager's.
    ///
    /// Known limits, none of them solved here:
    ///
    /// - The answer is what each attempt came to. Two sources of the profile
    ///   that depend on each other are judged together: the one whose body
    ///   arrived first is refused alone (its key reads as an error) and taken
    ///   a moment later with the other half. The listing shows the state as it
    ///   is afterwards.
    /// - A reload while a request waits. The request keeps the manager it
    ///   began with, and a key answers what the attempt in that manager came
    ///   to. The generation that replaced the old one read the cache when it
    ///   was built, possibly before a download landed there: as it comes in
    ///   it asks again for what the old one checked since, or is checking
    ///   (`Engine::swap_runtime`), and a request that ends under another
    ///   generation than it began with has that one look in the same way
    ///   once more, which finds what was asked of the old one after it
    ///   went. Nobody waits for either. The copy in use then follows within
    ///   the time of one more request, with these exceptions: what the
    ///   generation that came in was fetching at that moment is left to
    ///   that attempt, which may have gone out before the server changed; a
    ///   request that was dropped looks at nothing when it ends; and the
    ///   comparison goes by the wall clock, so a check made after the clock
    ///   was set back can be missed. An attempt of its own that fails is in
    ///   the listing and is made again as any other. `all` on a managed
    ///   profile that changed runs into this: the profile reloads within
    ///   about a second of its update, while the sets are still downloading.
    /// - A request in flight when rurge is asked to stop ends with the grace
    ///   period of the shutdown, as a policy test in flight does.
    pub async fn update_external_resources(
        &self,
        sources: Option<&Arc<ProfileSources>>,
        key: Option<&str>,
        limit: Duration,
    ) -> Option<Vec<(String, Result<(), String>)>> {
        // What is needed of the generation, and no more: the generation itself
        // would keep its resolver, its registry and its tasks for as long as a
        // key waits.
        let manager = self.runtime().stack.resources.clone();
        let own = sources
            .map(|sources| sources.statuses())
            .unwrap_or_default();
        let held = manager.statuses();
        let keys: Vec<String> = match key {
            Some(key) => {
                if !lists(&own, key) && !lists(&held, key) {
                    return None;
                }
                vec![key.to_string()]
            }
            None => {
                let mut keys: Vec<String> = Vec::new();
                for (_, status) in in_listing_order(&own, &held) {
                    if !status.local && !keys.contains(&status.key) {
                        keys.push(status.key.clone());
                    }
                }
                keys
            }
        };

        let deadline = Instant::now() + limit;
        let mut tasks = JoinSet::new();
        for (index, key) in keys.iter().enumerate() {
            let own = lists(&own, key).then(|| sources.cloned()).flatten();
            let held = lists(&held, key).then(|| manager.clone());
            let key = key.clone();
            tasks.spawn(async move {
                let result = timeout_at(deadline, update_in(own, held, &key))
                    .await
                    .unwrap_or_else(|_| Err(TIMED_OUT.to_string()));
                (index, result)
            });
        }
        let mut results: Vec<Option<Result<(), String>>> = vec![None; keys.len()];
        while let Some(done) = tasks.join_next().await {
            if let Ok((index, result)) = done {
                results[index] = Some(result);
            }
        }
        // A generation that came in meanwhile looked at the one before it as
        // it came (`Engine::swap_runtime`): what was asked of that one after
        // the look, it has not seen.
        let in_use = self.runtime().stack.resources.clone();
        if !Arc::ptr_eq(&manager, &in_use) {
            catch_up(&manager, &in_use);
        }
        Some(
            keys.into_iter()
                .zip(results)
                .map(|(key, result)| {
                    (
                        key,
                        result.unwrap_or_else(|| Err(DID_NOT_FINISH.to_string())),
                    )
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(key: &str, kind: SetKind, version: u64, lines: (usize, usize, usize)) -> SetStatus {
        SetStatus {
            name: "a set".to_string(),
            key: Some(key.to_string()),
            kind,
            entries: 0,
            version,
            state: "ok".to_string(),
            skipped: lines.0,
            truncated: lines.1,
            unknown_params: lines.2,
        }
    }

    fn lines(skipped: usize, truncated: usize, unknown_params: usize) -> UnusedLines {
        UnusedLines {
            skipped,
            truncated,
            unknown_params,
        }
    }

    /// One address written in two ways is two sets of one resource: the one
    /// that has loaded the latest version speaks, whichever comes first.
    #[test]
    fn of_two_sets_of_one_resource_the_one_with_the_later_version_speaks() {
        let (older, newer) = (
            set("k", SetKind::RuleSet, 2, (1, 2, 3)),
            set("k", SetKind::RuleSet, 3, (4, 5, 6)),
        );
        for sets in [
            [older.clone(), newer.clone()],
            [newer.clone(), older.clone()],
        ] {
            assert_eq!(unused_lines(&sets, "k", SetKind::RuleSet), lines(4, 5, 6));
        }
    }

    /// A set is the set of one resource, read in one way: another key, another
    /// kind, and a built-in or inline set that has no key are not it.
    #[test]
    fn only_a_set_of_the_same_resource_and_the_same_kind_speaks() {
        let mut keyless = set("k", SetKind::RuleSet, 9, (7, 7, 7));
        keyless.key = None;
        let sets = [
            set("other", SetKind::RuleSet, 9, (7, 7, 7)),
            set("k", SetKind::DomainSet, 9, (7, 7, 7)),
            keyless,
            set("k", SetKind::RuleSet, 1, (1, 0, 2)),
        ];
        assert_eq!(unused_lines(&sets, "k", SetKind::RuleSet), lines(1, 0, 2));
        assert_eq!(unused_lines(&sets, "k", SetKind::DomainSet), lines(7, 7, 7));
    }

    /// A resource none of the sets was made for has no lines to speak of, and
    /// says none.
    #[test]
    fn a_resource_without_a_set_has_no_lines_to_speak_of() {
        let sets = [set("other", SetKind::RuleSet, 1, (3, 0, 1))];
        assert_eq!(
            unused_lines(&sets, "k", SetKind::RuleSet),
            UnusedLines::default()
        );
        assert_eq!(
            unused_lines(&[], "k", SetKind::DomainSet),
            UnusedLines::default()
        );
    }

    /// Only the two kinds that are read line by line are sets.
    #[test]
    fn only_a_rule_set_and_a_domain_set_are_read_as_sets() {
        assert_eq!(set_kind(ResourceKind::RuleSet), Some(SetKind::RuleSet));
        assert_eq!(set_kind(ResourceKind::DomainSet), Some(SetKind::DomainSet));
        for kind in [
            ResourceKind::PolicyGroup,
            ResourceKind::GeoIp,
            ResourceKind::Profile,
            ResourceKind::Include,
            ResourceKind::System,
        ] {
            assert_eq!(set_kind(kind), None, "{kind:?}");
        }
    }
}
