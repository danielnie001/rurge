//! The enhanced mode's crash recovery, end to end (M4a task 7): what a run that
//! died with the enhanced mode on leaves behind, and what the next start makes
//! of it. The "system" is the file backend's JSON file
//! (`RURGE_ENHANCED_BACKEND=file:<path>`), so nothing here reads or changes the
//! machine's routes or DNS, and no test reaches the platform backend.

use super::*;
use serde_json::json;
use std::path::{Path, PathBuf};

/// A profile that serves both listeners on a port of the system's choosing and
/// logs at INFO, so that the log file has what the start says.
const LISTENERS: &str = "http-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nloglevel = info";

/// The enhanced mode's "system" of a `rurge run` this file starts.
fn enhanced_file(data: &Path) -> PathBuf {
    data.join("enhanced.json")
}

/// `rurge_run` with the enhanced mode's backend pointed at `file`: a file that
/// stands in for the machine.
fn run_with_the_file_backend(conf: &Path, data: &Path, file: &Path) -> Command {
    let mut cmd = rurge_run(conf, data);
    cmd.env("RURGE_ENHANCED_BACKEND", format!("file:{}", file.display()));
    cmd
}

/// What the file backend says it has done, in order.
fn actions(file: &Path) -> Vec<String> {
    read_json(file)["actions"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|step| step.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A data directory as a run killed with the enhanced mode on leaves it: the
/// two changes that outlive it in `state.json`, in the order they were made,
/// and a system that has both of them on. `extra` goes into the backend's
/// file, which is how a test makes a step fail.
fn left_by_a_dead_run(dir: &Path, extra: serde_json::Value) -> (PathBuf, PathBuf) {
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("state.json"),
        json!({
            "version": 1,
            "features": { "enhanced_mode": true },
            "enhanced_backup": { "undo": [
                { "kind": "test", "step": "take_dns" },
                { "kind": "test", "step": "adjust" },
            ] },
        })
        .to_string(),
    )
    .unwrap();
    let file = enhanced_file(&data);
    let mut system = json!({
        "open": true,
        "ipv6": false,
        "routes": [],
        "dns": true,
        "adjusted": true,
        "actions": ["open", "configure", "take_dns", "adjust"],
    });
    for (key, value) in extra.as_object().unwrap() {
        system[key] = value.clone();
    }
    std::fs::write(&file, system.to_string()).unwrap();
    (data, file)
}

/// What the daemon wrote to its log file: every file the rotation made in
/// `dir`, so a run on either side of midnight is found as well.
fn log_text(dir: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return String::new();
    };
    let mut text = String::new();
    for entry in entries.filter_map(Result::ok) {
        if entry.file_name().to_string_lossy().starts_with("rurge.log") {
            text.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
        }
    }
    text
}

/// What the environment of a prepared command gives `name`, if anything.
fn env_of(mut envs: std::process::CommandEnvs<'_>, name: &str) -> Option<String> {
    envs.find(|(set, _)| *set == name)
        .and_then(|(_, value)| value)
        .map(|value| value.to_string_lossy().into_owned())
}

/// A run that died with the enhanced mode on left two changes out in the
/// world: the next start undoes them, last first, and says so in its log.
#[test]
fn what_a_dead_run_left_is_undone_by_the_next_start() {
    let dir = tempfile::tempdir().unwrap();
    let conf = write_conf(dir.path(), LISTENERS);
    let (data, file) = left_by_a_dead_run(dir.path(), json!({}));
    let logs = dir.path().join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    let mut cmd = run_with_the_file_backend(&conf, &data, &file);
    cmd.arg("--log-file").arg(logs.join("rurge.log"));
    let daemon = spawn_command(cmd);
    // the recovery is over by the time the daemon says where it listens
    assert_eq!(
        actions(&file),
        [
            "open",
            "configure",
            "take_dns",
            "adjust",
            // last first
            "undo adjust",
            "undo take_dns",
        ]
    );
    let state = read_json(&data.join("state.json"));
    assert!(state["enhanced_backup"].is_null(), "{state}");
    assert_eq!(state["features"]["enhanced_mode"], false, "{state}");
    wait_until("the log to say the enhanced mode was restored", || {
        log_text(&logs).contains("restored what the enhanced mode left last time")
    });
    drop(daemon);
}

