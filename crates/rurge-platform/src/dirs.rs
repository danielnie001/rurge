//! Default data / config directories (PRD §6 platform matrix). The rules take
//! an environment lookup and an explicit `Os` so every platform's behaviour is
//! unit-tested on every host.

use std::ffi::OsString;
use std::path::PathBuf;

/// Environment lookup; production passes `std::env::var_os`.
pub type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<OsString>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    Unix,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Unix
        }
    }
}

/// `%LOCALAPPDATA%\rurge` / `~/Library/Application Support/rurge` / `$XDG_DATA_HOME/rurge`.
pub fn data_dir() -> PathBuf {
    data_dir_for(Os::current(), &|k| std::env::var_os(k))
}

/// `%APPDATA%\rurge` / `~/Library/Application Support/rurge/profiles` / `$XDG_CONFIG_HOME/rurge`.
pub fn config_dir() -> PathBuf {
    config_dir_for(Os::current(), &|k| std::env::var_os(k))
}

pub fn data_dir_for(os: Os, env: EnvLookup<'_>) -> PathBuf {
    match os {
        Os::Windows => var_or(env, "LOCALAPPDATA", &["AppData", "Local"]).join("rurge"),
        Os::MacOs => home(env)
            .join("Library")
            .join("Application Support")
            .join("rurge"),
        Os::Unix => var_or(env, "XDG_DATA_HOME", &[".local", "share"]).join("rurge"),
    }
}

pub fn config_dir_for(os: Os, env: EnvLookup<'_>) -> PathBuf {
    match os {
        Os::Windows => var_or(env, "APPDATA", &["AppData", "Roaming"]).join("rurge"),
        Os::MacOs => home(env)
            .join("Library")
            .join("Application Support")
            .join("rurge")
            .join("profiles"),
        Os::Unix => var_or(env, "XDG_CONFIG_HOME", &[".config"]).join("rurge"),
    }
}

fn home(env: EnvLookup<'_>) -> PathBuf {
    env("HOME")
        .or_else(|| env("USERPROFILE"))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `$var` when set and non-empty, otherwise `home/fallback[0]/fallback[1]/…`.
fn var_or(env: EnvLookup<'_>, var: &str, fallback: &[&str]) -> PathBuf {
    match env(var).filter(|v| !v.is_empty()) {
        Some(v) => PathBuf::from(v),
        None => fallback.iter().fold(home(env), |p, s| p.join(s)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn windows_uses_localappdata_and_appdata() {
        let env = env_of(&[
            ("LOCALAPPDATA", "C:\\Users\\u\\AppData\\Local"),
            ("APPDATA", "C:\\Users\\u\\AppData\\Roaming"),
        ]);
        assert_eq!(
            data_dir_for(Os::Windows, &env),
            PathBuf::from("C:\\Users\\u\\AppData\\Local").join("rurge")
        );
        assert_eq!(
            config_dir_for(Os::Windows, &env),
            PathBuf::from("C:\\Users\\u\\AppData\\Roaming").join("rurge")
        );
    }

    #[test]
    fn windows_falls_back_to_userprofile() {
        let env = env_of(&[("USERPROFILE", "C:\\Users\\u")]);
        assert_eq!(
            data_dir_for(Os::Windows, &env),
            PathBuf::from("C:\\Users\\u")
                .join("AppData")
                .join("Local")
                .join("rurge")
        );
    }

    #[test]
    fn macos_uses_application_support() {
        let env = env_of(&[("HOME", "/Users/u")]);
        assert_eq!(
            data_dir_for(Os::MacOs, &env),
            PathBuf::from("/Users/u/Library/Application Support/rurge")
        );
        assert_eq!(
            config_dir_for(Os::MacOs, &env),
            PathBuf::from("/Users/u/Library/Application Support/rurge/profiles")
        );
    }

    #[test]
    fn unix_prefers_xdg_and_falls_back_to_dotdirs() {
        let env = env_of(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "/data")]);
        assert_eq!(data_dir_for(Os::Unix, &env), PathBuf::from("/data/rurge"));
        assert_eq!(
            config_dir_for(Os::Unix, &env),
            PathBuf::from("/home/u/.config/rurge")
        );
        let env = env_of(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "")]);
        assert_eq!(
            data_dir_for(Os::Unix, &env),
            PathBuf::from("/home/u/.local/share/rurge")
        );
    }

    #[test]
    fn current_os_functions_return_something_under_a_home() {
        assert!(data_dir().ends_with("rurge") || data_dir().ends_with("profiles"));
        assert!(config_dir().components().count() >= 2);
    }
}
