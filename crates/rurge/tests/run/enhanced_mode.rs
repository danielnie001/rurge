//! The enhanced mode's crash recovery, end to end (M4a task 7): what a run that
//! died with the enhanced mode on leaves behind, and what the next start makes
//! of it. The "system" is the file backend's JSON file
//! (`RURGE_ENHANCED_BACKEND=file:<path>`), so nothing here reads or changes the
//! machine's routes or DNS, and no test reaches the platform backend.

use super::*;
use rurge_platform::route::{RouteInputs, RouteSet};
use serde_json::{Value, json};
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

/// The file backend's file, or nothing when it is not there yet: a test waits
/// for a daemon's first step by asking this, and the file is made by the step
/// itself.
fn backend_json(file: &Path) -> Value {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

/// What the file backend says it has done, in order.
fn actions(file: &Path) -> Vec<String> {
    backend_json(file)["actions"]
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

/// `LISTENERS` with the HTTP API: for the tests that switch the enhanced mode
/// through it, or stop the daemon that way. The key is `k`.
const LISTENERS_AND_API: &str = "http-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nhttp-api = k@127.0.0.1:0\nloglevel = info";

/// The routes an enhanced mode of these tests' profile installs, as the file
/// backend writes them: the profile has no `tun-*` items and no `external`
/// policies, so the computation is the base prefixes alone, and the count is
/// never written out here (M4a task 8).
fn routes_of(ipv6: bool) -> Vec<String> {
    let set = RouteSet::compute(&RouteInputs {
        ipv6,
        excluded: &[],
        included: &[],
        external: &[],
        explicit_space: cfg!(target_os = "macos"),
    });
    let mut all: Vec<String> = set
        .v4
        .iter()
        .map(ToString::to_string)
        .chain(set.v6.iter().map(ToString::to_string))
        .collect();
    all.sort();
    all
}

/// The routes the file backend says are installed.
fn installed(file: &Path) -> Vec<String> {
    backend_json(file)["routes"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A JSON body of the API as a value.
fn parsed(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("not JSON ({e}): {body}"))
}

/// The data directory a test starts a daemon on, made first: the file backend
/// writes into it (`enhanced_file`), and a daemon that ends before it has made
/// anything must not leave the test with a missing file instead of an answer.
fn prepared_data(dir: &Path) -> PathBuf {
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    data
}

#[test]
fn the_enhanced_mode_opens_at_start_and_closes_at_stop() {
    let dir = tempfile::tempdir().unwrap();
    let conf = write_conf(dir.path(), LISTENERS_AND_API);
    let data = prepared_data(dir.path());
    let file = enhanced_file(&data);
    let mut cmd = run_with_the_file_backend(&conf, &data, &file);
    cmd.args(["--enhanced-mode", "--system-proxy"]);
    let mut daemon = spawn_command(cmd);
    let expected = routes_of(false);
    wait_until("the enhanced mode to be on", || {
        actions(&file).last().map(String::as_str) == Some("adjust")
    });
    assert_eq!(
        actions(&file),
        [
            "open".to_string(),
            "configure".to_string(),
            format!("routes +{} -0", expected.len()),
            "take_dns".to_string(),
            "adjust".to_string(),
        ]
    );
    let system = read_json(&file);
    assert_eq!(system["dns"], true, "{system}");
    assert_eq!(system["adjusted"], true, "{system}");
    assert_eq!(installed(&file), expected);
    let state = read_json(&data.join("state.json"));
    assert_eq!(state["features"]["enhanced_mode"], true, "{state}");
    assert_eq!(
        state["enhanced_backup"]["undo"].as_array().map(Vec::len),
        Some(2),
        "the two changes that outlive the device are in the backup: {state}"
    );

    let api = api_port(&daemon);
    assert_eq!(api_call(api, "POST", "/v1/stop", "k", Some("{}")).0, 200);
    assert_eq!(wait_for_exit(&mut daemon, 20), Some(0));
    assert_eq!(
        actions(&file)[5..],
        [
            "give_dns_back".to_string(),
            "undo take_dns".to_string(),
            format!("routes +0 -{}", expected.len()),
            "close".to_string(),
            "undo adjust".to_string(),
        ],
        "a close gives the DNS back, then the routes, then the device"
    );
    let state = read_json(&data.join("state.json"));
    assert!(state["enhanced_backup"].is_null(), "{state}");
    let lines = printed_to_the_end(&daemon);
    let off = lines.iter().position(|line| line == "enhanced mode off");
    let restored = lines
        .iter()
        .position(|line| line == "system proxy restored");
    assert!(
        off.is_some() && restored.is_some(),
        "both were said at the exit: {lines:?}"
    );
    assert!(
        off < restored,
        "the enhanced mode goes before the system proxy (M4-D16): {lines:?}"
    );
}

#[test]
fn turning_it_on_and_off_through_the_api() {
    let dir = tempfile::tempdir().unwrap();
    let conf = write_conf(dir.path(), LISTENERS_AND_API);
    let data = prepared_data(dir.path());
    let file = enhanced_file(&data);
    let daemon = spawn_command(run_with_the_file_backend(&conf, &data, &file));
    let api = api_port(&daemon);

    let body = parsed(&api_call(api, "GET", "/v1/features/enhanced_mode", "k", None).1);
    assert_eq!(body["enabled"], false);
    let dropped = body["dropped"].as_object().unwrap();
    assert_eq!(dropped.len(), 10, "ten reasons: {body}");
    for (name, count) in dropped {
        assert_eq!(count, 0, "{name}: {body}");
    }
    assert_eq!(body["stackPanics"], 0, "{body}");

    assert_eq!(
        api_call(
            api,
            "POST",
            "/v1/features/enhanced_mode",
            "k",
            Some("{\"enabled\": true}")
        )
        .0,
        200
    );
    wait_until("the enhanced mode to be on", || {
        actions(&file).last().map(String::as_str) == Some("adjust")
    });
    assert_eq!(
        parsed(&api_call(api, "GET", "/v1/features/enhanced_mode", "k", None).1)["enabled"],
        true
    );
    let on = actions(&file);
    assert_eq!(on[0], "open");
    assert_eq!(installed(&file), routes_of(false));

    // already on: success, and no work
    assert_eq!(
        api_call(
            api,
            "POST",
            "/v1/features/enhanced_mode",
            "k",
            Some("{\"enabled\": true}")
        )
        .0,
        200
    );
    assert_eq!(actions(&file), on, "a second `true` did nothing");

    assert_eq!(
        api_call(
            api,
            "POST",
            "/v1/features/enhanced_mode",
            "k",
            Some("{\"enabled\": false}")
        )
        .0,
        200
    );
    wait_until("the enhanced mode to be off", || {
        actions(&file).last().map(String::as_str) == Some("undo adjust")
    });
    assert_eq!(
        parsed(&api_call(api, "GET", "/v1/features/enhanced_mode", "k", None).1)["enabled"],
        false
    );
    assert!(installed(&file).is_empty(), "the routes are off again");
    drop(daemon);
}

#[test]
fn a_step_that_fails_undoes_what_was_done_and_the_start_ends_with_1() {
    let dir = tempfile::tempdir().unwrap();
    let conf = write_conf(dir.path(), LISTENERS);
    let data = prepared_data(dir.path());
    let file = enhanced_file(&data);
    std::fs::write(&file, json!({"fail": "take_dns"}).to_string()).unwrap();
    let mut cmd = run_with_the_file_backend(&conf, &data, &file);
    cmd.arg("--enhanced-mode");
    let (code, stderr) = give_up(cmd);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("error: cannot turn the enhanced mode on: take_dns failed (test)"),
        "{stderr}"
    );
    let expected = routes_of(false);
    assert_eq!(
        actions(&file),
        [
            "open".to_string(),
            "configure".to_string(),
            format!("routes +{} -0", expected.len()),
            "give_dns_back".to_string(),
            "undo take_dns".to_string(),
            format!("routes +0 -{}", expected.len()),
            "close".to_string(),
        ],
        "what the open did goes back in the close's order"
    );
    let state = read_json(&data.join("state.json"));
    assert!(state["enhanced_backup"].is_null(), "{state}");
}

#[test]
fn post_failure_is_a_500_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let conf = write_conf(dir.path(), LISTENERS_AND_API);
    let data = prepared_data(dir.path());
    let file = enhanced_file(&data);
    std::fs::write(&file, json!({"fail": "routes"}).to_string()).unwrap();
    let daemon = spawn_command(run_with_the_file_backend(&conf, &data, &file));
    let api = api_port(&daemon);
    let (status, body) = api_call_within(
        api,
        "POST",
        "/v1/features/enhanced_mode",
        "k",
        Some("{\"enabled\": true}"),
        Duration::from_secs(20),
    );
    assert_eq!(status, 500, "{body}");
    assert!(body.contains("routes failed (test)"), "{body}");
    assert_eq!(
        parsed(&api_call(api, "GET", "/v1/features/enhanced_mode", "k", None).1)["enabled"],
        false,
        "a failed open turns nothing on"
    );
    drop(daemon);
}

#[test]
fn a_killed_run_with_the_enhanced_mode_on_is_undone_by_the_next_start() {
    let dir = tempfile::tempdir().unwrap();
    let conf = write_conf(dir.path(), LISTENERS_AND_API);
    let data = prepared_data(dir.path());
    let file = enhanced_file(&data);
    let mut cmd = run_with_the_file_backend(&conf, &data, &file);
    cmd.arg("--enhanced-mode");
    let daemon = spawn_command(cmd);
    wait_until("the enhanced mode to be on", || {
        actions(&file).last().map(String::as_str) == Some("adjust")
    });
    // the way a run dies: nothing is given back
    drop(daemon);
    assert_eq!(read_json(&file)["dns"], true, "the DNS is still taken over");

    // the next start has no `--enhanced-mode`, and undoes what the dead one
    // left before it says where it listens
    let daemon = spawn_command(run_with_the_file_backend(&conf, &data, &file));
    let expected = [
        "open".to_string(),
        "configure".to_string(),
        format!("routes +{} -0", routes_of(false).len()),
        "take_dns".to_string(),
        "adjust".to_string(),
        "undo adjust".to_string(),
        "undo take_dns".to_string(),
    ];
    assert_eq!(actions(&file), expected);
    let system = read_json(&file);
    assert_eq!(system["dns"], false, "{system}");
    assert_eq!(system["adjusted"], false, "{system}");
    assert_eq!(
        read_json(&data.join("state.json"))["features"]["enhanced_mode"],
        false
    );
    drop(daemon);
}

#[test]
fn ipv6_vif_always_opens_with_ipv6() {
    let dir = tempfile::tempdir().unwrap();
    let general = format!("{LISTENERS}\nipv6-vif = always");
    let conf = write_conf(dir.path(), &general);
    let data = prepared_data(dir.path());
    let file = enhanced_file(&data);
    let mut cmd = run_with_the_file_backend(&conf, &data, &file);
    cmd.arg("--enhanced-mode");
    let daemon = spawn_command(cmd);
    wait_until("the enhanced mode to be on", || {
        actions(&file).last().map(String::as_str) == Some("adjust")
    });
    let on = actions(&file);
    assert!(on.contains(&"configure".to_string()), "{on:?}");
    let system = read_json(&file);
    assert_eq!(system["ipv6"], true, "{system}");
    let expected = routes_of(true);
    assert!(
        expected.iter().any(|net| net.contains(':')),
        "the IPv6 base prefixes are in what is wanted"
    );
    assert_eq!(installed(&file), expected, "the IPv6 routes went in too");
    drop(daemon);
}

#[test]
fn ipv6_that_fails_goes_on_without_it() {
    let dir = tempfile::tempdir().unwrap();
    let general = format!("{LISTENERS}\nipv6-vif = always");
    let conf = write_conf(dir.path(), &general);
    let data = prepared_data(dir.path());
    let file = enhanced_file(&data);
    std::fs::write(&file, json!({"fail": "configure_ipv6"}).to_string()).unwrap();
    let logs = dir.path().join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    let mut cmd = run_with_the_file_backend(&conf, &data, &file);
    cmd.arg("--enhanced-mode")
        .arg("--log-file")
        .arg(logs.join("rurge.log"));
    let daemon = spawn_command(cmd);
    wait_until("the enhanced mode to be on", || {
        actions(&file).last().map(String::as_str) == Some("adjust")
    });
    let system = read_json(&file);
    assert_eq!(system["ipv6"], false, "{system}");
    let expected = routes_of(false);
    assert!(
        expected.iter().all(|net| !net.contains(':')),
        "what is wanted has no IPv6 half"
    );
    assert_eq!(installed(&file), expected, "no IPv6 route went in");
    wait_until("the log to say IPv6 is out", || {
        log_text(&logs).contains("the enhanced mode goes on without it")
    });
    let log = log_text(&logs);
    assert!(
        log.lines()
            .any(|line| line.contains("WARN") && line.contains("without it")),
        "{log}"
    );
    drop(daemon);
}

#[test]
fn a_device_that_fails_turns_the_enhanced_mode_off() {
    let dir = tempfile::tempdir().unwrap();
    let conf = write_conf(dir.path(), LISTENERS_AND_API);
    let data = prepared_data(dir.path());
    let file = enhanced_file(&data);
    std::fs::write(&file, json!({"device_fails_after_ms": 200}).to_string()).unwrap();
    let logs = dir.path().join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    let mut cmd = run_with_the_file_backend(&conf, &data, &file);
    cmd.arg("--enhanced-mode")
        .arg("--log-file")
        .arg(logs.join("rurge.log"));
    let daemon = spawn_command(cmd);
    let api = api_port(&daemon);
    let expected = routes_of(false);
    wait_until(
        "the enhanced mode to be off after the device failed",
        || actions(&file).last().map(String::as_str) == Some("undo adjust"),
    );
    assert_eq!(
        actions(&file)[5..],
        [
            "give_dns_back".to_string(),
            "undo take_dns".to_string(),
            format!("routes +0 -{}", expected.len()),
            "close".to_string(),
            "undo adjust".to_string(),
        ]
    );
    assert_eq!(
        parsed(&api_call(api, "GET", "/v1/features/enhanced_mode", "k", None).1)["enabled"],
        false
    );
    let state = read_json(&data.join("state.json"));
    assert!(state["enhanced_backup"].is_null(), "{state}");
    wait_until("the log to say the adapter failed", || {
        log_text(&logs).contains("the virtual adapter failed")
    });
    drop(daemon);
}
