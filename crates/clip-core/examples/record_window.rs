//! Record one window (picked by app id) through the real KWin capture path for
//! a fixed time, then save. Used to measure frame pacing.
//!
//!     cargo run -p clip-core --example record_window -- <app_id substring> <seconds> <out.mp4> [fps]

use std::time::Duration;

use anyhow::{Context, Result};
use clip_core::{
    CaptureSource, Recorder, RecorderConfig,
    kwin::{Kwin, WindowEvent},
};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "clip_core=info".into()),
        )
        .init();
    clip_core::init()?;
    let mut args = std::env::args().skip(1);
    let app_id = args.next().context("app id")?;
    let seconds: u64 = args.next().context("seconds")?.parse()?;
    let out = args.next().context("output path")?;
    let fps: Option<u32> = args.next().map(|s| s.parse()).transpose()?;

    let (kwin, mut events) = Kwin::connect()?;
    let uuid = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(ev) = events.recv().await {
            if let WindowEvent::Added(w) | WindowEvent::Changed(w) = ev
                && w.app_id.contains(&app_id)
            {
                return Some(w.uuid);
            }
        }
        None
    })
    .await?
    .context("window not found")?;

    let mut cfg = RecorderConfig {
        replay_seconds: seconds as u32 + 5,
        audio: if std::env::var_os("CLIPSH_AUDIO").is_some() {
            vec![clip_core::AudioSource::Desktop]
        } else {
            vec![]
        },
        ..Default::default()
    };
    if let Some(fps) = fps {
        cfg.video.fps = fps;
    }
    let stream = kwin.stream_window(&uuid, false).await?;
    let rec = Recorder::start(&cfg, CaptureSource::Kwin(stream)).await?;
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    let path = rec.save(Some(seconds as u32), out.as_ref()).await?;
    println!("saved {}", path.display());
    rec.stop().await
}
