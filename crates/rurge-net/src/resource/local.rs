//! Local file resources: watch the parent directory (editors replace files
//! rather than modify them) and report changes to the file by name.

use notify::{Config, Event, PollWatcher, RecommendedWatcher, RecursiveMode, Watcher};
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

// Held only to keep the platform watcher alive via `Drop`; its variants are never matched on.
#[allow(dead_code)]
pub enum AnyWatcher {
    Recommended(RecommendedWatcher),
    Poll(PollWatcher),
}

pub fn watch_file(path: &Path, tx: UnboundedSender<()>) -> Result<AnyWatcher, notify::Error> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let name: Option<OsString> = path.file_name().map(OsString::from);
    let handler = move |res: notify::Result<Event>| {
        if let Ok(ev) = res
            && ev
                .paths
                .iter()
                .any(|p| p.file_name().map(OsString::from) == name)
        {
            let _ = tx.send(());
        }
    };
    match notify::recommended_watcher(handler.clone()) {
        Ok(mut w) => {
            w.watch(&parent, RecursiveMode::NonRecursive)?;
            Ok(AnyWatcher::Recommended(w))
        }
        Err(e) => {
            tracing::warn!(error = %e, "native file watcher unavailable; polling every 2 s");
            let mut w = PollWatcher::new(
                handler,
                Config::default().with_poll_interval(Duration::from_secs(2)),
            )?;
            w.watch(&parent, RecursiveMode::NonRecursive)?;
            Ok(AnyWatcher::Poll(w))
        }
    }
}
