//! rurge makes the sockets it goes out with in one place: `open_socket` in
//! `rurge-net`, which asks the platform for the network to leave by first. A
//! second place that makes one, or that asks the system resolver for a name,
//! or that builds a direct connector with no socket hook, is traffic that
//! could go back into the virtual interface while the enhanced mode is on. So
//! this test reads the production code under `crates/*/src` and holds those
//! places to a list: one row for each place it finds, and one more place than
//! the rows, or one less, is a failure.
//!
//! Adding a place that makes a socket, asks the system resolver or builds a
//! connector without a hook means adding a row to the list and saying why —
//! which function it is in, so that the row of the second place in a file is
//! not taken for the row of the first. M4c and M4d have to do the same for the
//! sockets they add for a device (the utun control socket on macOS).
//!
//! Test code is skipped. A file is read up to the line of its first inline
//! test module, `#[cfg(test)]` followed by `mod name {`: the repository's
//! tests are the end of the file, and an item that begins in the first column
//! after that is a failure, reported with its file and line. `#[cfg(test)]`
//! followed by `mod name;` — a test module in another file — does not end the
//! file: that file is read like any other, which can only add hits. A path
//! with a `tests` or `testing` directory, or a file named `testing.rs`, is
//! skipped whole: those are gated by `#[cfg(test)]` or by the `testing`
//! feature. Nothing after `//` on a line is read, so a pattern is matched in
//! code only.

use std::fs;
use std::path::{Path, PathBuf};

/// The ways of making a socket, asking for a name or building a connector
/// without a hook that the list below accounts for. A pattern is matched where
/// the character before it does not continue a name (`[A-Za-z0-9_]`), so
/// `TosSocket::new(` is not `Socket::new(` and `socket2::Socket::new(` is.
const PATTERNS: [&str; 15] = [
    "UdpSocket::bind(",
    "TcpStream::connect",
    "TcpSocket::new_v4(",
    "TcpSocket::new_v6(",
    "Socket::new(",
    "Socket::new_raw(",
    "TcpListener::bind(",
    "build_http(",
    "lookup_host(",
    "DirectConnector::new(",
    "NoopSocketHook)",
    "Endpoint::client(",
    "libc::socket(",
    "socket::socket(",
    "resource_client(",
];

