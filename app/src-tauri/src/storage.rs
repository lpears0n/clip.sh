//! Disk usage and the clip storage limit. Like Medal's PC Storage limit it is a
//! soft cap: after a save pushes the library over it, the oldest clips are
//! removed until it fits again.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::library::{self, FileEntry};

#[derive(Debug, Clone, Serialize)]
pub struct Usage {
    pub clips_bytes: u64,
    pub clip_count: usize,
    /// Free and total space on the drive holding the clips folder.
    pub disk_free: u64,
    pub disk_total: u64,
    /// `None` = unlimited.
    pub limit_bytes: Option<u64>,
}

pub fn usage(clips_dir: &Path, limit_bytes: Option<u64>) -> Usage {
    let files = library::scan(clips_dir);
    let (disk_free, disk_total) = disk_space(clips_dir).unwrap_or((0, 0));
    Usage {
        clips_bytes: files.iter().map(|f| f.size).sum(),
        clip_count: files.len(),
        disk_free,
        disk_total,
        limit_bytes,
    }
}

/// Free/total bytes on the filesystem containing `path` (or its nearest
/// existing ancestor).
fn disk_space(path: &Path) -> Result<(u64, u64)> {
    let existing = path
        .ancestors()
        .find(|p| p.exists())
        .context("no existing ancestor")?;
    let st = rustix::fs::statvfs(existing)?;
    Ok((st.f_bavail * st.f_frsize, st.f_blocks * st.f_frsize))
}

#[derive(Debug, Clone, Copy)]
pub struct Policy {
    pub limit_bytes: Option<u64>,
    /// Never auto-delete clips made in the trimmer.
    pub protect_edits: bool,
    /// Move to the desktop trash instead of deleting outright.
    pub use_trash: bool,
}

/// Remove the oldest clips until the library fits the limit. The newest clip
/// is never removed, even if it alone exceeds the limit. Returns what was
/// removed.
pub fn enforce(clips_dir: &Path, policy: Policy) -> Result<Vec<PathBuf>> {
    let Some(limit) = policy.limit_bytes else {
        return Ok(vec![]);
    };
    let mut files = library::scan(clips_dir);
    let mut total: u64 = files.iter().map(|f| f.size).sum();
    if total <= limit {
        return Ok(vec![]);
    }
    files.sort_by_key(|f| f.modified_ms);
    let newest = files.last().map(|f| f.path.clone());
    let mut removed = Vec::new();
    for f in files {
        if total <= limit {
            break;
        }
        if Some(&f.path) == newest.as_ref() || (policy.protect_edits && f.is_edit) {
            continue;
        }
        remove(clips_dir, &f, policy.use_trash)?;
        total -= f.size;
        removed.push(f.path);
    }
    if !removed.is_empty() {
        tracing::info!(count = removed.len(), "storage limit: removed oldest clips");
    }
    Ok(removed)
}

fn remove(clips_dir: &Path, f: &FileEntry, use_trash: bool) -> Result<()> {
    library::delete(clips_dir, &f.path, use_trash)
}

/// Gigabytes (as set in the UI) to bytes.
pub fn gb(gb: f64) -> u64 {
    (gb.max(0.0) * 1e9) as u64
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;

    /// `<tmp>/Game/{a, b_edit, c}.mp4`, 1000 bytes each, oldest to newest.
    fn library() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "clipsh-storage-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let game = root.join("Game");
        std::fs::create_dir_all(&game).unwrap();
        let now = SystemTime::now();
        for (i, name) in ["a", "b_edit", "c"].into_iter().enumerate() {
            let p = game.join(format!("{name}.mp4"));
            std::fs::write(&p, [0u8; 1000]).unwrap();
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(now - Duration::from_secs(300 - i as u64 * 100))
                .unwrap();
        }
        root
    }

    fn names(paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    fn policy(limit: Option<u64>, protect_edits: bool) -> Policy {
        Policy {
            limit_bytes: limit,
            protect_edits,
            use_trash: false,
        }
    }

    #[test]
    fn unlimited_or_under_limit_removes_nothing() {
        let root = library();
        assert!(enforce(&root, policy(None, false)).unwrap().is_empty());
        assert!(
            enforce(&root, policy(Some(3000), false))
                .unwrap()
                .is_empty()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn removes_oldest_first() {
        let root = library();
        let removed = enforce(&root, policy(Some(2000), false)).unwrap();
        assert_eq!(names(&removed), ["a"]);
        assert_eq!(usage(&root, None).clips_bytes, 2000);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn protects_edits_and_newest() {
        let root = library();
        // Over the limit even after removing everything allowed.
        let removed = enforce(&root, policy(Some(500), true)).unwrap();
        assert_eq!(names(&removed), ["a"]);
        let left = names(
            &library::scan(&root)
                .into_iter()
                .map(|f| f.path)
                .collect::<Vec<_>>(),
        );
        assert!(left.contains(&"b_edit".to_string()) && left.contains(&"c".to_string()));

        let root = library();
        let removed = enforce(&root, policy(Some(500), false)).unwrap();
        assert_eq!(names(&removed), ["a", "b_edit"], "newest is always kept");
        std::fs::remove_dir_all(root).unwrap();
    }
}
