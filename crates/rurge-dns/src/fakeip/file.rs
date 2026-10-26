//! The file `fake-ip.txt` (M3 design §10.3): what the pool keeps between runs.
//!
//! ```text
//! rurge-fake-ip 1
//! cursor <n> wrapped <0|1> clean <0|1>
//! <index> <name>
//! ```
//!
//! then one line like the last for every index that has a name: those nobody
//! holds a lease on first, the one used longest ago leading, then those that
//! are leased. Reading is strict, and the reason it gives for refusing a file
//! quotes nothing of it: the names in it are not for a log.

use super::normalize;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAGIC: &str = "rurge-fake-ip 1";

/// What the file holds, apart from whether the run it belongs to ended.
pub(super) struct Table {
    pub(super) cursor: u32,
    pub(super) wrapped: bool,
    /// An index and its name; the one to be recycled first leads.
    pub(super) entries: Vec<(u32, Arc<str>)>,
}

/// The text of the file for `table`; `clean` says the run ended.
pub(super) fn render(table: &Table, clean: bool) -> String {
    let mut text = String::with_capacity(64 + table.entries.len() * 32);
    text.push_str(MAGIC);
    text.push('\n');
    let _ = writeln!(
        text,
        "cursor {} wrapped {} clean {}",
        table.cursor,
        u8::from(table.wrapped),
        u8::from(clean)
    );
    for (index, name) in &table.entries {
        let _ = writeln!(text, "{index} {name}");
    }
    text
}

/// Reads the file's bytes for a pool of `count` indices: the table and
/// whether the run ended. A line is ended by a line feed and nothing else; one
/// blank line at the end is let through. The error says what is wrong, by
/// line number, without quoting any of the file.
pub(super) fn parse(bytes: &[u8], count: u32) -> Result<(Table, bool), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "the file is not UTF-8".to_string())?;
    let mut lines: Vec<&str> = text.split('\n').collect();
    // the last line's own line feed leaves an empty piece; so may one more
    for _ in 0..2 {
        if lines.last() == Some(&"") {
            lines.pop();
        }
    }
    if lines.first() != Some(&MAGIC) {
        return Err("the first line is not `rurge-fake-ip 1`".to_string());
    }
    let Some((cursor, wrapped, clean)) = lines.get(1).and_then(|line| header(line)) else {
        return Err("the second line is not `cursor <n> wrapped <0|1> clean <0|1>`".to_string());
    };
    if cursor > count {
        return Err("the cursor is past the last address".to_string());
    }
    // no valid file has more lines than there are indices
    let room = (lines.len() - 2).min(count as usize);
    let mut entries = Vec::with_capacity(room);
    let mut index_seen = vec![false; count as usize];
    let mut name_seen = HashSet::with_capacity(room);
    for (at, line) in lines.iter().enumerate().skip(2) {
        let at = at + 1;
        let Some((index, name)) = line
            .split_once(' ')
            .and_then(|(index, name)| Some((number(index)?, name)))
        else {
            return Err(format!("line {at} is not `<number> <name>`"));
        };
        if index >= count || (!wrapped && index >= cursor) {
            return Err(format!("line {at}: the address number is out of range"));
        }
        if std::mem::replace(&mut index_seen[index as usize], true) {
            return Err(format!("line {at}: the address number comes twice"));
        }
        if normalize(name).as_deref() != Some(name) {
            return Err(format!("line {at}: the name is not in its normalized form"));
        }
        if !name_seen.insert(name) {
            return Err(format!("line {at}: the name comes twice"));
        }
        entries.push((index, Arc::from(name)));
    }
    let table = Table {
        cursor,
        wrapped,
        entries,
    };
    Ok((table, clean))
}

/// `cursor <n> wrapped <0|1> clean <0|1>`.
fn header(line: &str) -> Option<(u32, bool, bool)> {
    let words: Vec<&str> = line.split(' ').collect();
    let ["cursor", cursor, "wrapped", wrapped, "clean", clean] = words[..] else {
        return None;
    };
    Some((number(cursor)?, flag(wrapped)?, flag(clean)?))
}