/// The file, the pattern and why it is not an outbound socket that skips the
/// egress decision, in the order they sort in — one row for each place, so a
/// function that makes a second socket of the same kind has a row of its own.
///
/// Adding a row means changing this list; so does removing one.
const LISTED: [(&str, &str, &str); 24] = [
    (
        "crates/rurge-api/src/lib.rs",
        "TcpListener::bind(",
        "`serve`: the HTTP API's listener, which does not go out",
    ),
    (
        "crates/rurge-engine/src/outbounds.rs",
        "NoopSocketHook)",
        "`EngineFactory::dry`, whose empty build never dials",
    ),
    (
        "crates/rurge-engine/src/stack.rs",
        "NoopSocketHook)",
        "`resource_client()`: the commands that do not run an engine (`check` and the like) and the tests; the daemon's two download clients use `resource_client_with`",
    ),
    (
        "crates/rurge-engine/src/stack.rs",
        "resource_client(",
        "`resource_client` itself: the one place a client with no hook is built",
    ),
    (
        "crates/rurge-inbound/src/listener.rs",
        "TcpListener::bind(",
        "`run`: a listener, which does not go out",
    ),
    (
        "crates/rurge-inbound/src/socks5.rs",
        "UdpSocket::bind(",
        "the SOCKS5 UDP association's relay on the listening address, which only talks to the client locally",
    ),
    (
        "crates/rurge-net/src/connector.rs",
        "NoopSocketHook)",
        "`DirectConnector::new` itself: a direct connection with no hook; its callers are covered by the `DirectConnector::new(` rows",
    ),
    (
        "crates/rurge-net/src/connector.rs",
        "Socket::new(",
        "`new_socket`: the only way an outbound socket is made, and where the egress is asked",
    ),
    (
        "crates/rurge-net/src/connector.rs",
        "lookup_host(",
        "`SystemResolve`: the system resolver; with the enhanced mode on, rurge's own names go through `OwnResolve`",
    ),
    (
        "crates/rurge-platform/src/icmp.rs",
        "Socket::new(",
        "`open` on Linux / Android: the datagram ICMP socket, which `PlatformPinger` binds by egress",
    ),
    (
        "crates/rurge-platform/src/icmp.rs",
        "Socket::new(",
        "`open` on Linux / Android: the raw ICMP socket, when the datagram one is not allowed",
    ),
    (
        "crates/rurge-platform/src/icmp.rs",
        "Socket::new(",
        "`open` on macOS: the datagram ICMP socket",
    ),
    (
        "crates/rurge-platform/src/icmp.rs",
        "Socket::new(",
        "`open` on Windows: the raw ICMP socket",
    ),
    (
        "crates/rurge-platform/src/netenv/watch.rs",
        "Socket::new(",
        "the macOS routing socket (`AF_ROUTE`), which does not go out",
    ),
    (
        "crates/rurge-proto-wireguard/src/stack.rs",
        "Socket::new(",
        "`connect`: smoltcp's TCP socket, not the system's",
    ),
    (
        "crates/rurge-proto-wireguard/src/stack.rs",
        "Socket::new(",
        "`udp_open`: smoltcp's UDP socket, not the system's",
    ),
    (
        "crates/rurge-proto-wireguard/src/stack.rs",
        "Socket::new(",
        "`udp_bind`: smoltcp's UDP carrier socket, not the system's",
    ),
    (
        "crates/rurge-proto/src/direct.rs",
        "DirectConnector::new(",
        "`Direct::with_resolver`: only the tests call it",
    ),
    (
        "crates/rurge-proto/src/external.rs",
        "DirectConnector::new(",
        "the external program's UDP relay, on the loopback",
    ),
    (
        "crates/rurge-proto/src/external.rs",
        "TcpStream::connect",
        "connecting to the external program's port on the loopback",
    ),
    (
        "crates/rurge-tun/src/tcp/shard.rs",
        "Socket::new(",
        "smoltcp's sockets, not the system's",
    ),
    (
        "crates/rurge/src/cli/api_client.rs",
        "build_http(",
        "the process of `rurge status` / `reload` / `stop`, not the daemon",
    ),
    (
        "crates/rurge/src/cli/run.rs",
        "resource_client(",
        "`waiting`: the run that has not built its engine yet, which asks nothing of the network with it",
    ),
    (
        "crates/rurge/src/cli/runtime.rs",
        "lookup_host(",
        "`PlatformSystemDns::library_lookup`: `force-syslib` and `.local` (design 6.2)",
    ),
];

#[test]
fn every_outbound_socket_is_made_in_one_place() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("a crate directory under crates/")
        .to_path_buf();
    let root = crates.parent().expect("the workspace root").to_path_buf();
    let mut files = Vec::new();
    sources(&crates, &mut files);
    assert!(!files.is_empty(), "no source files under crates/*/src");

    let mut found: Vec<(String, String)> = Vec::new();
    for file in &files {
        let relative = file
            .strip_prefix(&root)
            .expect("a file under the workspace root")
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let text = fs::read_to_string(file).unwrap();
        match found_in(&text) {
            Ok(patterns) => found.extend(
                patterns
                    .into_iter()
                    .map(|pattern| (relative.clone(), pattern.to_string())),
            ),
            Err((line, item)) => panic!(
                "{relative}:{line}: `{}` begins in the first column after the tests, \
                 which are the end of the file",
                item.trim()
            ),
        }
    }
    found.sort();

    let mut listed: Vec<(String, String)> = LISTED
        .iter()
        .map(|(file, pattern, _)| (file.to_string(), pattern.to_string()))
        .collect();
    listed.sort();
    // One row for each place: a file with the same pattern twice needs two
    // rows, so the comparison is of the whole list over the whole list, not
    // of what each names once (the phase 3 / M4b plan's execution record row
    // 73).
    let mut left = found.clone();
    let mut missing: Vec<(String, String)> = Vec::new();
    for row in &listed {
        match left.iter().position(|found| found == row) {
            Some(at) => {
                left.remove(at);
            }
            None => missing.push(row.clone()),
        }
    }
    let extra = left;
    assert!(
        missing.is_empty() && extra.is_empty(),
        "the places that make a socket, ask for a name or build a connector without a hook \
         are not the ones on the list, one row for each\nfound but not listed: {extra:?}\n\
         listed but not found: {missing:?}"
    );
}

