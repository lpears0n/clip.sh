//! Mux a [`Clip`] snapshot into an MP4 file.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;

use crate::{
    encoder::Codec,
    ring::{Clip, ClipTrack},
};

/// Write `clip` to `path` as MP4 (faststart). Blocks until the file is done.
pub fn write_mp4(clip: &Clip, path: &Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }

    let pipeline = gst::Pipeline::new();
    let mux = gst::ElementFactory::make("mp4mux")
        .property("faststart", true)
        .build()?;
    let sink = gst::ElementFactory::make("filesink")
        .property("location", path)
        .build()?;
    pipeline.add_many([&mux, &sink])?;
    mux.link(&sink)?;

    let codec = Codec::from_caps(&clip.video.caps).context("unknown video codec")?;
    let mut srcs = vec![add_track(&pipeline, &mux, &clip.video, codec.parser())?];
    for track in &clip.audio {
        srcs.push(add_track(&pipeline, &mux, track, "aacparse")?);
    }

    // The appsrcs are unbounded, so every packet can be queued up front;
    // the buffers are refcounted, not copied.
    let tracks = std::iter::once(&clip.video).chain(&clip.audio);
    for (src, track) in srcs.iter().zip(tracks) {
        for buf in &track.buffers {
            src.push_buffer(buf.clone())?;
        }
        src.end_of_stream()?;
    }

    pipeline.set_state(gst::State::Playing)?;
    let bus = pipeline.bus().context("pipeline has no bus")?;
    let result = match bus.timed_pop_filtered(
        gst::ClockTime::from_seconds(60),
        &[gst::MessageType::Eos, gst::MessageType::Error],
    ) {
        Some(msg) => match msg.view() {
            gst::MessageView::Eos(_) => Ok(()),
            gst::MessageView::Error(e) => Err(anyhow::anyhow!(
                "{} from {:?}: {:?}",
                e.error(),
                msg.src().map(|s| s.path_string()),
                e.debug()
            )),
            _ => unreachable!(),
        },
        None => Err(anyhow::anyhow!("timed out writing {}", path.display())),
    };
    pipeline.set_state(gst::State::Null)?;
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

fn add_track(
    pipeline: &gst::Pipeline,
    mux: &gst::Element,
    track: &ClipTrack,
    parser: &str,
) -> Result<gst_app::AppSrc> {
    if track.buffers.is_empty() {
        bail!("empty track");
    }
    let src = gst_app::AppSrc::builder()
        .caps(&track.caps)
        .format(gst::Format::Time)
        .max_bytes(0)
        .build();
    let parse = gst::ElementFactory::make(parser).build()?;
    pipeline.add_many([src.upcast_ref(), &parse])?;
    src.link(&parse)?;
    parse.link(mux)?;
    Ok(src)
}

/// `<root>/<folder>/<YYYY-MM-DD_HH-MM-SS>.mp4`, adding a suffix if taken.
pub fn clip_path(root: &Path, folder: &str) -> PathBuf {
    let dir = root.join(sanitize(folder));
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    let mut path = dir.join(format!("{stamp}.mp4"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stamp}_{n}.mp4"));
        n += 1;
    }
    path
}

/// Default clips folder: `$XDG_VIDEOS_DIR/clip.sh`.
pub fn default_clips_dir() -> PathBuf {
    dirs::video_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Videos")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("clip.sh")
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c == '/' || c.is_control() { '_' } else { c })
        .collect();
    let s = s.trim().trim_start_matches('.');
    if s.is_empty() {
        "Desktop".into()
    } else {
        s.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_folder_names() {
        assert_eq!(sanitize("Counter-Strike 2"), "Counter-Strike 2");
        assert_eq!(sanitize("a/b"), "a_b");
        assert_eq!(sanitize("../x"), "_x");
        assert_eq!(sanitize("  "), "Desktop");
    }
}
