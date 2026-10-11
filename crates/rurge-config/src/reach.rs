//! What the text a line stands in may make rurge do on this machine.
//!
//! Where a text came from decides it, never what the text says about itself:
//! the user's own profile and the local files it takes in may name any local
//! file and start programs; the downloaded copy of a managed profile, and the
//! local files it takes in, may take in a local file below the profile's
//! directory and nothing more; any other downloaded text names nothing on
//! this machine. A text never gains rights by being taken in by another.

/// What the text a line stands in may make rurge do on this machine: which
/// local files it may name, and whether a program it names is started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// The user's own text. Any local file, by any path; its `external`
    /// policies are started.
    Anywhere,
    /// The downloaded copy of a managed profile, and every local file it
    /// takes in. It may `#!include` a local file by a plain relative path,
    /// which stays below the local profile's directory. It names no other
    /// local file and starts no program.
    BelowProfile,
    /// Any other downloaded text: nothing on this machine.
    Nowhere,
}

/// What is said of a line that asks for more than its text may: the warning
/// and what to do about it. Neither quotes anything of the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub(crate) message: &'static str,
    pub(crate) hint: &'static str,
}

impl Reach {
    /// What is said when text of this reach `#!include`s the local file
    /// `target`, if it may not. The line is ignored.
    pub(crate) fn include_refusal(self, target: &str) -> Option<Refusal> {
        match self {
            Reach::Anywhere => None,
            Reach::BelowProfile if is_plain_relative(target) => None,
            Reach::BelowProfile => Some(Refusal {
                message: "the downloaded copy of a managed profile (or a file it includes) can include a local file only by a relative path below the profile's directory; the line is ignored",
                hint: "name the file by a relative path without a drive, a leading slash or `..`",
            }),
            Reach::Nowhere => Some(Refusal {
                message: "a downloaded file cannot include a local file; the line is ignored",
                hint: "include the local file from the local profile instead",
            }),
        }
    }

    /// What is said when text of this reach names a local file as a set, if it
    /// may not. The message is the reason; whoever drops the line says so.
    pub(crate) fn set_refusal(self) -> Option<Refusal> {
        match self {
            Reach::Anywhere => None,
            Reach::BelowProfile => Some(Refusal {
                message: "the downloaded copy of a managed profile (or a file it includes) cannot name a local file as a set",
                hint: "name the set by a URL instead",
            }),
            Reach::Nowhere => Some(Refusal {
                message: "a downloaded file cannot name a local file as a set",
                hint: "write the line in the local profile to use a local set",
            }),
        }
    }

    /// What is said when the group a text of this reach defines names a local
    /// file as its `policy-path`, if it may not. The message follows the
    /// group's name.
    pub(crate) fn policy_path_refusal(self) -> Option<Refusal> {
        match self {
            Reach::Anywhere => None,
            Reach::BelowProfile => Some(Refusal {
                message: "the downloaded copy of a managed profile (or a file it includes) cannot name a local file as `policy-path`; the parameter is ignored",
                hint: "give `policy-path` a URL instead",
            }),
            Reach::Nowhere => Some(Refusal {
                message: "a downloaded file cannot name a local file as `policy-path`; the parameter is ignored",
                hint: "define the group in the local profile to import policies from a local file",
            }),
        }
    }
}

/// Whether `target`, as it is written, is a plain relative path: one that,
/// joined to a directory, names something below it on every platform.
///
/// The target is cut at every `/` and every `\`, whatever separator the
/// platform has, and every piece
///
/// - is not empty (so there is no root, no UNC path, no `\\?\` form, and no
///   doubled or trailing separator),
/// - does not end in a dot or a space, which includes `..` (Windows drops
///   them, so `.. ` and `...` name the parent and the directory itself there);
///   the piece `.` alone is allowed,
/// - holds none of `"*:<>?|` and no control character (so there is no drive,
///   no stream name and no wildcard), and
/// - is not the name of a device of Windows.
///
/// The rule is about the text: it does not ask the platform and gives the
/// same answer on all of them. Nothing is expanded; `~` is a name.
pub(crate) fn is_plain_relative(target: &str) -> bool {
    !target.is_empty() && target.split(['/', '\\']).all(is_plain_piece)
}

