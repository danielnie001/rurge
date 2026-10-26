//! The enhanced mode's crash recovery, end to end (M4a task 7): what a run that
//! died with the enhanced mode on leaves behind, and what the next start makes
//! of it. The "system" is the file backend's JSON file
//! (`RURGE_ENHANCED_BACKEND=file:<path>`), so nothing here reads or changes the
//! machine's routes or DNS, and no test reaches the platform backend.

use super::the_network::{OFFICE, World, wait_for_log, wifi};
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

/// What a profile with no `tun-*` items and no `external` policies installs,
/// with `one_excluded` left out where it is given (a `tun-excluded-routes`
/// item): the base prefixes alone, so the count is never written out here
/// (M4a task 8).
fn routes_set(ipv6: bool, one_excluded: Option<&str>) -> RouteSet {
    let excluded = one_excluded.map(|text| text.parse().unwrap());
    RouteSet::compute(&RouteInputs {
        ipv6,
        excluded: excluded.as_slice(),
        included: &[],
        external: &[],
        explicit_space: cfg!(target_os = "macos"),
    })
}

/// What the file backend writes for `set`: one string a prefix, sorted.
fn strings(set: &RouteSet) -> Vec<String> {
    let mut all: Vec<String> = set
        .v4
        .iter()
        .map(ToString::to_string)
        .chain(set.v6.iter().map(ToString::to_string))
        .collect();
    all.sort();
    all
}