/// The same when the profile does not load: the start ends as it always did,
/// but what the enhanced mode left goes back first — the machine's DNS cannot
/// wait for a profile that loads.
#[test]
fn what_a_dead_run_left_is_undone_even_when_the_profile_does_not_load() {
    let dir = tempfile::tempdir().unwrap();
    let [(broken, says), _] = profiles_that_do_not_load(dir.path());
    let (data, file) = left_by_a_dead_run(dir.path(), json!({}));
    let (code, stderr) = give_up(run_with_the_file_backend(&broken, &data, &file));
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains(says), "{stderr}");
    assert!(
        stderr.contains("a previous run left changes of the enhanced mode; they were undone"),
        "{stderr}"
    );
    assert_eq!(
        actions(&file),
        [
            "open",
            "configure",
            "take_dns",
            "adjust",
            "undo adjust",
            "undo take_dns",
        ]
    );
    assert!(read_json(&data.join("state.json"))["enhanced_backup"].is_null());
}

/// An undo that fails stays in `state.json` and is tried again by the next
/// start; the one that went back does not come back with it.
#[test]
fn an_undo_that_fails_stays_for_the_next_start() {
    let dir = tempfile::tempdir().unwrap();
    let conf = write_conf(
        dir.path(),
        "http-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0",
    );
    let (data, file) = left_by_a_dead_run(dir.path(), json!({"fail_undo": "take_dns"}));
    let daemon = spawn_command(run_with_the_file_backend(&conf, &data, &file));
    assert_eq!(
        actions(&file),
        ["open", "configure", "take_dns", "adjust", "undo adjust"],
        "the takeover's own undo failed"
    );
    let state = read_json(&data.join("state.json"));
    assert_eq!(
        state["enhanced_backup"],
        json!({"undo": [{"kind": "test", "step": "take_dns"}]}),
        "{state}"
    );
    drop(daemon);
    // without the failure the next start undoes what is left
    let mut system = read_json(&file);
    system.as_object_mut().unwrap().remove("fail_undo");
    std::fs::write(&file, system.to_string()).unwrap();
    let daemon = spawn_command(run_with_the_file_backend(&conf, &data, &file));
    assert_eq!(
        actions(&file),
        [
            "open",
            "configure",
            "take_dns",
            "adjust",
            "undo adjust",
            "undo take_dns",
        ]
    );
    assert!(read_json(&data.join("state.json"))["enhanced_backup"].is_null());
    drop(daemon);
}

/// The two helpers every test here starts `rurge` with take the enhanced mode
/// away: without that, a start on a machine that can have it would take over
/// the machine's routes and DNS.
#[test]
fn the_helpers_keep_the_enhanced_mode_off() {
    assert_eq!(
        env_of(rurge_process().get_envs(), "RURGE_ENHANCED_BACKEND").as_deref(),
        Some("off")
    );
    assert_eq!(
        env_of(rurge_command().get_envs(), "RURGE_ENHANCED_BACKEND").as_deref(),
        Some("off")
    );
}

/// A value the variable does not know ends the start as an unknown
/// `RURGE_NETWORK_BACKEND` does: a typo must not reach a real system.
#[test]
fn an_unknown_enhanced_backend_ends_the_start_with_2() {
    let dir = tempfile::tempdir().unwrap();
    let conf = write_conf(dir.path(), LISTENERS);
    let data = dir.path().join("data");
    let mut cmd = rurge_run(&conf, &data);
    cmd.env("RURGE_ENHANCED_BACKEND", "bogus");
    let (code, stderr) = give_up(cmd);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr
            .contains("unknown RURGE_ENHANCED_BACKEND value `bogus` (expected off or file:<path>)"),
        "{stderr}"
    );
}
