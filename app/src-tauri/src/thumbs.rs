//! Thumbnails and sprite sheets are made off the request path: `list_clips`
//! returns at once with whatever is cached, and this pool fills in the rest,
//! announcing each finished clip to the UI.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::SystemTime,
};

use tauri::{AppHandle, Emitter};

use crate::library;

/// Emitted with the refreshed `library::Clip` once its images exist.
pub const THUMBNAIL_READY: &str = "thumbnail-ready";

/// Decoding is itself multi-threaded; two jobs at once keeps a laptop
/// responsive while the first launch works through a big library.
const WORKERS: usize = 2;

struct Job {
    path: PathBuf,
    game: String,
}

/// A file's size and modification time: whether it changed since.
type Stamp = (u64, Option<SystemTime>);

fn stamp(path: &Path) -> Stamp {
    std::fs::metadata(path).map_or((0, None), |m| (m.len(), m.modified().ok()))
}

#[derive(Clone)]
pub struct Thumbnailer {
    tx: mpsc::Sender<Job>,
    /// `None`: queued or running, so a clip is never generated twice at
    /// once. `Some`: failed with the file as stamped, so a broken file isn't
    /// retried every refresh, but one that has changed since (still being
    /// copied in when it failed) is.
    seen: Arc<Mutex<HashMap<PathBuf, Option<Stamp>>>>,
}

impl Thumbnailer {
    pub fn start(app: AppHandle) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        let seen: Arc<Mutex<HashMap<PathBuf, Option<Stamp>>>> = Arc::default();
        for _ in 0..WORKERS {
            let rx = rx.clone();
            let app = app.clone();
            let seen = seen.clone();
            std::thread::Builder::new()
                .name("thumbnails".into())
                .spawn(move || {
                    loop {
                        let job = match rx.lock().unwrap().recv() {
                            Ok(j) => j,
                            Err(_) => return,
                        };
                        match library::generate(&job.path) {
                            Ok(()) => {
                                // Done: allow regeneration if the file changes later.
                                seen.lock().unwrap().remove(&job.path);
                                match library::load(&job.path, &job.game) {
                                    Ok(clip) => {
                                        let _ = app.emit(THUMBNAIL_READY, clip);
                                    }
                                    Err(e) => tracing::warn!("{}: {e:#}", job.path.display()),
                                }
                            }
                            Err(e) => {
                                tracing::warn!("thumbnail for {}: {e:#}", job.path.display());
                                let failed = stamp(&job.path);
                                seen.lock().unwrap().insert(job.path, Some(failed));
                            }
                        }
                    }
                })
                .expect("spawning thumbnail worker");
        }
        Self { tx, seen }
    }

    /// Queue a clip whose images are missing. Duplicates are ignored, and
    /// so is a file that failed and hasn't changed since.
    pub fn request(&self, path: PathBuf, game: String) {
        let mut seen = self.seen.lock().unwrap();
        match seen.get(&path) {
            Some(None) => return,
            Some(Some(failed)) if *failed == stamp(&path) => return,
            _ => {}
        }
        seen.insert(path.clone(), None);
        drop(seen);
        let _ = self.tx.send(Job { path, game });
    }
}
