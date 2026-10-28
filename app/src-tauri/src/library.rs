//! The clip library: `<clips_dir>/<game>/<timestamp>.mp4`, with thumbnails and
//! durations cached in `$XDG_CACHE_HOME/clip.sh/thumbs/`.

use std::{
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct Clip {
    pub path: PathBuf,
    pub game: String,
    pub name: String,
    /// Unix time in milliseconds.
    pub created_ms: u64,
    pub size_bytes: u64,
    pub duration: Option<f64>,
    pub audio_tracks: usize,
    pub is_edit: bool,
    pub steam_app_id: Option<u32>,
    pub thumbnail: Option<PathBuf>,
}

/// Saved next to each clip as `<clip>.mp4.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClipMeta {
    pub game: String,
    pub steam_app_id: Option<u32>,
}

fn meta_path(clip: &Path) -> PathBuf {
    let mut p = clip.as_os_str().to_owned();
    p.push(".json");
    PathBuf::from(p)
}

pub fn write_meta(clip: &Path, meta: &ClipMeta) -> Result<()> {
    std::fs::write(meta_path(clip), serde_json::to_string(meta)?)?;
    Ok(())
}

pub fn read_meta(clip: &Path) -> Option<ClipMeta> {
    serde_json::from_str(&std::fs::read_to_string(meta_path(clip)).ok()?).ok()
}

/// Give a rendered edit the same metadata as its source.
pub fn copy_meta(from: &Path, to: &Path) {
    if let Some(m) = read_meta(from) {
        let _ = write_meta(to, &m);
    }
}

#[derive(Serialize, Deserialize)]
struct CachedInfo {
    duration: f64,
    /// Missing in caches written before track counting; those are redone.
    audio_tracks: Option<usize>,
}

/// Part of the cache key; v2 = 1280px thumbnails.
const THUMB_VERSION: u32 = 2;

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("clip.sh/thumbs")
}

/// A clip file found on disk, without decoding it.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub game: String,
    pub size: u64,
    pub modified_ms: u64,
    pub is_edit: bool,
}

/// Every `<clips_dir>/<game>/*.mp4`, cheaply (metadata only).
pub fn scan(clips_dir: &Path) -> Vec<FileEntry> {
    let mut out = Vec::new();
    let Ok(games) = std::fs::read_dir(clips_dir) else {
        return out;
    };
    for game in games.flatten() {
        let game_dir = game.path();
        if !game_dir.is_dir() {
            continue;
        }
        let game_name = game.file_name().to_string_lossy().into_owned();
        let Ok(files) = std::fs::read_dir(&game_dir) else {
            continue;
        };
        for f in files.flatten() {
            let path = f.path();
            if path.extension().is_none_or(|e| e != "mp4") {
                continue;
            }
            let Ok(meta) = f.metadata() else { continue };
            let modified_ms = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as u64);
            out.push(FileEntry {
                is_edit: is_edit(&path),
                path,
                game: game_name.clone(),
                size: meta.len(),
                modified_ms,
            });
        }
    }
    out
}

/// Clips rendered by the trimmer are named `<stem>_edit.mp4` / `_editN.mp4`.
pub fn is_edit(path: &Path) -> bool {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    stem.rsplit_once("_edit")
        .is_some_and(|(_, n)| n.chars().all(|c| c.is_ascii_digit()))
}

/// All clips, newest first. Generates missing thumbnails, so call it off the
/// async runtime.
pub fn list(clips_dir: &Path) -> Vec<Clip> {
    let mut clips: Vec<Clip> = scan(clips_dir)
        .into_iter()
        .filter_map(|f| match load(&f.path, &f.game) {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::warn!("skipping {}: {e:#}", f.path.display());
                None
            }
        })
        .collect();
    clips.sort_by_key(|c| std::cmp::Reverse(c.created_ms));
    clips
}

