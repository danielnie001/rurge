//! The workspace forbids `unsafe` code; this crate opts out for the FFI calls
//! it has to make, one function at a time, with `#[allow(unsafe_code)]`. This
//! test reads the crate's own source and holds those functions to a list: one
//! more, one less, or the attribute on anything that is not a function, is a
//! failure. So is a line that names `unsafe_code` outside a `//` comment and is
//! not that attribute on a line of its own, which is how an attribute broken
//! over several lines would otherwise go unseen.

use std::fs;
use std::path::{Path, PathBuf};

/// The functions that carry `#[allow(unsafe_code)]`: the file under the crate's
/// directory, and the name of the function, in the order they sort in.
///
/// Adding a function that opts out of the lint means changing this list, and
/// saying so in the commit message and in the `unsafe_code` paragraph of
/// CLAUDE.md.
const LISTED: [(&str, &str); 13] = [
    ("src/process.rs", "in_job"),
    ("src/process.rs", "job_for"),
    ("src/procinfo/macos.rs", "pcblist"),
    ("src/procinfo/macos.rs", "pid_path"),
    ("src/procinfo/windows.rs", "image_path"),
    ("src/procinfo/windows.rs", "tcp_table"),
    ("src/procinfo/windows.rs", "udp_table"),
    ("src/socket/mod.rs", "unicast_if"),
    ("src/sysproxy/windows.rs", "notify_wininet"),
    ("src/tun/linux.rs", "set_iff"),
    ("src/tun/macos.rs", "utun_connect"),
    ("src/tun/windows/wintun.rs", "load"),
    ("src/tun/windows/wintun.rs", "verify_signer"),
];

#[test]
fn every_unsafe_exception_is_on_the_list() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    assert!(!files.is_empty(), "no source files under src/");

    let mut found: Vec<(String, String)> = Vec::new();
    for file in &files {
        let relative = file
            .strip_prefix(root)
            .unwrap()
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let text = fs::read_to_string(file).unwrap();
        match exceptions(&text) {
            Ok(names) => found.extend(names.into_iter().map(|name| (relative.clone(), name))),
            Err(problem) => panic!("{relative}: {problem}"),
        }
    }
    found.sort();

    let listed: Vec<(String, String)> = LISTED
        .iter()
        .map(|(file, name)| (file.to_string(), name.to_string()))
        .collect();
    assert_eq!(
        found, listed,
        "the functions that opt out of `unsafe_code` are not the ones on the list"
    );
}

/// Every `.rs` file under `dir`.
fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

/// The names of the functions in `text` that carry `#[allow(unsafe_code)]`.
///
/// A line is read as code up to a `//`, and what follows is a comment. A line
/// whose code names `unsafe_code` has to be, trimmed, exactly
/// `#[allow(unsafe_code)]`, and that attribute labels the function after it.
/// `Err` says which line is not that: the attribute written otherwise (an
/// inner one, one inside `cfg_attr`, one with other lints in it, one over
/// several lines, whatever else is on the line with it), or on something other
/// than a function. A block comment or a string that names the lint is refused
/// like any other line: a mention goes in a `//` comment.
fn exceptions(text: &str) -> Result<Vec<String>, String> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let mut names = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        let number = at + 1;
        let code = line.split_once("//").map_or(*line, |(code, _)| code);
        if !code.contains("unsafe_code") {
            continue;
        }
        if *line != "#[allow(unsafe_code)]" {
            return Err(format!(
                "line {number}: `{line}` is not exactly `#[allow(unsafe_code)]`"
            ));
        }
        // the item it is on: the first line that is not an attribute, a
        // comment or empty
        let item = lines[at + 1..]
            .iter()
            .find(|line| !(line.is_empty() || line.starts_with("//") || is_attribute(line)));
        match item.and_then(|item| function_name(item)) {
            Some(name) => names.push(name.to_string()),
            None => {
                return Err(format!(
                    "line {number}: `#[allow(unsafe_code)]` is not followed by a function"
                ));
            }
        }
    }
    Ok(names)
}

fn is_attribute(line: &str) -> bool {
    line.starts_with("#[") || line.starts_with("#![")
}

/// The name of the function a line declares, whatever its visibility: `fn f`,
/// `pub fn f`, `pub(crate) fn f`, `pub(super) fn f`, `pub(in path) fn f`.
fn function_name(line: &str) -> Option<&str> {
    let rest = match line.strip_prefix("pub") {
        None => line,
        Some(rest) if rest.starts_with(' ') => rest,
        Some(rest) if rest.starts_with('(') => &rest[rest.find(')')? + 1..],
        Some(_) => return None,
    };
    let rest = rest.trim_start().strip_prefix("fn ")?.trim_start();
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    Some(&rest[..end]).filter(|name| !name.is_empty())
}

