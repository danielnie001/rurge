//! Windows: the per-user Internet Settings key plus a WinINet refresh.

use super::{Backup, ProxySettings, SystemProxy};
use serde::{Deserialize, Serialize};
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr};

pub const KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
const ENABLE: &str = "ProxyEnable";
const SERVER: &str = "ProxyServer";
const OVERRIDE: &str = "ProxyOverride";

/// The registry operations the backend needs; faked in tests.
pub trait Registry: Send + Sync {
    fn get_u32(&self, name: &str) -> io::Result<Option<u32>>;
    fn get_string(&self, name: &str) -> io::Result<Option<String>>;
    fn set_u32(&self, name: &str, value: u32) -> io::Result<()>;
    fn set_string(&self, name: &str, value: &str) -> io::Result<()>;
    /// Removing a value that does not exist is not an error.
    fn delete(&self, name: &str) -> io::Result<()>;
    /// Tells WinINet the settings changed.
    fn notify(&self);
}

#[derive(Serialize, Deserialize)]
struct WindowsBackup {
    platform: String,
    #[serde(rename = "ProxyEnable")]
    enable: Option<u32>,
    #[serde(rename = "ProxyServer")]
    server: Option<String>,
    #[serde(rename = "ProxyOverride")]
    bypass: Option<String>,
}

pub struct WindowsProxy<R: Registry> {
    reg: R,
}

impl<R: Registry> WindowsProxy<R> {
    pub fn new(reg: R) -> Self {
        WindowsProxy { reg }
    }
}

impl<R: Registry> SystemProxy for WindowsProxy<R> {
    fn snapshot(&self) -> io::Result<Backup> {
        let backup = WindowsBackup {
            platform: "windows".to_string(),
            enable: self.reg.get_u32(ENABLE)?,
            server: self.reg.get_string(SERVER)?,
            bypass: self.reg.get_string(OVERRIDE)?,
        };
        Ok(Backup(
            serde_json::to_value(backup).expect("backup serializes"),
        ))
    }

    fn apply(&self, settings: &ProxySettings) -> io::Result<()> {
        let server = proxy_server_value(settings);
        if server.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no proxy address to apply",
            ));
        }
        self.reg.set_string(SERVER, &server)?;
        let bypass = proxy_override_value(settings);
        if bypass.is_empty() {
            self.reg.delete(OVERRIDE)?;
        } else {
            self.reg.set_string(OVERRIDE, &bypass)?;
        }
        // switched on last, once the address it points at is in place
        self.reg.set_u32(ENABLE, 1)?;
        self.reg.notify();
        Ok(())
    }

    fn restore(&self, backup: &Backup) -> io::Result<()> {
        let saved: WindowsBackup = serde_json::from_value(backup.0.clone())
            .map_err(|_| super::wrong_platform("windows"))?;
        if saved.platform != "windows" {
            return Err(super::wrong_platform("windows"));
        }
        match saved.enable {
            Some(v) => self.reg.set_u32(ENABLE, v)?,
            None => self.reg.delete(ENABLE)?,
        }
        match &saved.server {
            Some(v) => self.reg.set_string(SERVER, v)?,
            None => self.reg.delete(SERVER)?,
        }
        match &saved.bypass {
            Some(v) => self.reg.set_string(OVERRIDE, v)?,
            None => self.reg.delete(OVERRIDE)?,
        }
        self.reg.notify();
        Ok(())
    }
}

/// `http=h:p;https=h:p;socks=h:p`, leaving out what is not set.
pub fn proxy_server_value(settings: &ProxySettings) -> String {
    let mut parts = Vec::new();
    if let Some(addr) = settings.http {
        parts.push(format!("http={addr}"));
    }
    if let Some(addr) = settings.https {
        parts.push(format!("https={addr}"));
    }
    if let Some(addr) = settings.socks {
        parts.push(format!("socks={addr}"));
    }
    parts.join(";")
}