/// The routes an enhanced mode of these tests' profile installs.
fn routes_of(ipv6: bool) -> Vec<String> {
    strings(&routes_set(ipv6, None))
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
    // The line comes once `enable` has returned, and so after `state.json`'s
    // flag is written (the file's last action, `adjust`, is written before it).
    // `wait_for_line` takes the lines it reads, and the API's line comes first.
    let api = api_port(&daemon);
    wait_for_line(&daemon, "enhanced mode on");
    let expected = routes_of(false);
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
    wait_for_line(&daemon, "enhanced mode on");
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
    wait_for_line(&daemon, "enhanced mode on");
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
    wait_for_line(&daemon, "enhanced mode on");
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
    // A close writes `undo adjust` first, then rewrites the backup, and only
    // then says the mode is off; so the sequence and `state.json` are read once
    // that last action is in the file and `GET` says off. `GET` alone is not
    // enough: it is false before the open has finished as well.
    wait_until(
        "the enhanced mode to be off after the device failed",
        || {
            actions(&file).last().map(String::as_str) == Some("undo adjust")
                && parsed(&api_call(api, "GET", "/v1/features/enhanced_mode", "k", None).1)["enabled"]
                    == false
        },
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

/// The enhanced mode's "system" of a `World`: the data directory is made
/// first, because the file backend writes into it.
fn world_file(world: &World) -> PathBuf {
    let data = world.data();
    std::fs::create_dir_all(&data).unwrap();
    enhanced_file(&data)
}

/// `world.run(conf, extra)` with the enhanced mode's backend pointed at
/// `file`, beside the network file `World::run` gives the daemon.
fn run_enhanced(world: &World, conf: &Path, file: &Path, extra: &[&str]) -> Command {
    let mut cmd = world.run(conf, extra);
    cmd.env("RURGE_ENHANCED_BACKEND", format!("file:{}", file.display()));
    cmd
}

/// Whether `GET /v1/features/enhanced_mode` says the enhanced mode is on.
fn enhanced_on(api: u16) -> bool {
    let (status, body) = api_call(api, "GET", "/v1/features/enhanced_mode", "k", None);
    assert_eq!(status, 200, "{body}");
    parsed(&body)["enabled"]
        .as_bool()
        .unwrap_or_else(|| panic!("no `enabled` in {body}"))
}

/// `POST /v1/features/enhanced_mode`, the status only: what is asked for
/// during a pause is answered with success without being done.
fn ask_for_the_enhanced_mode(api: u16, on: bool) -> u16 {
    api_call(
        api,
        "POST",
        "/v1/features/enhanced_mode",
        "k",
        Some(&json!({ "enabled": on }).to_string()),
    )
    .0
}

/// How often the backend did `step`.
fn done(file: &Path, step: &str) -> usize {
    actions(file).iter().filter(|done| *done == step).count()
}

/// A start on a network that says to suspend opens nothing: the enhanced mode
/// is only noted, and the pause is what keeps it off. Leaving the pause opens
/// it, going back closes it, and leaving again opens it once more.
#[test]
fn the_enhanced_mode_waits_on_a_suspend_network_and_opens_when_it_is_left() {
    let world = World::on(&wifi("Office"));
    let conf = world.profile("", OFFICE);
    let file = world_file(&world);
    let daemon = spawn_command(run_enhanced(&world, &conf, &file, &["--enhanced-mode"]));
    let api = api_port(&daemon);
    wait_for_log(&daemon, "takeover paused on this network");
    assert_eq!(
        backend_json(&file),
        Value::Null,
        "nothing was asked of the system"
    );
    assert!(!enhanced_on(api));

    world.move_to(&wifi("Home"));
    wait_until("the enhanced mode to open when the pause ends", || {
        enhanced_on(api)
    });
    assert_eq!(actions(&file)[0], "open");
    assert_eq!(installed(&file), routes_of(false));

    world.move_to(&wifi("Office"));
    wait_until("the enhanced mode to close for the pause", || {
        !enhanced_on(api)
    });
    assert_eq!(done(&file, "close"), 1);
    assert_eq!(done(&file, "open"), 1);

    world.move_to(&wifi("Home"));
    wait_until("the enhanced mode to open again", || enhanced_on(api));
    assert_eq!(done(&file, "open"), 2, "{:?}", actions(&file));
    drop(daemon);
}

/// What is asked for during a pause is answered with success and done when
/// the pause ends; what is asked for while the pause holds it off is left
/// off when the pause ends.
#[test]
fn a_post_during_the_pause_is_noted_and_done_when_it_ends() {
    let world = World::on(&wifi("Office"));
    let conf = world.profile("", OFFICE);
    let file = world_file(&world);
    let daemon = spawn_command(run_enhanced(&world, &conf, &file, &["--enhanced-mode"]));
    let api = api_port(&daemon);
    wait_for_log(&daemon, "takeover paused on this network");

    // asked for while paused: answered, and nothing is opened
    assert_eq!(ask_for_the_enhanced_mode(api, true), 200);
    assert_eq!(
        backend_json(&file),
        Value::Null,
        "nothing was asked of the system"
    );
    assert!(!enhanced_on(api));

    // the pause ends: what was asked for is done
    world.move_to(&wifi("Home"));
    // The line comes once the enhanced mode is done, and is read here so that
    // the last `takeover resumed` of this test is the last one the daemon
    // says: one that is never read hides what the daemon did after it.
    wait_for_log(&daemon, "takeover resumed");
    wait_until("the enhanced mode to open when the pause ends", || {
        enhanced_on(api)
    });
    assert_eq!(done(&file, "open"), 1);

    // on, and the machine goes back to the paused network: it closes
    world.move_to(&wifi("Office"));
    wait_for_log(&daemon, "takeover paused on this network");
    wait_until("the enhanced mode to close for the pause", || {
        !enhanced_on(api)
    });
    assert_eq!(done(&file, "close"), 1);

    // asked to stay off while the pause holds it off: it stays off
    assert_eq!(ask_for_the_enhanced_mode(api, false), 200);
    world.move_to(&wifi("Home"));
    wait_for_log(&daemon, "takeover resumed");
    assert!(
        !enhanced_on(api),
        "it is left off, as asked during the pause"
    );
    assert_eq!(done(&file, "open"), 1, "it was not opened a second time");
    drop(daemon);
}

/// A reload leaves the routes alone except for what differs: one `routes`
/// call with what `RouteSet::diff` says, and no open, close or DNS takeover.
#[test]
fn a_reload_changes_only_the_routes_that_changed() {
    let world = World::on(&wifi("Home"));
    let conf = world.profile("", "[Rule]\nFINAL,DIRECT\n");
    let file = world_file(&world);
    let daemon = spawn_command(run_enhanced(&world, &conf, &file, &["--enhanced-mode"]));
    let api = api_port(&daemon);
    wait_until("the enhanced mode to open", || enhanced_on(api));
    let before = actions(&file);
    assert_eq!(installed(&file), routes_of(false));

    // the profile leaves one piece of the range out, and is reloaded
    let excluded = "1.2.3.0/24";
    let (add, remove) =
        RouteSet::diff(&routes_set(false, None), &routes_set(false, Some(excluded)));
    assert!(!add.is_empty() && !remove.is_empty(), "the piece is a hole");
    world.profile(
        &format!("tun-excluded-routes = {excluded}"),
        "[Rule]\nFINAL,DIRECT\n",
    );
    let (status, body) = api_call(api, "POST", "/v1/profiles/reload", "k", Some("{}"));
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"ok\":true"), "{body}");

    let next = routes_set(false, Some(excluded));
    wait_until("the routes of the reload to be installed", || {
        installed(&file) == strings(&next)
    });
    assert_eq!(
        actions(&file)[before.len()..],
        [format!("routes +{} -{}", add.len(), remove.len())],
        "one call with the difference, and nothing else"
    );
    drop(daemon);
}

