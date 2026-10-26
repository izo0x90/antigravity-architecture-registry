//! Watches the repo and refreshes the architecture service when files change,
//! so planned components already in the code are re-verified as it moves.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use arch_service::ArchitectureService;
use notify_debouncer_full::notify::{self, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, NoCache, new_debouncer_opt};
use tracing::{info, warn};

/// Starts watching `arch`'s repo, and the folders above it whose ignore
/// rules apply to it. Plan files are watched too: they're read again only
/// when they change. notify-debouncer-full settles each file's events over
/// limits.toml's `watch.debounce_ms`; ignored paths and paths outside the
/// repo (rule files aside) are dropped as they arrive (a build writing its
/// output never holds a batch open), and the rest arriving within another
/// window of each other are one batch (a burst of saves is read once). A batch's
/// paths are marked out of date and handed to
/// [`ArchitectureService::refresh`]. A window of 0 turns watching off. Runs
/// until the process ends.
pub fn watch(arch: Arc<ArchitectureService>) -> notify::Result<()> {
    let window = Duration::from_millis(arch.limits().watch.debounce_ms);
    if window.is_zero() {
        return Ok(());
    }
    let (tx, rx) = channel::<Vec<PathBuf>>();
    let (ignores, root, plans) = (arch.ignores(), arch.root().to_path_buf(), arch.clone());
    let handler = move |result: DebounceEventResult| match result {
        Ok(events) => {
            let mut paths: Vec<PathBuf> = events
                .into_iter()
                .filter(|e| !e.kind.is_access())
                .flat_map(|e| e.event.paths)
                // What the rules leave in (hidden files too, for search; the
                // reader still skips those).
                .filter(|p| ignores.is_rule_file(p) || plans.is_plan_file(p) || (p.starts_with(&root) && !ignores.rules_ignore(p)))
                .collect();
            paths.sort();
            paths.dedup();
            if !paths.is_empty() {
                let _ = tx.send(paths);
            }
        }
        Err(errors) => errors.into_iter().for_each(|e| warn!("file watch error: {e}")),
    };
    // No file-id cache: it walks the whole tree (build output too) on start.
    let mut debouncer =
        new_debouncer_opt::<_, notify::RecommendedWatcher, _>(window, None, handler, NoCache::new(), notify::Config::default())?;
    debouncer.watch(arch.root(), RecursiveMode::Recursive)?;
    // Rule files above the project apply to it too.
    for folder in arch.ignores().rule_folders_outside() {
        if let Err(e) = debouncer.watch(&folder, RecursiveMode::NonRecursive) {
            warn!("not watching {} for ignore rules: {e}", folder.display());
        }
    }
    std::thread::Builder::new()
        .name("nah-watch".into())
        .spawn(move || {
            // The watcher lives as long as this thread.
            let _debouncer = debouncer;
            while let Ok(mut paths) = rx.recv() {
                loop {
                    match rx.recv_timeout(window) {
                        Ok(more) => paths.extend(more),
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                paths.sort();
                paths.dedup();
                if let Err(e) = arch.mark_stale(&paths) {
                    warn!("marking changes out of date failed: {e}");
                }
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
