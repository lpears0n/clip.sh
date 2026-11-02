# clip.sh

An instant replay recorder for Linux, kind of like Medal. It keeps the last 30 seconds (or however long you set) in memory while you play, and when you hit F8 it saves that to an MP4 named after the game. It also has a basic trimmer for cutting clips and mixing the game and mic audio.

## How to run it locally

You'll need Rust, Node with pnpm, and GStreamer with the PipeWire and VA-API plugins. On Arch that's roughly:

```
sudo pacman -S rustup nodejs pnpm gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugin-pipewire gst-plugin-va gst-libav webkit2gtk-4.1 libayatana-appindicator
rustup default stable
```

Then, from the repo root:

```
cd app
pnpm install
pnpm tauri dev
```

To build a release binary instead, use `pnpm tauri build`. There's also a small CLI, `clipsh save`, that talks to the running app. Bind it to a key in your desktop settings if the in-app hotkey doesn't work for you.

## How it works / Tech stack

- **Tauri 2 + Svelte 5** for the UI. Tauri uses the system webview, so the app stays small compared to Electron.
- **GStreamer** does all the capture, encoding and muxing. It's a pipeline: screen, then encoder, then a ring buffer in memory. Saving copies references to the buffered frames into a new MP4 pipeline, so it doesn't re-encode anything.
- **NVENC / VA-API / x264** for encoding. It probes which one actually works on your machine and uses the best one.
- **KWin's private screencast protocol** on KDE, so it can record a game window without a permission popup. Everywhere else it falls back to the standard screen-share portal, which asks once and remembers your choice.
- **Process scanning** (`/proc`) to find running games, looking for Steam's app id or a Wine `.exe`. Works on any desktop.
- **ashpd** for the portal calls, and **zbus** for the KDE and notification D-Bus stuff.
- **Tailwind 4** for styling, plus Lucide icons.

## Using it

- **Games** shows what you've played, with a hero for the most recent game.
- **All clips** has search (`Ctrl+F`), sorting, a game filter and a selection
  mode for deleting many clips at once. Deletes can be undone for a few
  seconds from the toast.
- Hover a clip for a silent preview; open it to watch or trim. Press `?` in
  the viewer for every keyboard shortcut (speed, frame stepping, in/out
  points, repeat).
- Star a clip (`S`), rename it (`N`) or add labels in the viewer. Favourites
  and labels are filters on the clips page, and they live in the small
  `.json` file next to each clip, so they survive moving the folder.
- **Settings → Appearance** picks the accent colour and card size, and turns
  off hover previews or motion.

The whole thing is a Cargo workspace: `crates/clip-core` has the capture and encoding code with no UI, `crates/clip-ipc` and `crates/clip-cli` are the `clipsh` command, and `app/` is the Tauri app.

## Current state / What's next

It works, but it's a lot of hacky glue and I've only really tested it on my own machine, which is KDE Plasma 6 on Wayland with an NVIDIA card.

There's a roadmap with the research behind it in [docs/ROADMAP.md](docs/ROADMAP.md):
what's planned for the library and editor, how a Windows port would work
(GStreamer's Windows Graphics Capture and WASAPI elements, so the ring
buffer and trimmer carry over), and packaging. CI builds and lints every
push; tagging `v*` builds AppImage, deb and rpm as a draft release.

Known problems:
- **Recordings can stutter under heavy GPU load.** When the game is using the GPU hard, the compositor delivers fewer frames, and the encoder duplicates some of them to keep a constant frame rate. The playback in the app is fine, the loss happens during capture.
- **Only KDE gets prompt-free window capture.** On GNOME and Hyprland you get a one-time screen-share prompt instead. That part is only partly tested.
- **The portal hotkey path isn't tested** on a real desktop yet.
- **X11 isn't supported.** There's no capture backend for it.
- **Not packaged yet.** The release workflow and an AUR `PKGBUILD` (under `packaging/`) are in place but no release has been tagged.
- **Storage limits are a soft cap.** Old clips get deleted after a save if you go over, and the Trash option is on by default so you can recover them.

What I'd do next, if I had more time: zero-copy capture for high frame rates (the current path copies every frame through system memory), the `ext-image-copy-capture` protocol for window capture on wlroots desktops, X11 support, and a proper package.