/// A reload that turns IPv6 on gives the adapter its IPv6 half, adds the IPv6
/// routes and takes the DNS over again; one that turns it off gives the half
/// back, removes those routes and takes the DNS over without IPv6.
#[test]
fn a_reload_turns_ipv6_on_and_off() {
    let world = World::on(&wifi("Home"));
    let conf = world.profile("ipv6-vif = disabled", "[Rule]\nFINAL,DIRECT\n");
    let file = world_file(&world);
    let daemon = spawn_command(run_enhanced(&world, &conf, &file, &["--enhanced-mode"]));
    let api = api_port(&daemon);
    wait_until("the enhanced mode to open", || enhanced_on(api));
    assert_eq!(installed(&file), routes_of(false));
    let after_open = actions(&file).len();

    world.profile("ipv6-vif = always", "[Rule]\nFINAL,DIRECT\n");
    let (status, body) = api_call(api, "POST", "/v1/profiles/reload", "k", Some("{}"));
    assert_eq!(status, 200, "{body}");
    wait_until("IPv6 to be turned on", || {
        installed(&file) == routes_of(true) && backend_json(&file)["ipv6"] == true
    });
    assert_eq!(
        actions(&file)[after_open..],
        [
            "configure_ipv6".to_string(),
            format!("routes +{} -0", routes_set(true, None).v6.len()),
            "take_dns".to_string(),
        ],
        "the IPv6 half, its routes, and the DNS once more"
    );
    let after_ipv6 = actions(&file).len();

    world.profile("ipv6-vif = disabled", "[Rule]\nFINAL,DIRECT\n");
    let (status, body) = api_call(api, "POST", "/v1/profiles/reload", "k", Some("{}"));
    assert_eq!(status, 200, "{body}");
    wait_until("IPv6 to be turned off", || {
        installed(&file) == routes_of(false) && backend_json(&file)["ipv6"] == false
    });
    assert_eq!(
        actions(&file)[after_ipv6..],
        [
            "take_dns".to_string(),
            format!("routes +0 -{}", routes_set(true, None).v6.len()),
            "unconfigure_ipv6".to_string(),
        ],
        "the DNS given back while the address is still there, then its routes, then the half"
    );
    drop(daemon);
}

/// `ipv6-vif = auto` follows the network: the IPv6 half comes with a global
/// address and an IPv6 gateway, and goes without them.
#[test]
fn ipv6_auto_follows_the_network() {
    let world = World::on(&wifi("Home"));
    let conf = world.profile("ipv6-vif = auto", "[Rule]\nFINAL,DIRECT\n");
    let file = world_file(&world);
    let daemon = spawn_command(run_enhanced(&world, &conf, &file, &["--enhanced-mode"]));
    let api = api_port(&daemon);
    wait_until("the enhanced mode to open", || enhanced_on(api));
    assert_eq!(backend_json(&file)["ipv6"], false);
    assert_eq!(installed(&file), routes_of(false));

    // the network gains IPv6: a gateway (the link-local address a router
    // advertises from) and a global /64
    let mut with_ipv6 = wifi("Home");
    with_ipv6["primary"]["routers"] = json!(["192.0.2.1", "fe80::1"]);
    with_ipv6["addresses"] = json!(["192.0.2.10", "2001:db8:1::"]);
    // A change of network asks for the whole of what a follow does, and the
    // file backend rewrites its file once a step: wait for the last step, and
    // read the file only then (a read that meets half a file is no read).
    let before = actions(&file).len();
    let coming_on = [
        "configure_ipv6".to_string(),
        format!("routes +{} -0", routes_set(true, None).v6.len()),
        "take_dns".to_string(),
        "adjust".to_string(),
    ];
    world.move_to(&with_ipv6);
    wait_until("IPv6 to come on with the network", || {
        actions(&file).get(before..) == Some(&coming_on[..])
    });
    assert_eq!(installed(&file), routes_of(true));
    assert_eq!(backend_json(&file)["ipv6"], true);
    assert_eq!(done(&file, "configure_ipv6"), 1);
    assert_eq!(done(&file, "take_dns"), 2, "the open took it over once");

    // and loses it again: the DNS goes while its address is still there
    let before = actions(&file).len();
    let going_off = [
        "take_dns".to_string(),
        format!("routes +0 -{}", routes_set(true, None).v6.len()),
        "unconfigure_ipv6".to_string(),
        "adjust".to_string(),
    ];
    world.move_to(&wifi("Home"));
    wait_until("IPv6 to go off with the network", || {
        actions(&file).get(before..) == Some(&going_off[..])
    });
    assert_eq!(installed(&file), routes_of(false));
    assert_eq!(backend_json(&file)["ipv6"], false);
    assert_eq!(
        done(&file, "take_dns"),
        3,
        "one for each change of the IPv6 half, and no more"
    );
    assert_eq!(done(&file, "unconfigure_ipv6"), 1);
    drop(daemon);
}

