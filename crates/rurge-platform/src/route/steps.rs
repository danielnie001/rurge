//! A step is one program to run (design 5.1): what it is, what it is given,
//! and whether failing is an error. The translations in `linux`, `macos` and
//! `windows` produce these; running them is the platform layer's job
//! (M4c / M4d), so nothing here touches the system.

use std::path::PathBuf;

/// One program to run (design 5.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// An absolute path: never looked up in `PATH`.
    pub program: PathBuf,
    pub args: Vec<String>,
    /// A script the program reads: it is written to a file of its own,
    /// whose path takes the place of [`SCRIPT`] in `args`.
    pub script: Option<String>,
    pub on_failure: OnFailure,
}

/// Where a script's path goes in `args`.
pub const SCRIPT: &str = "{script}";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnFailure {
    /// The open fails, and what was done is undone.
    Fail,
    /// The IPv6 half: a warning, and this open goes on as if IPv6 were off
    /// (M4-D17).
    Ipv6Off,
    /// Undoing goes on: removing routes, giving back what went with the
    /// device.
    Ignore,
}

/// The text of a script a program reads: one command on each line, every line
/// — the last one too — ending in a newline, and no carriage return.
pub(crate) fn script(lines: &[String]) -> String {
    let mut text = String::new();
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    text
}
