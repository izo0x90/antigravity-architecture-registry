//! Watches the repo and refreshes the architecture service when files change,
//! so planned components already in the code are re-verified as it moves.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use arch_service::ArchitectureService;
use notify::{RecursiveMode, Watcher};
use tracing::{info, warn};

/// Starts watching `arch`'s repo. A changed file is marked out of date at
/// once; changes are gathered until `quiet` passes without another, then
/// handed to [`ArchitectureService::refresh`] (which skips what ingestion
/// doesn't read). Runs until the process ends.
pub fn watch(arch: Arc<ArchitectureService>, quiet: Duration) -> notify::Result<()> {
    let (tx, rx) = channel::<PathBuf>();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
        Ok(event) if !event.kind.is_access() => {
            for path in event.paths {
                let _ = tx.send(path);
            }
        }
        Ok(_) => {}
        Err(e) => warn!("file watch error: {e}"),
    })?;
    watcher.watch(arch.root(), RecursiveMode::Recursive)?;
    std::thread::Builder::new()
        .name("nah-watch".into())
        .spawn(move || {
            // The watcher lives as long as this thread.
            let _watcher = watcher;
            while let Ok(first) = rx.recv() {
                mark(&arch, &first);
                let mut paths = vec![first];
                loop {
                    match rx.recv_timeout(quiet) {
                        Ok(path) => {
                            mark(&arch, &path);
                            paths.push(path);
                        }
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                paths.sort();
                paths.dedup();
                match arch.refresh(&paths) {
                    Ok(r) if !r.ingest.changed.is_empty() => {
                        let broken = r.reverified.iter().filter(|v| v.report.has_errors()).count();
                        info!("{} files changed; re-verified {} components ({broken} failing)", r.ingest.changed.len(), r.reverified.len());
                    }
                    Ok(_) => {}
                    Err(e) => warn!("refresh after file changes failed: {e}"),
                }
            }
        })
        .map_err(|e| notify::Error::generic(&e.to_string()))?;
    Ok(())
}

/// Shows a changed file as out of date until it's read again.
fn mark(arch: &ArchitectureService, path: &std::path::Path) {
    if let Err(e) = arch.mark_stale(&[path.to_path_buf()]) {
        warn!("marking {} out of date failed: {e}", path.display());
    }
}
