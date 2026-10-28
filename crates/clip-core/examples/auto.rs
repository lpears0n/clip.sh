//! Medal-style spike: waits for a game window, buffers it automatically (no
//! picker), and saves a clip on every Enter. `q` + Enter quits.
//!
//!     cargo run -p clip-core --example auto -- [replay_seconds] [--mic] [--portal]
//!
//! Clips go to ~/Videos/clip.sh/<game name>/ (or $CLIPSH_CLIPS_DIR).

use anyhow::Result;
use clip_core::{
    AudioSource, RecorderConfig,
    auto::{AutoRecorder, Backend},
    default_clips_dir,
};
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "clip_core=info".into()),
        )
        .init();
    clip_core::init()?;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cfg = RecorderConfig::default();
    if let Some(s) = args.iter().find_map(|a| a.parse().ok()) {
        cfg.replay_seconds = s;
    }
    if args.iter().any(|a| a == "--mic") {
        cfg.audio.push(AudioSource::Mic(None));
    }

    let prefer = args
        .iter()
        .any(|a| a == "--portal")
        .then_some(Backend::Portal);
    let token_path = std::env::var_os("CLIPSH_TOKEN_FILE")
        .map_or_else(|| default_clips_dir().join(".portal-token"), Into::into);
    let (mut auto, mut events) = AutoRecorder::new(cfg, prefer, token_path).await?;
    if auto.needs_screen() {
        println!("choose the screen to record in the dialog…");
        if let Some(change) = auto.choose_screen().await? {
            println!("{change:?}");
        }
    }
    println!("waiting for a game; Enter = save, q = quit");
    let root = std::env::var_os("CLIPSH_CLIPS_DIR").map_or_else(default_clips_dir, Into::into);
    let mut lines = BufReader::new(tokio::io::stdin()).lines();

    loop {
        tokio::select! {
            Some(ev) = events.recv() => {
                match auto.handle(ev).await {
                    Ok(Some(change)) => println!("{change:?}"),
                    Ok(None) => {}
                    Err(e) => eprintln!("error: {e:#}"),
                }
            }
            line = lines.next_line() => {
                let Some(line) = line? else { break };
                if line.trim() == "q" {
                    break;
                }
                match auto.save(&root, None).await {
                    Ok(p) => println!("saved {}", p.display()),
                    Err(e) => eprintln!("save failed: {e:#}"),
                }
            }
        }
    }
    auto.shutdown().await
}