/// Describe one clip file.
pub fn load(path: &Path, game: &str) -> Result<Clip> {
    let meta = std::fs::metadata(path)?;
    let modified = meta.modified()?;
    let created_ms = modified.duration_since(UNIX_EPOCH)?.as_millis() as u64;

    // Cache key: path + size + mtime, so edited files get new thumbnails.
    let mut h = DefaultHasher::new();
    // Bump THUMB_VERSION when thumbnail generation changes.
    (THUMB_VERSION, path, meta.len(), created_ms).hash(&mut h);
    let key = format!("{:016x}", h.finish());
    let dir = cache_dir();
    let thumb = dir.join(format!("{key}.jpg"));
    let info_path = dir.join(format!("{key}.json"));

    let cached = std::fs::read_to_string(&info_path)
        .ok()
        .and_then(|s| serde_json::from_str::<CachedInfo>(&s).ok());
    let info = match cached {
        Some(c) if thumb.exists() && c.audio_tracks.is_some() => Some(c),
        _ => match clip_core::media::inspect(path) {
            Ok(info) => {
                std::fs::create_dir_all(&dir)?;
                std::fs::write(&thumb, &info.thumbnail)?;
                let c = CachedInfo {
                    duration: info.duration.seconds_f64(),
                    audio_tracks: Some(info.audio_tracks),
                };
                std::fs::write(&info_path, serde_json::to_string(&c)?)?;
                Some(c)
            }
            Err(e) => {
                tracing::warn!("can't read {}: {e:#}", path.display());
                None
            }
        },
    };

    Ok(Clip {
        path: path.to_owned(),
        game: game.to_owned(),
        name: path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        created_ms,
        size_bytes: meta.len(),
        duration: info.as_ref().map(|c| c.duration),
        audio_tracks: info.as_ref().and_then(|c| c.audio_tracks).unwrap_or(0),
        is_edit: is_edit(path),
        steam_app_id: read_meta(path).and_then(|m| m.steam_app_id),
        thumbnail: thumb.exists().then_some(thumb),
    })
}

/// Where to render an edit of `source`: next to it when it's in the library,
/// otherwise in `<clips_dir>/Edits/`. Never overwrites.
pub fn edit_path(clips_dir: &Path, source: &Path) -> PathBuf {
    let in_library = matches!(
        (clips_dir.canonicalize(), source.canonicalize()),
        (Ok(root), Ok(src)) if src.starts_with(&root)
    );
    let dir = match source.parent() {
        Some(p) if in_library => p.to_owned(),
        _ => clips_dir.join("Edits"),
    };
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "clip".into());
    let mut n = 1;
    loop {
        let suffix = if n == 1 {
            "edit".to_string()
        } else {
            format!("edit{n}")
        };
        let p = dir.join(format!("{stem}_{suffix}.mp4"));
        if !p.exists() {
            return p;
        }
        n += 1;
    }
}

/// Delete a clip (to the desktop trash if `use_trash`), refusing anything
/// outside the clips folder.
pub fn delete(clips_dir: &Path, path: &Path, use_trash: bool) -> Result<()> {
    let root = clips_dir.canonicalize().context("clips folder missing")?;
    let target = path.canonicalize().context("clip not found")?;
    if !target.starts_with(&root) || target.extension().is_none_or(|e| e != "mp4") {
        bail!("not a clip: {}", path.display());
    }
    let _ = std::fs::remove_file(meta_path(&target));
    if use_trash {
        trash::delete(&target).with_context(|| format!("moving {} to trash", target.display()))?;
    } else {
        std::fs::remove_file(&target)?;
    }
    // Remove the game folder once it's empty.
    if let Some(dir) = target.parent()
        && dir != root
    {
        let _ = std::fs::remove_dir(dir);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_edits() {
        assert!(is_edit(Path::new("a/2026-10-28_15-01-16_edit.mp4")));
        assert!(is_edit(Path::new("a/2026-10-28_15-01-16_edit3.mp4")));
        assert!(!is_edit(Path::new("a/2026-10-28_15-01-16.mp4")));
        assert!(!is_edit(Path::new("a/my_editing_session.mp4")));
    }
}