/// Every `.rs` file under `crates/*/src`, in a stable order, leaving out
/// `tests` and `testing` directories and `testing.rs`: those are test code.
fn sources(crates: &Path, found: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = fs::read_dir(crates)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for entry in entries {
        let src = entry.join("src");
        if src.is_dir() {
            rust_files(&src, found);
        }
    }
    found.sort();
}

fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if !is_test_directory(&path) {
                rust_files(&path, found);
            }
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            if path.file_name().is_some_and(|name| name == "testing.rs") {
                continue;
            }
            found.push(path);
        }
    }
}

fn is_test_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "tests" || name == "testing")
}

/// The patterns `text` matches, one for each place they occur.
///
/// The lines from the first inline test module on are not read, and an item
/// that begins in the first column from there is `Err` with its line and text.
fn found_in(text: &str) -> Result<Vec<&'static str>, (usize, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let tests = inline_test_module(&lines).unwrap_or(lines.len());
    if let Some((at, line)) = (tests..lines.len())
        .find(|&at| starts_an_item(&lines, at))
        .map(|at| (at, lines[at]))
    {
        return Err((at + 1, line.to_string()));
    }
    let mut found = Vec::new();
    for line in lines[..tests].iter() {
        let code = line.split_once("//").map_or(*line, |(code, _)| code);
        for pattern in PATTERNS {
            if code
                .match_indices(pattern)
                .any(|(at, _)| !continues_a_name(code, at))
            {
                found.push(pattern);
            }
        }
    }
    Ok(found)
}

/// Whether the match of a pattern at `at` is part of a longer name: the
/// character before it is `[A-Za-z0-9_]`, as the `o` of `TosSocket::new(` is.
fn continues_a_name(code: &str, at: usize) -> bool {
    code[..at]
        .chars()
        .next_back()
        .is_some_and(|before| before.is_alphanumeric() || before == '_')
}

/// The line of the first inline test module: `#[cfg(test)]` followed by
/// `mod name {`, past attributes, comments and empty lines. `#[cfg(test)]`
/// followed by `mod name;` names a test module in another file and is not one.
fn inline_test_module(lines: &[&str]) -> Option<usize> {
    let attribute = (0..lines.len()).find(|&at| {
        lines[at].trim() == "#[cfg(test)]"
            && next_code_line(lines, at).is_some_and(|next| is_inline_module(lines[next].trim()))
    })?;
    next_code_line(lines, attribute)
}

/// The first line after `at` that is not an attribute, a comment or empty.
fn next_code_line(lines: &[&str], at: usize) -> Option<usize> {
    (at + 1..lines.len()).find(|&line| {
        let text = lines[line].trim();
        !(text.is_empty() || text.starts_with("//") || text.starts_with("#["))
    })
}

/// Whether `text` declares `mod name {`, a module in this file, rather than
/// `mod name;`, one in another file, whatever its visibility.
fn is_inline_module(text: &str) -> bool {
    let mut rest = text;
    if let Some(after) = rest.strip_prefix("pub(") {
        let Some(close) = after.find(')') else {
            return false;
        };
        rest = after[close + 1..].trim_start();
    } else if let Some(after) = rest.strip_prefix("pub ") {
        rest = after.trim_start();
    }
    let Some(after) = rest.strip_prefix("mod ") else {
        return false;
    };
    let after = after.trim_start();
    let name = after
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(after.len());
    after[name..].trim_start().starts_with('{')
}

/// Whether `line`, which begins in the first column, declares an item that
/// would be production code after the tests: `fn`, `impl`, `struct`, `enum`,
/// `union`, `const`, `static`, `use`, `type`, `trait`, `mod`, `extern` or
/// `macro_rules!`, after any of `pub …`, `pub(…)`, `async`, `unsafe` or
/// `default`, and `const` before `fn`. A `mod` that follows `#[cfg(test)]` is
/// another test module.
fn starts_an_item(lines: &[&str], at: usize) -> bool {
    let line = lines[at];
    if line.is_empty()
        || line.starts_with(char::is_whitespace)
        || line.starts_with("//")
        || line.starts_with('#')
        || line.starts_with('}')
    {
        return false;
    }
    let Some(keyword) = item_keyword(line) else {
        return false;
    };
    keyword != "mod" || !follows_cfg_test(lines, at)
}

