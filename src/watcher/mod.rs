//! File system watcher for incremental indexing

use notify::{Watcher, RecursiveMode};
use std::path::Path;
use std::sync::mpsc;

pub struct FileWatcher {
    #[allow(dead_code)]
    tx: mpsc::Sender<FileEvent>,
}

#[derive(Debug, Clone)]
pub enum FileEvent {
    Created(String),
    Modified(String),
    Deleted(String),
    Renamed { from: String, to: String },
}

impl FileWatcher {
    pub fn new(watched_dirs: &[&Path]) -> crate::Result<(Self, mpsc::Receiver<FileEvent>)> {
        let (tx, rx) = mpsc::channel();

        // Set up notify watcher
        let mut watcher = notify::recommended_watcher({
            let tx = tx.clone();
            move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res {
                    // Map notify events to our FileEvent
                    match event.kind {
                        notify::EventKind::Create(_) => {
                            for path in &event.paths {
                                let _ = tx.send(FileEvent::Created(
                                    path.to_string_lossy().to_string(),
                                ));
                            }
                        }
                        notify::EventKind::Modify(_) => {
                            for path in &event.paths {
                                let _ = tx.send(FileEvent::Modified(
                                    path.to_string_lossy().to_string(),
                                ));
                            }
                        }
                        notify::EventKind::Remove(_) => {
                            for path in &event.paths {
                                let _ = tx.send(FileEvent::Deleted(
                                    path.to_string_lossy().to_string(),
                                ));
                            }
                        }
                        _ => {}
                    }
                }
            }
        })?;

        // Watch all directories
        for dir in watched_dirs {
            watcher.watch(dir, RecursiveMode::Recursive)?;
        }

        // Keep watcher alive by storing it (would need proper lifecycle management in real impl)
        std::mem::forget(watcher);

        Ok((Self { tx: tx.clone() }, rx))
    }
}