/// A change that cannot be followed is logged as an error and leaves the
/// enhanced mode on, with what it has installed.
#[test]
fn a_follow_that_fails_keeps_the_enhanced_mode_on() {
    let world = World::on(&wifi("Home"));
    let conf = world.profile("", "[Rule]\nFINAL,DIRECT\n");
    let file = world_file(&world);
    let logs = world.data().join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    let log_file = logs.join("rurge.log");
    let daemon = spawn_command(run_enhanced(
        &world,
        &conf,
        &file,
        &["--enhanced-mode", "--log-file", log_file.to_str().unwrap()],
    ));
    let api = api_port(&daemon);
    wait_until("the enhanced mode to open", || enhanced_on(api));
    let before = installed(&file);

    // the system will not take the routes now, and the profile is reloaded
    let mut system = backend_json(&file);
    system["fail"] = json!("routes");
    std::fs::write(&file, system.to_string()).unwrap();
    world.profile("tun-excluded-routes = 1.2.3.0/24", "[Rule]\nFINAL,DIRECT\n");
    let (status, body) = api_call(api, "POST", "/v1/profiles/reload", "k", Some("{}"));
    assert_eq!(status, 200, "{body}");
    wait_until("the log to say the change could not be followed", || {
        log_text(&logs)
            .lines()
            .any(|line| line.contains("ERROR") && line.contains("routes"))
    });
    assert!(enhanced_on(api), "the enhanced mode goes on");
    assert_eq!(installed(&file), before, "what went in before stays");
    drop(daemon);
}

/// A close that fails has still done what the user asked: the mode is off (the
/// 500 speaks of what could not be undone), and a pause that ends does not open
/// it again (the phase 3 / M4a plan's execution record row 97).
#[test]
fn a_close_that_fails_is_still_what_the_user_asked_for() {
    let world = World::on(&wifi("Home"));
    let conf = world.profile("", OFFICE);
    let file = world_file(&world);
    let daemon = spawn_command(run_enhanced(&world, &conf, &file, &["--enhanced-mode"]));
    let api = api_port(&daemon);
    wait_until("the enhanced mode to open", || enhanced_on(api));
    assert_eq!(done(&file, "open"), 1);

    // the system will not take the DNS back, so the close cannot undo it all
    let mut system = backend_json(&file);
    system["fail_undo"] = json!("take_dns");
    std::fs::write(&file, system.to_string()).unwrap();
    assert_eq!(ask_for_the_enhanced_mode(api, false), 500);
    assert!(!enhanced_on(api), "the mode is off, whatever the 500 says");

    // into a paused network and out again: what the user closed stays closed
    world.move_to(&wifi("Office"));
    wait_for_log(&daemon, "takeover paused on this network");
    world.move_to(&wifi("Home"));
    wait_for_log(&daemon, "takeover resumed");
    assert_eq!(done(&file, "open"), 1, "it was not opened a second time");
    assert!(
        !enhanced_on(api),
        "the pause does not open what the user closed"
    );
    drop(daemon);
}
