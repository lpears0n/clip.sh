//! Capture spike: pick a screen through the portal and record N seconds to MP4.
//!
//!     cargo run -p clip-core --example record -- [seconds] [out.mp4]
//!
//! Set CLIPSH_RESTORE_TOKEN to the printed token to skip the picker next time.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use clip_core::{CapturePath, Codec, Encoder, ScreenSource, VideoConfig, add_video_branch};
use gstreamer as gst;
use gstreamer::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    clip_core::init()?;

    let mut args = std::env::args().skip(1);
    let seconds: u64 = args.next().map(|s| s.parse()).transpose()?.unwrap_or(10);
    let out = args.next().unwrap_or_else(|| "record.mp4".into());

    let encoder = match std::env::var("CLIPSH_ENCODER").ok().as_deref() {
        Some("nvenc") => Encoder::Nvenc,
        Some("vaapi") => Encoder::Vaapi,
        Some("x264") => Encoder::X264,
        _ => Encoder::probe(Codec::H264).context("no working H.264 encoder found")?,
    };
    println!("encoder: {encoder:?}");

    let token = std::env::var("CLIPSH_RESTORE_TOKEN").ok();
    let source = ScreenSource::open(token.as_deref()).await?;
    println!(
        "pipewire node {} size {:?}",
        source.node_id(),
        source.size()
    );
    if let Some(t) = source.restore_token() {
        println!("restore token: {t}");
    }

    let cfg = VideoConfig::default();
    let pipeline = gst::Pipeline::new();
    // CLIPSH_ZERO_COPY=0 forces the system-memory path for comparison.
    let path = if std::env::var("CLIPSH_ZERO_COPY").is_ok_and(|v| v == "0")
        || encoder.gpu_memory().is_none()
    {
        CapturePath::Copy
    } else {
        CapturePath::ZeroCopy
    };
    println!("capture path: {path:?}");
    let parse = add_video_branch(&pipeline, source.target(), encoder, &cfg, path)?;
    let mux = gst::ElementFactory::make("mp4mux")
        .property("faststart", true)
        .build()?;
    let sink = gst::ElementFactory::make("filesink")
        .property("location", &out)
        .build()?;
    pipeline.add_many([&mux, &sink])?;
    gst::Element::link_many([&parse, &mux, &sink])?;

    pipeline.set_state(gst::State::Playing)?;
    println!("recording {seconds}s to {out} ...");

    let bus = pipeline.bus().context("no bus")?;
    let stopper = pipeline.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(seconds)).await;
        stopper.send_event(gst::event::Eos::new());
    });

    let result = tokio::task::spawn_blocking(move || -> Result<()> {
        for msg in bus.iter_timed(gst::ClockTime::NONE) {
            match msg.view() {
                gst::MessageView::Eos(_) => return Ok(()),
                gst::MessageView::Error(e) => bail!(
                    "{} from {:?}: {:?}",
                    e.error(),
                    msg.src().map(|s| s.path_string()),
                    e.debug()
                ),
                _ => {}
            }
        }
        Ok(())
    })
    .await?;

    pipeline.set_state(gst::State::Null)?;
    source.close().await?;
    result?;
    println!("wrote {out}");
    Ok(())
}