/// `;`-separated patterns, `<local>` last when simple host names are excluded.
pub fn proxy_override_value(settings: &ProxySettings) -> String {
    let mut out: Vec<String> = Vec::new();
    for entry in &settings.bypass {
        for pattern in override_patterns(entry) {
            if !out.contains(&pattern) {
                out.push(pattern);
            }
        }
    }
    let local = "<local>".to_string();
    if settings.exclude_simple && !out.contains(&local) {
        out.push(local);
    }
    out.join(";")
}

/// `ProxyOverride` has no CIDR syntax: an IPv4 network becomes wildcard
/// patterns, an IPv6 literal gets brackets, anything else passes through. An
/// IPv6 network cannot be expressed and is dropped, and so is an entry
/// carrying the `;` that separates the patterns.
fn override_patterns(entry: &str) -> Vec<String> {
    let entry = entry.trim();
    if entry.contains(';') {
        tracing::debug!(
            entry,
            "skip-proxy entry contains `;` and would split into several ProxyOverride patterns; dropped"
        );
        return Vec::new();
    }
    if let Some((addr, prefix)) = entry.split_once('/') {
        return match (addr.parse::<Ipv4Addr>(), prefix.parse::<u8>()) {
            (Ok(ip), Ok(prefix)) if prefix <= 32 => v4_wildcards(ip, prefix),
            _ => {
                tracing::debug!(
                    entry,
                    "skip-proxy entry cannot be expressed in ProxyOverride; dropped"
                );
                Vec::new()
            }
        };
    }
    if entry.parse::<Ipv6Addr>().is_ok() {
        return vec![format!("[{entry}]")];
    }
    if entry.is_empty() {
        Vec::new()
    } else {
        vec![entry.to_string()]
    }
}

fn v4_wildcards(ip: Ipv4Addr, prefix: u8) -> Vec<String> {
    if prefix == 32 {
        return vec![ip.to_string()];
    }
    let octets = ip.octets();
    let whole = usize::from(prefix / 8); // octets that are fully fixed
    let rest = prefix % 8;
    let fixed: Vec<String> = octets[..whole].iter().map(|o| o.to_string()).collect();
    if rest == 0 {
        return vec![if whole == 0 {
            "*".to_string()
        } else {
            format!("{}.*", fixed.join("."))
        }];
    }
    // the partially fixed octet enumerates 2^(8 - rest) values
    let span = 1u16 << (8 - rest);
    let base = u16::from(octets[whole]) & !(span - 1);
    (base..base + span)
        .map(|value| {
            let mut parts = fixed.clone();
            parts.push(value.to_string());
            if whole + 1 == 4 {
                parts.join(".")
            } else {
                format!("{}.*", parts.join("."))
            }
        })
        .collect()
}

/// A key under `HKEY_CURRENT_USER`, opened per call.
#[cfg(windows)]
pub struct RealRegistry {
    path: String,
}

#[cfg(windows)]
/// `HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)`: the key or the value is absent.
const NOT_FOUND: i32 = 0x8007_0002_u32 as i32;

#[cfg(windows)]
impl RealRegistry {
    pub fn internet_settings() -> RealRegistry {
        RealRegistry::at(KEY_PATH)
    }

    /// Any other key under `HKEY_CURRENT_USER` (the tests use a scratch key).
    pub fn at(path: impl Into<String>) -> RealRegistry {
        RealRegistry { path: path.into() }
    }

