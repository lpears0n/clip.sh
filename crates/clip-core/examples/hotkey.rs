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
    //! Register KGlobalAccel hotkeys (save clip, screenshot) and print presses.
    //!
    //!     cargo run -p clip-core --example hotkey -- [save keys] [screenshot keys]
    //!     # defaults: Ctrl+Alt+F12 and Ctrl+Alt+F11

    use clip_core::{Action, Binding, kwin::shortcut::GlobalShortcut};

    #[tokio::main]
    pub async fn main() -> anyhow::Result<()> {
        let mut args = std::env::args().skip(1);
        let bindings = [
            Binding::new(
                Action::SaveClip,
                args.next().unwrap_or_else(|| "Ctrl+Alt+F12".into()),
            ),
            Binding::new(
                Action::Screenshot,
                args.next().unwrap_or_else(|| "Ctrl+Alt+F11".into()),
            ),
        ];
        let (_shortcut, mut presses) = GlobalShortcut::register(&bindings).await?;
        for b in &bindings {
            println!("registered {} for {}", b.keys, b.action.id());
        }
        println!("press one (Ctrl+C to quit)");
        while let Some(action) = presses.recv().await {
            println!("pressed: {}", action.id());
        }
        Ok(())
    }
}