/// The keyword of the item `line` declares, in the first column, past the
/// modifiers it may begin with.
fn item_keyword(line: &str) -> Option<&str> {
    const ITEMS: [&str; 12] = [
        "fn", "impl", "struct", "enum", "union", "const", "static", "use", "type", "trait", "mod",
        "extern",
    ];
    let mut rest = line;
    loop {
        if let Some(after) = rest.strip_prefix("pub(") {
            rest = after[after.find(')')? + 1..].trim_start();
            continue;
        }
        let (word, after) = word(rest)?;
        let modifier = matches!(word, "pub" | "async" | "unsafe" | "default")
            || (word == "const" && after.trim_start().starts_with("fn "));
        if modifier {
            rest = after.trim_start();
            continue;
        }
        if word == "macro_rules" && after.starts_with('!') {
            return Some("macro_rules!");
        }
        return ITEMS.contains(&word).then_some(word);
    }
}

/// The word `text` begins with, and what follows it.
fn word(text: &str) -> Option<(&str, &str)> {
    let end = text
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    (end > 0).then(|| (&text[..end], &text[end..]))
}

/// Whether the item at `at` follows `#[cfg(test)]` past attributes, comments
/// and empty lines: another test module of this file.
fn follows_cfg_test(lines: &[&str], at: usize) -> bool {
    for line in lines[..at].iter().rev().map(|line| line.trim()) {
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if line == "#[cfg(test)]" {
            return true;
        }
        if !line.starts_with("#[") {
            return false;
        }
    }
    false
}

#[test]
fn a_pattern_is_found_only_where_a_name_does_not_continue() {
    let none: Vec<&str> = Vec::new();
    // `TosSocket::new(` is a quic crate type, not a socket of the system's
    assert_eq!(
        found_in("let tos = TosSocket::new(socket);\n"),
        Ok(none.clone())
    );
    // a path before it leaves the name alone
    assert_eq!(
        found_in("let socket = socket2::Socket::new(a, b, c)?;\n"),
        Ok(vec!["Socket::new("])
    );
    // and so does a quote, so the probe this test is held to is found
    assert_eq!(
        found_in("const _PROBE: &str = \"UdpSocket::bind(\";\n"),
        Ok(vec!["UdpSocket::bind("])
    );
    assert_eq!(found_in("let s = UdpUdpSocket::bind(x);\n"), Ok(none));
}

#[test]
fn a_pattern_after_a_comment_is_not_read() {
    let none: Vec<&str> = Vec::new();
    assert_eq!(
        found_in("// UdpSocket::bind( is not here\n"),
        Ok(none.clone())
    );
    assert_eq!(found_in("/// docs: Socket::new(\n"), Ok(none));
}

#[test]
fn an_item_after_the_tests_is_reported() {
    let text = "\
#[cfg(test)]
mod tests {
    fn inside() {}
}

fn after() {}
";
    let (line, item) = found_in(text).unwrap_err();
    assert_eq!((line, item.as_str()), (6, "fn after() {}"));
    // an `extern` item is an item too, however it is written
    for item in [
        "extern \"C\" fn after() {}",
        "extern crate after;",
        "extern \"C\" {",
    ] {
        let text = format!("#[cfg(test)]\nmod tests {{\n    fn inside() {{}}\n}}\n\n{item}\n");
        let (line, found) = found_in(&text).unwrap_err();
        assert_eq!((line, found.as_str()), (6, item), "{item}");
    }
}

#[test]
fn a_connector_without_a_hook_is_found_however_it_is_written() {
    assert_eq!(
        found_in("let hook = Arc::new(NoopSocketHook);\n"),
        Ok(vec!["NoopSocketHook)"])
    );
    assert_eq!(
        found_in("let hook = Arc::new(rurge_net::socket::NoopSocketHook);\n"),
        Ok(vec!["NoopSocketHook)"])
    );
}

#[test]
fn a_test_module_in_another_file_does_not_end_the_file() {
    let none: Vec<&str> = Vec::new();
    let text = "\
#[cfg(test)]
mod vectors;

fn after() {}
";
    assert_eq!(found_in(text), Ok(none.clone()));
    // and another test module of this file is not an item after the tests
    let text = "\
#[cfg(test)]
mod tests {
    fn inside() {}
}

#[cfg(test)]
mod fake {
    const C: &str = \"TcpListener::bind(\";
}
";
    assert_eq!(found_in(text), Ok(none));
}
