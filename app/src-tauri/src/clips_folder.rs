//! Keeping saves working when the clips folder can't be written to.
//!
//! Windows' Controlled folder access (ransomware protection) guards
//! Videos, Documents, Pictures and the Desktop: an app it doesn't know may
//! read them but not create anything in them, and is told "file not found".
//! With it on, every save into the default `Videos\Reruns` failed. A folder
//! on a drive that is gone fails the same way.

use std::path::{Path, PathBuf};

/// Where clips go instead: `Reruns` in the home folder, which Controlled
/// folder access doesn't guard.
pub fn fallback() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join("Reruns"))
}

/// Whether a file can be made in `dir`, which is created if missing.
pub fn writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".reruns-write-check");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// `None` while `dir` can be written to; otherwise `fallback`, if that can.
pub fn instead_of(dir: &Path, fallback: &Path) -> Option<PathBuf> {
    if writable(dir) || dir == fallback || !writable(fallback) {
        return None;
    }
    Some(fallback.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("reruns-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_folder_that_can_be_made_is_kept() {
        let root = temp("keep");
        let clips = root.join("Videos").join("Reruns");
        assert!(instead_of(&clips, &root.join("fallback")).is_none());
        assert!(clips.is_dir(), "made on the way");
        assert_eq!(
            std::fs::read_dir(&clips).unwrap().count(),
            0,
            "no check file left"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A folder that can't be made (here under a file, as a guarded
    /// Videos folder or a missing drive) sends saves to the fallback.
    #[test]
    fn an_unwritable_folder_uses_the_fallback() {
        let root = temp("blocked");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("not-a-folder");
        std::fs::write(&file, b"").unwrap();
        let clips = file.join("Reruns");
        let fallback = root.join("fallback");
        assert!(!writable(&clips));
        assert_eq!(instead_of(&clips, &fallback), Some(fallback.clone()));
        // Nowhere to go: keep the folder, and let the save report why.
        assert_eq!(instead_of(&clips, &file.join("also-blocked")), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
