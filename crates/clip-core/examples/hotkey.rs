//! Register a KGlobalAccel hotkey and print presses.
//!
//!     cargo run -p clip-core --example hotkey -- [keys]   # default Ctrl+Alt+F12

use clip_core::kwin::shortcut::GlobalShortcut;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let keys = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Ctrl+Alt+F12".into());
    let (_shortcut, mut presses) = GlobalShortcut::register(&keys).await?;
    println!("registered {keys}; press it (Ctrl+C to quit)");
    while presses.recv().await.is_some() {
        println!("pressed");
    }
    Ok(())
}