fn is_plain_piece(piece: &str) -> bool {
    if piece.is_empty() {
        return false;
    }
    // the one piece that may end in a dot
    if piece == "." {
        return true;
    }
    // `..` ends in a dot
    if piece.ends_with(['.', ' ']) {
        return false;
    }
    if piece
        .chars()
        .any(|c| matches!(c, '"' | '*' | ':' | '<' | '>' | '?' | '|') || c <= '\u{1f}')
    {
        return false;
    }
    !is_device_name(piece)
}

/// The names Windows gives to devices, whatever follows them: `nul.txt` is
/// the device, and so is `aux .dconf`. The rule is the one Python's
/// `ntpath.isreserved` applies to a file name, plus `COM0` and `LPT0`, which
/// Microsoft's "Naming Files, Paths, and Namespaces" lists.
fn is_device_name(piece: &str) -> bool {
    let stem = piece
        .split('.')
        .next()
        .unwrap_or(piece)
        .trim_end_matches(' ');
    if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"]
        .iter()
        .any(|name| stem.eq_ignore_ascii_case(name))
    {
        return true;
    }
    ["COM", "LPT"].iter().any(|port| {
        let Some((head, number)) = stem.split_at_checked(port.len()) else {
            return false;
        };
        let mut number = number.chars();
        head.eq_ignore_ascii_case(port)
            && matches!(
                (number.next(), number.next()),
                (Some('0'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'), None)
            )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADMITTED: &[&str] = &[
        "a.list",
        "sets/a.list",
        "sets\\a.list",
        "./a.list",
        "sets/./a.list",
        "a b.list",
        ".hidden",
        "a..b/c.list",
        "...x/a.list",
        "~/a.list",
        "\u{89c4}\u{5219}/\u{5217}\u{8868}.list",
        "console.dconf",
        "com10",
        "nul1",
        "a.nul",
        "con-fig/x",
    ];

    const REFUSED: &[&str] = &[
        "",
        "/a.list",
        "\\a.list",
        "//host/share/a.list",
        "\\\\host\\share\\a.list",
        "\\\\?\\C:\\a.list",
        "\\\\.\\pipe\\x",
        "C:\\a.list",
        "C:/a.list",
        "C:a.list",
        "c:",
        "..",
        "../a.list",
        "..\\a.list",
        "sets/../a.list",
        "sets/..",
        "sets/.../a.list",
        "sets/.. /a.list",
        "sets/ /a.list",
        "sets /a.list",
        "sets./a.list",
        "sets//a.list",
        "sets/",
        "sets\\",
        "a.list.",
        "a.list:stream",
        "file:///a.list",
        "a?.list",
        "a*.list",
        "a|b",
        "a<b",
        "a>b",
        "a\"b",
        "a\tb.list",
        "con",
        "NUL",
        "nul.txt",
        "sets/aux",
        "aux .dconf",
        "COM1",
        "com\u{b9}",
        "LPT9.dconf",
        "lpt0",
        "CONIN$",
        "conout$",
    ];

    /// The same answer on every platform: the rule is about the text, and the
    /// table is the proof (no `cfg`, no `Path`).
    #[test]
    fn a_plain_relative_path_is_told_from_every_other_on_every_platform() {
        let refused_wrongly: Vec<&str> = ADMITTED
            .iter()
            .copied()
            .filter(|target| !is_plain_relative(target))
            .collect();
        let admitted_wrongly: Vec<&str> = REFUSED
            .iter()
            .copied()
            .filter(|target| is_plain_relative(target))
            .collect();
        assert!(
            refused_wrongly.is_empty() && admitted_wrongly.is_empty(),
            "refused, though plain: {refused_wrongly:?}\nadmitted, though not: {admitted_wrongly:?}"
        );
    }
}
