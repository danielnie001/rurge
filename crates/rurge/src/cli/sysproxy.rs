//! The system proxy as `rurge run` drives it (M4 design §8.2): what to point
//! the operating system at, and the enable / disable / crash-recovery /
//! follow-the-listeners state machine around `rurge_platform::sysproxy`.

use super::control::connect_addr;
use rurge_config::general::General;
use rurge_config::session::ListenerKind;
use rurge_config::{HostList, HostPattern};
use rurge_engine::state::StateStore;
use rurge_platform::sysproxy::{Backup, ProxySettings, SystemProxy};
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// `file:<path>` swaps the operating system for a JSON file (end-to-end tests).
pub const BACKEND_ENV: &str = "RURGE_SYSTEM_PROXY_BACKEND";

/// The first HTTP listener serves http and https; the first SOCKS5 listener
/// serves socks when `set-system-socks-proxy` allows it.
pub fn proxy_settings(
    general: &General,
    listeners: &[(ListenerKind, SocketAddr)],
) -> Option<ProxySettings> {
    let first = |kind: ListenerKind| {
        listeners
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, addr)| connect_addr(*addr))
    };
    let http = first(ListenerKind::Http);
    let socks = if general.set_system_socks_proxy {
        first(ListenerKind::Socks5)
    } else {
        None
    };
    if http.is_none() && socks.is_none() {
        return None;
    }
    Some(ProxySettings {
        http,
        https: http,
        socks,
        bypass: bypass_list(&general.skip_proxy),
        exclude_simple: general.exclude_simple_hostnames,
    })
}

/// The `skip-proxy` entries an operating system's bypass list can express: no
/// negation, no `<…>` tokens, no ports.
fn bypass_list(list: &HostList) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for entry in &list.entries {
        if entry.negate {
            continue;
        }
        let item = match &entry.pattern {
            HostPattern::Glob(glob) => glob.source().to_string(),
            HostPattern::Cidr(net) if net.prefix_len() == net.max_prefix_len() => {
                net.addr().to_string()
            }
            HostPattern::Cidr(net) => net.trunc().to_string(),
            _ => continue,
        };
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

pub fn describe(settings: &ProxySettings) -> String {
    let mut parts = Vec::new();
    if let Some(addr) = settings.http {
        parts.push(format!("http {addr}"));
    }
    if let Some(addr) = settings.socks {
        parts.push(format!("socks {addr}"));
    }
    parts.join(", ")
}

/// The operating system's backend, unless `RURGE_SYSTEM_PROXY_BACKEND` says
/// otherwise. An unknown value is an error: silently falling back would let a
/// typo in a test change the real settings.
pub fn backend() -> anyhow::Result<Arc<dyn SystemProxy>> {
    match std::env::var_os(BACKEND_ENV) {
        None => Ok(Arc::from(rurge_platform::sysproxy::platform())),
        Some(value) => backend_from(&value.to_string_lossy()),
    }
}

fn backend_from(value: &str) -> anyhow::Result<Arc<dyn SystemProxy>> {
    match value.strip_prefix("file:") {
        Some(path) if !path.is_empty() => Ok(Arc::new(FileBackend {
            path: PathBuf::from(path),
        })),
        _ => anyhow::bail!("unknown {BACKEND_ENV} value `{value}` (expected file:<path>)"),
    }
}

/// The "system proxy" is a JSON file.
struct FileBackend {
    path: PathBuf,
}

impl SystemProxy for FileBackend {
    fn snapshot(&self) -> io::Result<Backup> {
        let previous = match std::fs::read_to_string(&self.path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        Ok(Backup(
            serde_json::json!({ "platform": "file", "previous": previous }),
        ))
    }

    fn apply(&self, settings: &ProxySettings) -> io::Result<()> {
        let text = serde_json::json!({
            "http": settings.http.map(|a| a.to_string()),
            "https": settings.https.map(|a| a.to_string()),
            "socks": settings.socks.map(|a| a.to_string()),
            "bypass": settings.bypass,
            "exclude_simple": settings.exclude_simple,
        });
        std::fs::write(&self.path, text.to_string())
    }

    fn restore(&self, backup: &Backup) -> io::Result<()> {
        if backup.0["platform"] != "file" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the system proxy backup was not taken by the file backend",
            ));
        }
        match backup.0["previous"].as_str() {
            Some(text) => std::fs::write(&self.path, text),
            None => match std::fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            },
        }
    }
}

/// Registry writes and spawned tools block: keep them off the runtime.
async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> Result<T, String> {
    match tokio::task::spawn_blocking(job).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(e.to_string()),
        Err(e) => Err(format!("system proxy task failed: {e}")),
    }
}

