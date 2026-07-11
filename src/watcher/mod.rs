//! File system watcher for incremental indexing

use notify::{Watcher, RecursiveMode};
use std::path::Path;
use std::sync::mpsc;

pub struct FileWatcher {
    _watcher: notify::RecommendedWatcher,
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

        let mut watcher = notify::recommended_watcher({
            let tx = tx.clone();
            move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res {
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

        for dir in watched_dirs {
            watcher.watch(dir, RecursiveMode::Recursive)?;
        }

        Ok((Self { _watcher: watcher }, rx))
    }
}