    fn read<T>(
        &self,
        get: impl FnOnce(&windows_registry::Key) -> windows_registry::Result<T>,
    ) -> io::Result<Option<T>> {
        let key = match windows_registry::CURRENT_USER.open(&self.path) {
            Ok(key) => key,
            Err(e) if e.code().0 == NOT_FOUND => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        match get(&key) {
            Ok(value) => Ok(Some(value)),
            Err(e) if e.code().0 == NOT_FOUND => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn writable(&self) -> io::Result<windows_registry::Key> {
        Ok(windows_registry::CURRENT_USER.create(&self.path)?)
    }
}

#[cfg(windows)]
impl Registry for RealRegistry {
    fn get_u32(&self, name: &str) -> io::Result<Option<u32>> {
        self.read(|key| key.get_u32(name))
    }

    fn get_string(&self, name: &str) -> io::Result<Option<String>> {
        self.read(|key| key.get_string(name))
    }

    fn set_u32(&self, name: &str, value: u32) -> io::Result<()> {
        Ok(self.writable()?.set_u32(name, value)?)
    }

    fn set_string(&self, name: &str, value: &str) -> io::Result<()> {
        Ok(self.writable()?.set_string(name, value)?)
    }

    fn delete(&self, name: &str) -> io::Result<()> {
        match self.writable()?.remove_value(name) {
            Ok(()) => Ok(()),
            Err(e) if e.code().0 == NOT_FOUND => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn notify(&self) {
        notify_wininet();
    }
}

/// The one `unsafe` in the workspace (plan decision P1).
#[cfg(windows)]
#[allow(unsafe_code)]
fn notify_wininet() {
    use windows_sys::Win32::Networking::WinInet::{
        INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED, InternetSetOptionW,
    };
    // SAFETY: a null handle with a null buffer of length 0 is the documented
    // way to broadcast these two options; the calls read no memory of ours.
    let (changed, refreshed) = unsafe {
        (
            InternetSetOptionW(
                std::ptr::null(),
                INTERNET_OPTION_SETTINGS_CHANGED,
                std::ptr::null(),
                0,
            ),
            InternetSetOptionW(
                std::ptr::null(),
                INTERNET_OPTION_REFRESH,
                std::ptr::null(),
                0,
            ),
        )
    };
    if changed == 0 || refreshed == 0 {
        tracing::debug!(
            "InternetSetOptionW reported a failure; running WinINet applications may notice the change late"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone, Debug, PartialEq)]
    enum Val {
        U32(u32),
        Str(String),
    }

    /// An in-memory registry key that also records the order of writes.
    #[derive(Default)]
    struct FakeRegistry {
        values: Mutex<BTreeMap<String, Val>>,
        writes: Mutex<Vec<String>>,
        notified: AtomicUsize,
    }

    impl FakeRegistry {
        fn with(values: &[(&str, Val)]) -> FakeRegistry {
            let reg = FakeRegistry::default();
            for (name, value) in values {
                reg.values
                    .lock()
                    .unwrap()
                    .insert(name.to_string(), value.clone());
            }
            reg
        }
        fn dump(&self) -> BTreeMap<String, Val> {
            self.values.lock().unwrap().clone()
        }
    }

    impl Registry for FakeRegistry {
        fn get_u32(&self, name: &str) -> io::Result<Option<u32>> {
            Ok(match self.values.lock().unwrap().get(name) {
                Some(Val::U32(v)) => Some(*v),
                _ => None,
            })
        }
        fn get_string(&self, name: &str) -> io::Result<Option<String>> {
            Ok(match self.values.lock().unwrap().get(name) {
                Some(Val::Str(v)) => Some(v.clone()),
                _ => None,
            })
        }
        fn set_u32(&self, name: &str, value: u32) -> io::Result<()> {
            self.writes.lock().unwrap().push(name.to_string());
            self.values
                .lock()
                .unwrap()
                .insert(name.to_string(), Val::U32(value));
            Ok(())
        }
        fn set_string(&self, name: &str, value: &str) -> io::Result<()> {
            self.writes.lock().unwrap().push(name.to_string());
            self.values
                .lock()
                .unwrap()
                .insert(name.to_string(), Val::Str(value.to_string()));
            Ok(())
        }
        fn delete(&self, name: &str) -> io::Result<()> {
            self.writes.lock().unwrap().push(format!("-{name}"));
            self.values.lock().unwrap().remove(name);
            Ok(())
        }
        fn notify(&self) {
            self.notified.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn settings() -> ProxySettings {
        ProxySettings {
            http: Some("127.0.0.1:6152".parse().unwrap()),
            https: Some("127.0.0.1:6152".parse().unwrap()),
            socks: Some("127.0.0.1:6153".parse().unwrap()),
            bypass: vec![
                "localhost".into(),
                "*.local".into(),
                "10.0.0.0/8".into(),
                "192.168.1.0/24".into(),
            ],
            exclude_simple: true,
        }
    }

    #[test]
    fn apply_writes_the_three_values_enables_last_and_notifies() {
        let proxy = WindowsProxy::new(FakeRegistry::default());
        proxy.apply(&settings()).unwrap();
        let values = proxy.reg.dump();
        assert_eq!(
            values["ProxyServer"],
            Val::Str("http=127.0.0.1:6152;https=127.0.0.1:6152;socks=127.0.0.1:6153".into())
        );
        assert_eq!(
            values["ProxyOverride"],
            Val::Str("localhost;*.local;10.*;192.168.1.*;<local>".into())
        );
        assert_eq!(values["ProxyEnable"], Val::U32(1));
        assert_eq!(
            *proxy.reg.writes.lock().unwrap(),
            ["ProxyServer", "ProxyOverride", "ProxyEnable"],
            "the proxy is switched on only after its address is in place"
        );
        assert_eq!(proxy.reg.notified.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn socks_is_optional_and_an_empty_bypass_list_removes_the_override() {
        let reg = FakeRegistry::with(&[("ProxyOverride", Val::Str("old".into()))]);
        let proxy = WindowsProxy::new(reg);
        let s = ProxySettings {
            socks: None,
            bypass: Vec::new(),
            exclude_simple: false,
            ..settings()
        };
        proxy.apply(&s).unwrap();
        let values = proxy.reg.dump();
        assert_eq!(
            values["ProxyServer"],
            Val::Str("http=127.0.0.1:6152;https=127.0.0.1:6152".into())
        );
        assert!(!values.contains_key("ProxyOverride"));
    }

    #[test]
    fn apply_without_any_address_is_an_error_and_writes_nothing() {
        let proxy = WindowsProxy::new(FakeRegistry::default());
        let err = proxy.apply(&ProxySettings::default()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(proxy.reg.dump().is_empty());
        assert_eq!(proxy.reg.notified.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn snapshot_then_restore_round_trips_present_and_absent_values() {
        let original = [
            ("ProxyEnable", Val::U32(0)),
            ("ProxyServer", Val::Str("corp:8080".into())),
        ];
        let proxy = WindowsProxy::new(FakeRegistry::with(&original));
        let backup = proxy.snapshot().unwrap();
        assert_eq!(backup.0["platform"], "windows");
        assert_eq!(backup.0["ProxyEnable"], 0);
        assert_eq!(backup.0["ProxyServer"], "corp:8080");
        assert!(backup.0["ProxyOverride"].is_null());
        proxy.apply(&settings()).unwrap();
        assert!(proxy.reg.dump().contains_key("ProxyOverride"));
        proxy.restore(&backup).unwrap();
        let expected: BTreeMap<String, Val> = original
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        assert_eq!(proxy.reg.dump(), expected, "ProxyOverride is absent again");
        assert_eq!(proxy.reg.notified.load(Ordering::SeqCst), 2);
        // the backup survives a JSON round trip (it lives in state.json)
        let text = serde_json::to_string(&backup).unwrap();
        let reread: Backup = serde_json::from_str(&text).unwrap();
        assert_eq!(reread, backup);
    }

    #[test]
    fn restore_rejects_a_backup_from_another_backend() {
        let proxy = WindowsProxy::new(FakeRegistry::default());
        let foreign = Backup(serde_json::json!({ "platform": "macos", "services": {} }));
        let err = proxy.restore(&foreign).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(proxy.reg.notified.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn bypass_entries_become_proxy_override_patterns() {
        let value = |entries: &[&str]| {
            proxy_override_value(&ProxySettings {
                bypass: entries.iter().map(|s| s.to_string()).collect(),
                ..ProxySettings::default()
            })
        };
        assert_eq!(
            value(&["localhost", "127.0.0.1", "*.corp.example"]),
            "localhost;127.0.0.1;*.corp.example"
        );
        assert_eq!(value(&["10.0.0.0/8"]), "10.*");
        assert_eq!(value(&["192.168.0.0/16"]), "192.168.*");
        assert_eq!(value(&["1.2.3.0/24"]), "1.2.3.*");
        assert_eq!(value(&["1.2.3.4/32"]), "1.2.3.4");
        assert_eq!(value(&["0.0.0.0/0"]), "*");
        let twelve = value(&["172.16.0.0/12"]);
        let parts: Vec<&str> = twelve.split(';').collect();
        assert_eq!(parts.len(), 16);
        assert_eq!((parts[0], parts[15]), ("172.16.*", "172.31.*"));
        assert_eq!(
            value(&["1.2.3.252/30"]),
            "1.2.3.252;1.2.3.253;1.2.3.254;1.2.3.255"
        );
        assert_eq!(value(&["::1"]), "[::1]");
        assert_eq!(
            value(&["fd00::/8", "10.0.0.0/8", "10.0.0.0/8"]),
            "10.*",
            "IPv6 networks are dropped, duplicates collapse"
        );
        assert_eq!(
            value(&["a;b", "localhost"]),
            "localhost",
            "an entry with a `;` would become two patterns; it is dropped"
        );
        // `<local>` is appended for exclude-simple-hostnames, but only once
        let with_local = |entries: &[&str]| {
            proxy_override_value(&ProxySettings {
                bypass: entries.iter().map(|s| s.to_string()).collect(),
                exclude_simple: true,
                ..ProxySettings::default()
            })
        };
        assert_eq!(with_local(&["localhost"]), "localhost;<local>");
        assert_eq!(with_local(&["<local>", "localhost"]), "<local>;localhost");
    }

    /// Removes the scratch key when the test ends, pass or fail.
    #[cfg(windows)]
    struct ScratchKey(String);

    #[cfg(windows)]
    impl Drop for ScratchKey {
        fn drop(&mut self) {
            let _ = windows_registry::CURRENT_USER.remove_tree(&self.0);
        }
    }

    #[cfg(windows)]
    #[test]
    fn real_registry_round_trips_under_a_scratch_key() {
        let path = format!(r"Software\rurge-test-{}", std::process::id());
        // a previous run of this test may have crashed before cleaning up
        let _ = windows_registry::CURRENT_USER.remove_tree(&path);
        let _cleanup = ScratchKey(path.clone());
        let reg = RealRegistry::at(path.clone());
        assert_eq!(
            reg.get_u32("ProxyEnable").unwrap(),
            None,
            "a missing key reads as absent"
        );
        reg.set_u32("ProxyEnable", 1).unwrap();
        reg.set_string("ProxyServer", "http=127.0.0.1:1").unwrap();
        assert_eq!(reg.get_u32("ProxyEnable").unwrap(), Some(1));
        assert_eq!(
            reg.get_string("ProxyServer").unwrap().as_deref(),
            Some("http=127.0.0.1:1")
        );
        assert_eq!(reg.get_string("ProxyOverride").unwrap(), None);
        reg.delete("ProxyServer").unwrap();
        reg.delete("ProxyServer").unwrap(); // deleting an absent value is fine
        assert_eq!(reg.get_string("ProxyServer").unwrap(), None);
    }

    #[cfg(windows)]
    #[test]
    fn real_internet_settings_can_be_snapshotted() {
        // read-only: this never writes the machine's proxy settings
        let backup = WindowsProxy::new(RealRegistry::internet_settings())
            .snapshot()
            .unwrap();
        assert_eq!(backup.0["platform"], "windows");
    }
}