/// Owned by the `rurge run` main loop, which calls it one request at a time.
pub struct SystemProxyManager {
    backend: Arc<dyn SystemProxy>,
    store: Arc<StateStore>,
    /// What the operating system points at right now (`None` = not ours).
    applied: Option<ProxySettings>,
    flag: Arc<AtomicBool>,
}

impl SystemProxyManager {
    pub fn new(backend: Arc<dyn SystemProxy>, store: Arc<StateStore>) -> Self {
        SystemProxyManager {
            backend,
            store,
            applied: None,
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Mirrors `enabled()`; shared with `Control::system_proxy_enabled`.
    pub fn flag(&self) -> Arc<AtomicBool> {
        self.flag.clone()
    }

    pub fn enabled(&self) -> bool {
        self.applied.is_some()
    }

    /// What the operating system points at right now.
    pub fn applied(&self) -> Option<&ProxySettings> {
        self.applied.as_ref()
    }

    /// Startup: a backup still in `state.json` means the last run died with
    /// the system proxy pointing at it.
    pub async fn recover(&mut self) {
        let Some(saved) = self.store.snapshot().await.system_proxy_backup else {
            return;
        };
        tracing::warn!(
            "a previous run left the system proxy pointing at rurge; restoring the saved settings"
        );
        let backend = self.backend.clone();
        match blocking(move || backend.restore(&Backup(saved))).await {
            Ok(()) => self.forget().await,
            Err(e) => tracing::error!(
                error = %e,
                "cannot restore the system proxy; the saved settings stay in state.json for the next attempt"
            ),
        }
    }

    pub async fn enable(&mut self, settings: ProxySettings) -> Result<(), String> {
        if self.applied.as_ref() == Some(&settings) {
            return Ok(());
        }
        if self.applied.is_none() {
            // A backup that is already saved is the real original (an earlier
            // restore failed): never snapshot rurge's own settings over it.
            let backup = match self.store.snapshot().await.system_proxy_backup {
                Some(_) => None,
                None => {
                    let backend = self.backend.clone();
                    Some(blocking(move || backend.snapshot()).await?)
                }
            };
            // saved before anything changes, so a crash mid-apply is recoverable
            self.store
                .update(move |s| {
                    if let Some(backup) = backup {
                        s.system_proxy_backup = Some(backup.0);
                    }
                    s.features.system_proxy = true;
                })
                .await;
        }
        let backend = self.backend.clone();
        let wanted = settings.clone();
        match blocking(move || backend.apply(&wanted)).await {
            Ok(()) => {
                self.applied = Some(settings);
                self.flag.store(true, Ordering::SeqCst);
                Ok(())
            }
            Err(e) => {
                // a partial apply may have changed something: put the original back
                if let Err(undo) = self.disable().await {
                    tracing::error!(
                        error = %undo,
                        "cannot undo the failed system proxy change; the saved settings stay in state.json"
                    );
                    self.applied = None;
                    self.flag.store(false, Ordering::SeqCst);
                }
                Err(e)
            }
        }
    }

    /// Restores the saved settings. When the restore fails the backup stays in
    /// `state.json`, so the next start tries again.
    pub async fn disable(&mut self) -> Result<(), String> {
        if let Some(saved) = self.store.snapshot().await.system_proxy_backup {
            let backend = self.backend.clone();
            blocking(move || backend.restore(&Backup(saved))).await?;
        }
        self.forget().await;
        Ok(())
    }

    /// After a reload: follow the listeners and `skip-proxy` when they changed.
    pub async fn refresh(&mut self, settings: Option<ProxySettings>) -> Result<(), String> {
        match settings {
            Some(settings) if self.enabled() => self.enable(settings).await,
            _ => Ok(()),
        }
    }

    async fn forget(&mut self) {
        self.store
            .update(|s| {
                s.system_proxy_backup = None;
                s.features.system_proxy = false;
            })
            .await;
        self.applied = None;
        self.flag.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostList;
    use serde_json::json;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MockProxy {
        calls: Mutex<Vec<String>>,
        restored: Mutex<Vec<Backup>>,
        fail_apply: AtomicBool,
        fail_restore: AtomicBool,
    }

    impl MockProxy {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl SystemProxy for MockProxy {
        fn snapshot(&self) -> io::Result<Backup> {
            self.calls.lock().unwrap().push("snapshot".to_string());
            Ok(Backup(json!({ "platform": "mock", "original": true })))
        }
        fn apply(&self, settings: &ProxySettings) -> io::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("apply {}", describe(settings)));
            if self.fail_apply.load(Ordering::SeqCst) {
                return Err(io::Error::other("apply refused"));
            }
            Ok(())
        }
        fn restore(&self, backup: &Backup) -> io::Result<()> {
            self.calls.lock().unwrap().push("restore".to_string());
            if self.fail_restore.load(Ordering::SeqCst) {
                return Err(io::Error::other("restore refused"));
            }
            self.restored.lock().unwrap().push(backup.clone());
            Ok(())
        }
    }

    fn settings(http_port: u16) -> ProxySettings {
        ProxySettings {
            http: Some(SocketAddr::from(([127, 0, 0, 1], http_port))),
            https: Some(SocketAddr::from(([127, 0, 0, 1], http_port))),
            socks: Some(SocketAddr::from(([127, 0, 0, 1], 6153))),
            bypass: vec!["localhost".to_string()],
            exclude_simple: false,
        }
    }

    async fn manager() -> (
        tempfile::TempDir,
        Arc<MockProxy>,
        Arc<StateStore>,
        SystemProxyManager,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = StateStore::open(dir.path().join("state.json")).await;
        let mock = Arc::new(MockProxy::default());
        let manager = SystemProxyManager::new(mock.clone(), store.clone());
        (dir, mock, store, manager)
    }

    #[tokio::test]
    async fn enable_saves_the_original_before_applying_and_disable_puts_it_back() {
        let (_dir, mock, store, mut manager) = manager().await;
        let flag = manager.flag();
        manager.enable(settings(6152)).await.unwrap();
        assert_eq!(
            mock.calls(),
            [
                "snapshot",
                "apply http 127.0.0.1:6152, socks 127.0.0.1:6153"
            ]
        );
        let state = store.snapshot().await;
        assert_eq!(
            state.system_proxy_backup,
            Some(json!({ "platform": "mock", "original": true }))
        );
        assert!(state.features.system_proxy && manager.enabled() && flag.load(Ordering::SeqCst));
        manager.disable().await.unwrap();
        assert_eq!(
            *mock.restored.lock().unwrap(),
            [Backup(json!({ "platform": "mock", "original": true }))]
        );
        let state = store.snapshot().await;
        assert_eq!(state.system_proxy_backup, None);
        assert!(!state.features.system_proxy && !manager.enabled() && !flag.load(Ordering::SeqCst));
        manager.disable().await.unwrap();
        assert_eq!(mock.calls().len(), 3, "disabling twice touches nothing");
    }

    #[tokio::test]
    async fn changed_settings_are_applied_without_a_second_snapshot() {
        let (_dir, mock, store, mut manager) = manager().await;
        manager.enable(settings(6152)).await.unwrap();
        manager.enable(settings(6152)).await.unwrap();
        assert_eq!(
            mock.calls().len(),
            2,
            "the same settings are not applied twice"
        );
        manager.enable(settings(7000)).await.unwrap();
        assert_eq!(
            mock.calls(),
            [
                "snapshot",
                "apply http 127.0.0.1:6152, socks 127.0.0.1:6153",
                "apply http 127.0.0.1:7000, socks 127.0.0.1:6153",
            ],
            "a second snapshot would save rurge's own settings as the original"
        );
        assert_eq!(
            store.snapshot().await.system_proxy_backup,
            Some(json!({ "platform": "mock", "original": true }))
        );
    }

    #[tokio::test]
    async fn a_failed_apply_is_rolled_back() {
        let (_dir, mock, store, mut manager) = manager().await;
        mock.fail_apply.store(true, Ordering::SeqCst);
        let err = manager.enable(settings(6152)).await.unwrap_err();
        assert!(err.contains("apply refused"), "{err}");
        assert_eq!(
            mock.calls(),
            [
                "snapshot",
                "apply http 127.0.0.1:6152, socks 127.0.0.1:6153",
                "restore"
            ]
        );
        let state = store.snapshot().await;
        assert_eq!(state.system_proxy_backup, None);
        assert!(
            !state.features.system_proxy
                && !manager.enabled()
                && !manager.flag().load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn recover_restores_what_a_crashed_run_left_behind() {
        let (_dir, mock, store, mut manager) = manager().await;
        manager.recover().await;
        assert!(
            mock.calls().is_empty(),
            "nothing to recover on a clean state"
        );
        let left_behind = json!({ "platform": "mock", "from": "crashed run" });
        let saved = left_behind.clone();
        store
            .update(move |s| {
                s.system_proxy_backup = Some(saved);
                s.features.system_proxy = true;
            })
            .await;
        manager.recover().await;
        assert_eq!(*mock.restored.lock().unwrap(), [Backup(left_behind)]);
        let state = store.snapshot().await;
        assert_eq!(state.system_proxy_backup, None);
        assert!(!state.features.system_proxy);
    }

    #[tokio::test]
    async fn a_failed_recovery_keeps_the_original_for_later() {
        let (_dir, mock, store, mut manager) = manager().await;
        let left_behind = json!({ "platform": "mock", "from": "crashed run" });
        let saved = left_behind.clone();
        store
            .update(move |s| s.system_proxy_backup = Some(saved))
            .await;
        mock.fail_restore.store(true, Ordering::SeqCst);
        manager.recover().await;
        assert_eq!(
            store.snapshot().await.system_proxy_backup,
            Some(left_behind.clone()),
            "the backup stays for the next attempt"
        );
        // enabling now must not snapshot rurge's stale settings over the original
        manager.enable(settings(6152)).await.unwrap();
        assert!(
            !mock.calls().contains(&"snapshot".to_string()),
            "{:?}",
            mock.calls()
        );
        mock.fail_restore.store(false, Ordering::SeqCst);
        manager.disable().await.unwrap();
        assert_eq!(*mock.restored.lock().unwrap(), [Backup(left_behind)]);
    }

    #[tokio::test]
    async fn refresh_follows_changes_only_while_enabled() {
        let (_dir, mock, _store, mut manager) = manager().await;
        manager.refresh(Some(settings(6152))).await.unwrap();
        assert!(
            mock.calls().is_empty(),
            "a disabled proxy is not switched on by a reload"
        );
        manager.enable(settings(6152)).await.unwrap();
        manager.refresh(Some(settings(6152))).await.unwrap();
        manager.refresh(None).await.unwrap();
        assert_eq!(
            mock.calls().len(),
            2,
            "unchanged or missing settings change nothing"
        );
        manager.refresh(Some(settings(7000))).await.unwrap();
        assert_eq!(
            mock.calls().last().unwrap(),
            "apply http 127.0.0.1:7000, socks 127.0.0.1:6153"
        );
    }

    #[test]
    fn settings_come_from_the_first_listeners_and_skip_proxy() {
        let general = General {
            skip_proxy: HostList::parse(
                "localhost, *.local, 192.168.1.5/16, 127.0.0.1, -internal.example, <ip-address>, www.example.com:8080, localhost",
                None,
            ),
            exclude_simple_hostnames: true,
            ..General::default()
        };
        let listeners = [
            (ListenerKind::Socks5, "0.0.0.0:6153".parse().unwrap()),
            (ListenerKind::Http, "0.0.0.0:6152".parse().unwrap()),
            (ListenerKind::Http, "127.0.0.1:7000".parse().unwrap()),
        ];
        let s = proxy_settings(&general, &listeners).unwrap();
        assert_eq!(s.http, Some("127.0.0.1:6152".parse().unwrap()));
        assert_eq!(s.https, s.http);
        assert_eq!(s.socks, Some("127.0.0.1:6153".parse().unwrap()));
        assert_eq!(
            s.bypass,
            [
                "localhost",
                "*.local",
                "192.168.0.0/16",
                "127.0.0.1",
                "www.example.com"
            ]
        );
        assert!(s.exclude_simple);
        assert_eq!(describe(&s), "http 127.0.0.1:6152, socks 127.0.0.1:6153");

        let v6 = [(ListenerKind::Http, "[::]:6152".parse().unwrap())];
        assert_eq!(
            proxy_settings(&general, &v6).unwrap().http,
            Some("[::1]:6152".parse().unwrap()),
            "a wildcard bind becomes the loopback of the same family"
        );
        let no_socks = General {
            set_system_socks_proxy: false,
            ..General::default()
        };
        assert_eq!(proxy_settings(&no_socks, &listeners).unwrap().socks, None);
        assert_eq!(proxy_settings(&general, &[]), None);
        let socks_only = [(ListenerKind::Socks5, "127.0.0.1:6153".parse().unwrap())];
        assert_eq!(proxy_settings(&no_socks, &socks_only), None);
    }

    #[test]
    fn the_file_backend_round_trips_and_unknown_backends_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sysproxy.json");
        let spec = format!("file:{}", path.display());
        let backend = backend_from(&spec).unwrap();
        // no file yet: restore removes what apply created
        let empty = backend.snapshot().unwrap();
        backend.apply(&settings(6152)).unwrap();
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["http"], "127.0.0.1:6152");
        assert_eq!(written["socks"], "127.0.0.1:6153");
        assert_eq!(written["bypass"], json!(["localhost"]));
        backend.restore(&empty).unwrap();
        assert!(!path.exists());
        // an existing file comes back byte for byte
        std::fs::write(&path, "the user's own settings").unwrap();
        let original = backend.snapshot().unwrap();
        backend.apply(&settings(6152)).unwrap();
        backend.restore(&original).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "the user's own settings"
        );
        let foreign = Backup(json!({ "platform": "windows" }));
        assert!(backend.restore(&foreign).is_err());
        assert!(backend_from("registry").is_err());
        assert!(backend_from("file:").is_err());
    }
}