fn flag(word: &str) -> Option<bool> {
    match word {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

/// A number written with digits and nothing else.
fn number(word: &str) -> Option<u32> {
    if word.is_empty() || !word.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    word.parse().ok()
}

/// Writes `text` to the file: to a temporary file beside it first and then
/// renamed over it, so that a reader finds the old text or the new, and
/// without waiting for the disk. The directory is made if it is not there.
/// On Unix the file belongs to its owner alone: it lists the names
/// applications have asked for.
pub(super) fn write(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    write_temporary(&temporary, text)?;
    std::fs::rename(&temporary, path)
}

/// Writes `text` to `temporary`, which the rename then puts in place: on Unix
/// as its owner's alone, since it lists the names applications have asked
/// for.
#[cfg(unix)]
fn write_temporary(temporary: &Path, text: &str) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    // `mode` reaches only a file this call makes: one that a run which ended
    // between the write and the rename left there keeps its own permissions,
    // and the rename would carry them over.
    if temporary.exists() {
        std::fs::set_permissions(temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(temporary)?;
    file.write_all(text.as_bytes())
}

/// `write_temporary` on a platform without mode bits: the permissions of the
/// data directory are the file's (Windows).
#[cfg(not(unix))]
fn write_temporary(temporary: &Path, text: &str) -> io::Result<()> {
    std::fs::write(temporary, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(cursor: u32, wrapped: bool, entries: &[(u32, &str)]) -> Table {
        Table {
            cursor,
            wrapped,
            entries: entries.iter().map(|&(i, n)| (i, Arc::from(n))).collect(),
        }
    }

    #[test]
    fn a_table_is_read_back_as_it_was_written() {
        let written = table(3, false, &[(2, "b.test"), (0, "a.test")]);
        let text = render(&written, true);
        assert_eq!(
            text,
            "rurge-fake-ip 1\ncursor 3 wrapped 0 clean 1\n2 b.test\n0 a.test\n"
        );
        let (read, clean) = parse(text.as_bytes(), 16).unwrap();
        assert!(clean);
        assert_eq!((read.cursor, read.wrapped), (3, false));
        assert_eq!(read.entries, written.entries);

        let (read, clean) = parse(render(&table(16, true, &[]), false).as_bytes(), 16).unwrap();
        assert!(!clean);
        assert_eq!((read.cursor, read.wrapped), (16, true));
        assert!(read.entries.is_empty());
    }

    #[test]
    fn the_end_of_the_file_may_have_one_blank_line() {
        let head = "rurge-fake-ip 1\ncursor 1 wrapped 0 clean 1\n0 a.test";
        for (tail, readable) in [("", true), ("\n", true), ("\n\n", true), ("\n\n\n", false)] {
            let text = format!("{head}{tail}");
            assert_eq!(parse(text.as_bytes(), 16).is_ok(), readable, "{tail:?}");
        }
    }

    #[test]
    fn a_line_ends_with_a_line_feed_and_nothing_else() {
        let text = "rurge-fake-ip 1\r\ncursor 1 wrapped 0 clean 1\r\n0 a.test\r\n";
        assert!(parse(text.as_bytes(), 16).is_err());
        let text = "rurge-fake-ip 1\ncursor 1 wrapped 0 clean 1\n0 a.test\r\n";
        assert!(parse(text.as_bytes(), 16).is_err());
    }

    #[test]
    fn a_number_is_digits_and_nothing_else() {
        for bad in ["+1", "-1", " 1", "0x1", "1e0", "", "4294967296"] {
            let text = format!("rurge-fake-ip 1\ncursor {bad} wrapped 0 clean 1\n");
            assert!(parse(text.as_bytes(), 16).is_err(), "cursor {bad:?}");
            let text = format!("rurge-fake-ip 1\ncursor 4 wrapped 0 clean 1\n{bad} a.test\n");
            assert!(parse(text.as_bytes(), 16).is_err(), "index {bad:?}");
        }
    }

    #[test]
    fn the_second_line_is_exactly_three_pairs() {
        for bad in [
            "cursor 1 wrapped 0 clean 1 extra 1",
            "cursor 1 wrapped 0 clean",
            "cursor  1 wrapped 0 clean 1",
            "wrapped 0 cursor 1 clean 1",
            "cursor 1 wrapped 0 clean 2",
            "cursor 1 wrapped true clean 1",
            "",
        ] {
            let text = format!("rurge-fake-ip 1\n{bad}\n");
            assert!(parse(text.as_bytes(), 16).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_reason_quotes_nothing_of_the_file() {
        for text in [
            "rurge-fake-ip 1\ncursor 1 wrapped 0 clean 1\nsecret-a.test\n",
            "rurge-fake-ip 1\ncursor 2 wrapped 0 clean 1\n0 secret-a.test\n0 secret-b.test\n",
            "rurge-fake-ip 1\ncursor 2 wrapped 0 clean 1\n0 secret-a.test\n1 secret-a.test\n",
            "rurge-fake-ip 1\ncursor 1 wrapped 0 clean 1\n0 Secret-a.test\n",
            "rurge-fake-ip 1\ncursor 1 wrapped 0 clean 1\n5 secret-a.test\n",
            "rurge-fake-ip 1\ncursor 1 wrapped 0 clean 1\n0 secret a.test\n",
            "secret first line\n",
            "rurge-fake-ip 1\nsecret second line\n",
        ] {
            let Err(reason) = parse(text.as_bytes(), 16) else {
                panic!("read: {text:?}");
            };
            assert!(!reason.contains("secret"), "{reason}");
        }
    }

    /// The names in the file are the ones applications have asked for: on
    /// Unix only its owner may read it, also when a temporary file that a run
    /// ended between the write and the rename left there is written again.
    #[cfg(unix)]
    #[test]
    fn the_pool_file_is_its_owners_alone() {
        use std::os::unix::fs::PermissionsExt;

        fn mode_of(path: &Path) -> u32 {
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fake-ip.txt");
        write(&path, "rurge-fake-ip 1\n").unwrap();
        assert_eq!(mode_of(&path), 0o600, "the file is its owner's alone");

        // What a run that ended between the write and the rename left: the
        // write's own mode does not reach a file that is there already, and
        // the rename would carry 0644 over.
        let temporary = dir.path().join("fake-ip.txt.tmp");
        std::fs::write(&temporary, "old").unwrap();
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(mode_of(&temporary), 0o644, "the file left there");
        write(&path, "rurge-fake-ip 1\ncursor 0 wrapped 0 clean 1\n").unwrap();
        assert_eq!(mode_of(&path), 0o600, "the rename carried 0644 over");
        assert!(!temporary.exists(), "the temporary file is renamed");
    }
}
