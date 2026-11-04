// Linux-only example: it talks to Wayland, PipeWire or the portals.
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("this example needs Linux");
}

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    imp::main()
}

#[cfg(target_os = "linux")]
mod imp {
    //! Replay-buffer spike: buffer the screen and save a clip on every Enter.
    //!
    //!     cargo run -p clip-core --example replay -- [replay_seconds] [--mic]
    //!
    //! Clips go to ~/Videos/Reruns/Desktop/. Type `q` + Enter to quit.
    //! Set RERUNS_RESTORE_TOKEN to the printed token to skip the picker next time.

    use anyhow::Result;
    use clip_core::{
        AudioSource, CaptureSource, Encoder, Recorder, RecorderConfig, ScreenSource, clip_path,
        default_clips_dir,
    };
    use tokio::io::{AsyncBufReadExt, BufReader};

    #[tokio::main]
    pub async fn main() -> Result<()> {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "info".into()),
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
        cfg.encoder = match std::env::var("RERUNS_ENCODER").ok().as_deref() {
            Some("nvenc") => Some(Encoder::Nvenc),
            Some("vaapi") => Some(Encoder::Vaapi),
            Some("x264") => Some(Encoder::X264),
            _ => None,
        };

        let token = std::env::var("RERUNS_RESTORE_TOKEN").ok();
        let source = ScreenSource::open(token.as_deref()).await?;
        let recorder = Recorder::start(&cfg, CaptureSource::Portal(source)).await?;
        if let Some(t) = recorder.restore_token() {
            println!("restore token: {t}");
        }
        println!("buffering {}s; Enter = save, q = quit", cfg.replay_seconds);

        let root = default_clips_dir();
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        while let Some(line) = lines.next_line().await? {
            if line.trim() == "q" {
                break;
            }
            let status = recorder.status();
            if let Some(e) = &status.error {
                eprintln!("pipeline error: {e}");
            }
            let path = clip_path(&root, "Desktop");
            match recorder.save(None, &path).await {
                Ok(s) => println!(
                    "saved {} (buffered {:.1}s)",
                    s.path.display(),
                    status.buffered_seconds
                ),
                Err(e) => eprintln!("save failed: {e:#}"),
            }
        }
        recorder.stop().await
    }
}
