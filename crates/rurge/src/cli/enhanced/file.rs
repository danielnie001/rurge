//! The enhanced mode's "system" as a JSON file (M4 design 9.10; P19): what the
//! manager did to it, in the order it did it, and a device that carries
//! nothing. Only tests set it (`RURGE_ENHANCED_BACKEND=file:<path>`).
//!
//! The file keeps the keys this backend does not know, so a test can put
//! `fail`, `fail_undo` and `device_fails_after_ms` in it and read its own back
//! afterwards. An action is written to `actions` once it has returned, and the
//! fields it changes (`open`, `ipv6`, `routes`, `dns`, `adjusted`) change only
//! when it did what it was asked: an action that failed is neither recorded nor
//! reflected, so `actions` is exactly what the backend did.

use super::backend::{Backend, Opened};
use super::undo::Undo;
use rurge_net::BoxFuture;
use rurge_platform::route::{MTU, RouteSet};
use rurge_tun::device::Device;
use serde_json::{Map, Value, json};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// The name of the interface the empty device pretends to be.
const DEVICE_NAME: &str = "rurge-test";

/// The "system": one JSON file.
pub struct FileBackend {
    path: PathBuf,
}

impl FileBackend {
    pub fn new(path: PathBuf) -> FileBackend {
        FileBackend { path }
    }