#[test]
fn the_function_is_found_past_other_attributes_comments_and_empty_lines() {
    let text = "\
/// Docs.
#[cfg(windows)]
#[allow(unsafe_code)]
// a comment

#[inline]
fn first(x: u32) -> u32 {
    x
}

    #[allow(unsafe_code)]
    pub(super) fn second() {}
";
    assert_eq!(exceptions(text), Ok(vec!["first".into(), "second".into()]));
}

#[test]
fn every_visibility_is_accepted() {
    for (line, name) in [
        ("fn a() {", "a"),
        ("pub fn b<T>(t: T) {", "b"),
        ("pub(crate) fn c_1() {", "c_1"),
        ("pub(super) fn d() {", "d"),
        ("pub(in crate::x) fn e() {", "e"),
    ] {
        let text = format!("#[allow(unsafe_code)]\n{line}\n");
        assert_eq!(exceptions(&text), Ok(vec![name.to_string()]), "{line}");
    }
}

#[test]
fn text_that_does_not_mention_the_lint_has_no_exceptions() {
    let text = "\
// #[allow(unsafe_code)] in a comment
/// and in a doc comment: #[allow(unsafe_code)]
//! and in an inner one: unsafe_code
let note = 1; // unsafe_code, after code
#[allow(dead_code)]
fn f() {}
";
    assert_eq!(exceptions(text), Ok(Vec::new()));
}

#[test]
fn a_line_that_names_the_lint_and_is_not_the_attribute_is_refused() {
    // What rustfmt makes of an attribute with a long list of lints: the lint
    // is on a line that does not begin with `#[`. That line is the one
    // reported.
    let several_lines = [
        (
            "fn before() {}\n\n#[allow(\n    unsafe_code,\n    dead_code,\n)]\nfn f() {}\n",
            4,
        ),
        (
            "#[allow(\n    dead_code,\n    unsafe_code,\n)]\nfn f() {}\n",
            3,
        ),
        ("#[allow(\n    unsafe_code\n)]\nfn f() {}\n", 2),
    ];
    for (text, line) in several_lines {
        let problem = exceptions(text).unwrap_err();
        assert!(
            problem.starts_with(&format!("line {line}: ")) && problem.contains("is not exactly"),
            "{text:?}: {problem}"
        );
    }
    // Any line that names it outside a `//` comment is refused, whatever it
    // is: a string, or the text of a block comment.
    for line in ["let note = \"unsafe_code\";", "/* #[allow(unsafe_code)] */"] {
        let text = format!("fn before() {{}}\n{line}\n#[allow(unsafe_code)]\nfn f() {{}}\n");
        let problem = exceptions(&text).unwrap_err();
        assert!(
            problem.starts_with("line 2: ") && problem.contains("is not exactly"),
            "{line}: {problem}"
        );
    }
}

#[test]
fn the_attribute_on_anything_but_a_function_is_refused() {
    for item in [
        "struct S;",
        "impl S {",
        "mod m {",
        "const C: u32 = 0;",
        "unsafe impl Send for S {}",
        "pubfn()",
        "pub struct S;",
        "let x = 1;",
    ] {
        let text = format!("#[allow(unsafe_code)]\n{item}\n");
        let problem = exceptions(&text).unwrap_err();
        assert!(
            problem.contains("is not followed by a function"),
            "{item}: {problem}"
        );
    }
    // nothing after it at all
    assert!(exceptions("#[allow(unsafe_code)]\n\n// end\n").is_err());
}

#[test]
fn the_attribute_written_in_another_way_is_refused() {
    for line in [
        "#![allow(unsafe_code)]",
        "#[cfg_attr(windows, allow(unsafe_code))]",
        "#[allow(unsafe_code, dead_code)]",
        "#[allow(dead_code, unsafe_code)]",
        "#[expect(unsafe_code)]",
        "#[deny(unsafe_code)]",
        "#[forbid(unsafe_code)]",
        "#[allow(unsafe_code)] // reason",
    ] {
        let text = format!("{line}\nfn f() {{}}\n");
        let problem = exceptions(&text).unwrap_err();
        assert!(problem.contains("is not exactly"), "{line}: {problem}");
    }
}
