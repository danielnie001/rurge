//! Where `wintun.dll` is (M4 design 8.4): `--wintun-dll` (`RURGE_WINTUN_DLL`)
//! made absolute, or next to the executable, which is where the file that goes
//! with rurge sits.
//!
//! Nothing here belongs to one platform: the answer is a path on all of them,
//! and Windows is the one that opens it. On the others the option is a usage
//! error rather than a path (M4 design 9.6), so nothing else reads this.

use std::io;
use std::path::{Path, PathBuf};

/// What the error says when the caller cannot say where `rurge.exe` is: the
/// file is looked for next to it, so without that the answer has to come from
/// the option.
const CANNOT_TELL_WHERE_RURGE_IS: &str =
    "cannot tell where rurge.exe is, to find wintun.dll next to it: give --wintun-dll";

/// What the error says when a relative option cannot be made absolute: the
/// current directory is what it is relative to, and the loader wants a fully
/// qualified path (plan P7).
const CANNOT_TELL_WHERE_WE_ARE: &str = "cannot tell where the current directory is, to make --wintun-dll absolute: \
     give an absolute path";

/// Where `wintun.dll` is: `option` (`--wintun-dll`) made absolute, or next to
/// the executable — `exe` is where the caller found it, or why it could not.
///
/// The answer is always absolute and may well not exist: nothing is opened or
/// looked at here. The path is not in either error (plan P16): it can carry
/// the account's name, and the error can end up in an API reply.
pub fn wintun_dll(option: Option<&Path>, exe: io::Result<PathBuf>) -> Result<PathBuf, String> {
    match option {
        Some(path) => absolute(path),
        None => {
            let exe = exe.map_err(|_| CANNOT_TELL_WHERE_RURGE_IS.to_string())?;
            match exe.parent() {
                Some(dir) => absolute(&dir.join("wintun.dll")),
                None => Err(CANNOT_TELL_WHERE_RURGE_IS.to_string()),
            }
        }
    }
}

/// `path` with the current directory in front of it when it has none: what the
/// loader needs to search that path and nothing else.
fn absolute(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let current = std::env::current_dir().map_err(|_| CANNOT_TELL_WHERE_WE_ARE.to_string())?;
    Ok(current.join(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::io::ErrorKind;

    /// The fully qualified path of a relative one: the current directory is
    /// what it is relative to.
    fn under_the_current_directory(relative: &str) -> PathBuf {
        env::current_dir()
            .expect("a current directory")
            .join(relative)
    }

    /// A relative `--wintun-dll` is made absolute, because the loader searches
    /// only the fully qualified path it is given (plan P7).
    #[test]
    fn a_relative_option_becomes_absolute() {
        let path = wintun_dll(
            Some(Path::new("wintun.dll")),
            Err(io::Error::from(ErrorKind::NotFound)),
        )
        .expect("a relative path is answered");

        assert!(path.is_absolute(), "{}", path.display());
        assert_eq!(path, under_the_current_directory("wintun.dll"));
    }

    /// An absolute one is kept as given, whether or not it exists.
    #[test]
    fn an_absolute_option_is_kept() {
        let given = env::temp_dir().join("rurge-test").join("wintun.dll");
        let path = wintun_dll(Some(&given), Err(io::Error::from(ErrorKind::NotFound)))
            .expect("an absolute path is answered");

        assert_eq!(path, given);
    }

    /// Without the option, the file is next to the executable.
    #[test]
    fn wintun_dll_is_next_to_the_executable() {
        let exe = env::temp_dir().join("rurge").join("rurge.exe");
        let path = wintun_dll(None, Ok(exe)).expect("the executable's directory is known");

        assert_eq!(path, env::temp_dir().join("rurge").join("wintun.dll"));
        assert!(path.is_absolute(), "{}", path.display());
    }

    /// Not knowing where the executable is says what to do about it.
    #[test]
    fn not_knowing_where_rurge_exe_is_is_said() {
        let error = wintun_dll(None, Err(io::Error::from(ErrorKind::NotFound)))
            .expect_err("no executable, no answer");

        assert_eq!(
            error,
            "cannot tell where rurge.exe is, to find wintun.dll next to it: give --wintun-dll"
        );
    }
}