    /// The file as an object; a file that is not there or cannot be read is
    /// `{}`.
    fn read(&self) -> Map<String, Value> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| match value {
                Value::Object(map) => Some(map),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn write(&self, map: &Map<String, Value>) -> Result<(), String> {
        std::fs::write(&self.path, Value::Object(map.clone()).to_string())
            .map_err(|e| format!("cannot write the test backend's file: {e}"))
    }

    /// Whether the file asks the action named `action` to fail.
    fn fails(&self, map: &Map<String, Value>, action: &str) -> bool {
        map.get("fail").and_then(Value::as_str) == Some(action)
    }

    /// The file, unless `fail` names `action`; nothing is written then.
    fn step(&self, action: &str) -> Result<Map<String, Value>, String> {
        let map = self.read();
        if self.fails(&map, action) {
            return Err(format!("{action} failed (test)"));
        }
        Ok(map)
    }

    /// One action as `actions` says it, appended to what is there.
    fn record(&self, map: &mut Map<String, Value>, said: String) {
        let list = map.entry("actions").or_insert_with(|| json!([]));
        if let Value::Array(list) = list {
            list.push(Value::String(said));
        }
    }

    /// The routes the file says are installed.
    fn installed(&self, map: &Map<String, Value>) -> Vec<String> {
        map.get("routes")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Backend for FileBackend {
    fn open(&self) -> Result<Opened, String> {
        let mut map = self.step("open")?;
        let fails_after = map
            .get("device_fails_after_ms")
            .and_then(Value::as_u64)
            .map(Duration::from_millis);
        map.insert("open".to_string(), Value::Bool(true));
        self.record(&mut map, "open".to_string());
        self.write(&map)?;
        Ok(Opened {
            device: Arc::new(EmptyDevice { fails_after }),
            name: DEVICE_NAME.to_string(),
            index: None,
        })
    }

    fn configure(&self, _dev: &Opened, ipv6: bool) -> Result<bool, String> {
        let mut map = self.step("configure")?;
        // the IPv6 half failing is no failure of the step: the open goes on
        // without it, and the file says so
        let ipv6 = ipv6 && !self.fails(&map, "configure_ipv6");
        map.insert("ipv6".to_string(), Value::Bool(ipv6));
        self.record(&mut map, "configure".to_string());
        self.write(&map)?;
        Ok(ipv6)
    }

    fn configure_ipv6(&self, _dev: &Opened) -> Result<bool, String> {
        let mut map = self.step("configure_ipv6")?;
        if self.fails(&map, "configure_ipv6") {
            return Ok(false);
        }
        map.insert("ipv6".to_string(), Value::Bool(true));
        self.record(&mut map, "configure_ipv6".to_string());
        self.write(&map)?;
        Ok(true)
    }

    fn unconfigure_ipv6(&self, _dev: &Opened) -> Result<(), String> {
        let mut map = self.step("unconfigure_ipv6")?;
        map.insert("ipv6".to_string(), Value::Bool(false));
        self.record(&mut map, "unconfigure_ipv6".to_string());
        self.write(&map)
    }

    fn routes(&self, _dev: &Opened, add: &RouteSet, remove: &RouteSet) -> Result<bool, String> {
        let mut map = self.read();
        // `fail: routes` is the half that adds: a close's removals still go
        // through, as a real system's would
        if !add.is_empty() && self.fails(&map, "routes") {
            return Err("routes failed (test)".to_string());
        }
        let mut installed = self.installed(&map);
        for net in add
            .v4
            .iter()
            .map(ToString::to_string)
            .chain(add.v6.iter().map(ToString::to_string))
        {
            installed.push(net);
        }
        for net in remove
            .v4
            .iter()
            .map(ToString::to_string)
            .chain(remove.v6.iter().map(ToString::to_string))
        {
            installed.retain(|have| *have != net);
        }
        installed.sort();
        installed.dedup();
        map.insert("routes".to_string(), json!(installed));
        self.record(&mut map, format!("routes +{} -{}", add.len(), remove.len()));
        self.write(&map)?;
        Ok(true)
    }

    fn dns_undo(&self, _dev: &Opened, saved: &[Undo]) -> Result<Vec<Undo>, String> {
        self.step("dns_undo")?;
        let take = Undo::Test {
            step: "take_dns".to_string(),
        };
        Ok(if saved.contains(&take) {
            Vec::new()
        } else {
            vec![take]
        })
    }

    fn take_dns(&self, _dev: &Opened, _ipv6: bool) -> Result<bool, String> {
        let mut map = self.step("take_dns")?;
        map.insert("dns".to_string(), Value::Bool(true));
        self.record(&mut map, "take_dns".to_string());
        self.write(&map)?;
        Ok(true)
    }

    fn give_dns_back(&self, _dev: &Opened) -> Result<(), String> {
        let mut map = self.step("give_dns_back")?;
        self.record(&mut map, "give_dns_back".to_string());
        self.write(&map)
    }

    fn adjust_undo(&self, _primary: Option<&str>, saved: &[Undo]) -> Result<Option<Undo>, String> {
        self.step("adjust_undo")?;
        let adjust = Undo::Test {
            step: "adjust".to_string(),
        };
        Ok(if saved.contains(&adjust) {
            None
        } else {
            Some(adjust)
        })
    }

    fn adjust(&self) -> Result<(), String> {
        let mut map = self.step("adjust")?;
        map.insert("adjusted".to_string(), Value::Bool(true));
        self.record(&mut map, "adjust".to_string());
        self.write(&map)
    }

    fn undo(&self, undo: &Undo) -> Result<(), String> {
        let label = undo.label();
        let mut map = self.read();
        if map.get("fail_undo").and_then(Value::as_str) == Some(label.as_str()) {
            return Err(format!("{label} undo failed (test)"));
        }
        match undo {
            Undo::Test { step } if step == "take_dns" => {
                map.insert("dns".to_string(), Value::Bool(false));
            }
            Undo::Test { step } if step == "adjust" => {
                map.insert("adjusted".to_string(), Value::Bool(false));
            }
            _ => {}
        }
        self.record(&mut map, format!("undo {label}"));
        self.write(&map)
    }

    fn close(&self, _dev: Opened) {
        let mut map = self.read();
        map.insert("open".to_string(), Value::Bool(false));
        self.record(&mut map, "close".to_string());
        // a close has no way of saying that it went wrong: the file is best
        // effort, as closing a device is
        let _ = self.write(&map);
    }
}

/// The device the file backend opens: it carries nothing. It never gives the
/// TUN a packet (the reader waits until the TUN stops), and it drops what the
/// TUN writes. With `fails_after`, its next read fails once that has passed:
/// a device that breaks under the TUN, for task 8's tests.
struct EmptyDevice {
    fails_after: Option<Duration>,
}

impl Device for EmptyDevice {
    fn recv<'a>(
        &'a self,
        _into: &'a mut Vec<Vec<u8>>,
        _max: usize,
    ) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            match self.fails_after {
                Some(after) => {
                    tokio::time::sleep(after).await;
                    Err(io::Error::other("the device failed (test)"))
                }
                None => std::future::pending().await,
            }
        })
    }

    fn send<'a>(&'a self, packets: &'a mut Vec<Vec<u8>>) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            packets.clear();
            Ok(())
        })
    }

    fn mtu(&self) -> usize {
        usize::from(MTU)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// What a test needs of a device: that it says how much it carries.
    #[test]
    fn the_device_carries_nothing_and_waits() {
        let device = EmptyDevice { fails_after: None };
        assert_eq!(device.mtu(), 1500);
    }

    /// The file keeps the keys it does not know, and says what was done in the
    /// order it was done.
    #[test]
    fn the_file_keeps_unknown_keys_and_records_what_was_done() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("enhanced.json");
        std::fs::write(&path, json!({"mine": 7, "fail_undo": "adjust"}).to_string()).unwrap();
        let backend = FileBackend::new(path.clone());
        let dev = backend.open().unwrap();
        assert_eq!(dev.name, DEVICE_NAME);
        assert_eq!(dev.index, None);
        assert!(backend.configure(&dev, true).unwrap());
        let add = RouteSet {
            v4: vec!["1.0.0.0/8".parse().unwrap()],
            v6: vec![],
        };
        assert!(backend.routes(&dev, &add, &RouteSet::default()).unwrap());
        assert_eq!(
            backend.dns_undo(&dev, &[]).unwrap(),
            [Undo::Test {
                step: "take_dns".to_string()
            }]
        );
        assert!(backend.take_dns(&dev, true).unwrap());
        assert_eq!(
            backend.adjust_undo(None, &[]).unwrap(),
            Some(Undo::Test {
                step: "adjust".to_string()
            })
        );
        backend.adjust().unwrap();
        backend.close(dev);
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["mine"], 7);
        assert_eq!(written["fail_undo"], "adjust");
        assert_eq!(written["open"], false);
        assert_eq!(written["ipv6"], true);
        assert_eq!(written["dns"], true);
        assert_eq!(written["adjusted"], true);
        assert_eq!(written["routes"], json!(["1.0.0.0/8"]));
        assert_eq!(
            written["actions"],
            json!([
                "open",
                "configure",
                "routes +1 -0",
                "take_dns",
                "adjust",
                "close"
            ])
        );
    }

    /// A step the file names fails, leaves the file's state alone and is not
    /// among what was done; the routes fail only while something is added.
    #[test]
    fn a_step_the_file_names_fails_and_is_not_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("enhanced.json");
        std::fs::write(&path, json!({"fail": "routes"}).to_string()).unwrap();
        let backend = FileBackend::new(path.clone());
        let dev = backend.open().unwrap();
        let add = RouteSet {
            v4: vec!["1.0.0.0/8".parse().unwrap()],
            v6: vec![],
        };
        assert_eq!(
            backend
                .routes(&dev, &add, &RouteSet::default())
                .unwrap_err(),
            "routes failed (test)"
        );
        // removing is not the half the file names
        assert!(backend.routes(&dev, &RouteSet::default(), &add).unwrap());
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["actions"], json!(["open", "routes +0 -1"]));
        assert_eq!(written["routes"], json!([]));
        // IPv6 that cannot be had is no failure of the step
        std::fs::write(&path, json!({"fail": "configure_ipv6"}).to_string()).unwrap();
        assert!(!backend.configure(&dev, true).unwrap());
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["ipv6"], false);
        assert_eq!(written["actions"], json!(["configure"]));
    }

    /// An undo puts the field back and is recorded; the one the file names in
    /// `fail_undo` fails and changes nothing.
    #[test]
    fn an_undo_that_the_file_stops_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("enhanced.json");
        std::fs::write(
            &path,
            json!({"dns": true, "adjusted": true, "fail_undo": "adjust"}).to_string(),
        )
        .unwrap();
        let backend = FileBackend::new(path.clone());
        let take = Undo::Test {
            step: "take_dns".to_string(),
        };
        let adjust = Undo::Test {
            step: "adjust".to_string(),
        };
        backend.undo(&take).unwrap();
        assert_eq!(
            backend.undo(&adjust).unwrap_err(),
            "adjust undo failed (test)"
        );
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["dns"], false);
        assert_eq!(written["adjusted"], true);
        assert_eq!(written["actions"], json!(["undo take_dns"]));
    }

    /// A file that is not there, or is not JSON, is a system nothing has been
    /// done to yet.
    #[test]
    fn a_file_that_cannot_be_read_is_an_empty_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("enhanced.json");
        let backend = FileBackend::new(path.clone());
        assert!(backend.read().is_empty());
        std::fs::write(&path, "not json").unwrap();
        assert!(backend.read().is_empty());
        // and a directory where the file should be makes every step fail
        let dir_path = dir.path().join("a-directory.json");
        std::fs::create_dir(&dir_path).unwrap();
        let backend = FileBackend::new(dir_path);
        let Err(error) = backend.open() else {
            panic!("the file backend wrote into a directory")
        };
        assert!(error.starts_with("cannot write"), "{error}");
    }

    /// Two tests in this module share no file, and the device's failure time
    /// is read from the file once, at the open.
    #[test]
    fn the_device_failure_comes_from_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("enhanced.json");
        std::fs::write(&path, json!({"device_fails_after_ms": 5}).to_string()).unwrap();
        let backend = FileBackend::new(path);
        let dev = backend.open().unwrap();
        let device = dev.device.clone();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let failed = runtime.block_on(async move {
            let mut into = Vec::new();
            device.recv(&mut into, 1).await.unwrap_err().to_string()
        });
        assert_eq!(failed, "the device failed (test)");
    }
}
