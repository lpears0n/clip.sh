# clip.sh: continuity notes

Last updated: 2026-10-28. Full plan: `~/.claude/plans/delegated-snuggling-cocke.md`.

## Goal
A Linux-first alternative to Medal.TV. An always-on instant-replay buffer: press a hotkey and the last N seconds are saved. Includes a local clip library and a trim editor.
Decisions: **no capture picker** — like Medal, a game window is detected and recorded automatically (KDE: KWin private protocols; portal + restore token only as a non-KDE fallback). **Linux first** (KDE Wayland main target), **v1 = local only** (no cloud/sharing), **Rust + Tauri 2 + Svelte 5**.

## Machine
CachyOS, KDE Plasma 6 (Wayland), RTX 5080 (main) + Radeon 610M iGPU. Rust 1.99, Node 26, pnpm 12, GStreamer 1.28. All required packages are installed.

## Layout
```
Cargo.toml              workspace (resolver 3, edition 2024); shared deps in [workspace.dependencies]
crates/clip-core/       capture/encode library (no UI)
  src/portal.rs         ScreenSource: ashpd ScreenCast portal → PipeWire fd + node id, restore token
  src/encoder.rs        Encoder {Nvenc, Vaapi, X264}: probe() test-encodes a frame; build() sets properties
  src/capture.rs        VideoConfig (60fps, 20Mbps) + add_video_branch() → returns h264parse tail
  src/audio.rs          AudioSource {Desktop, Mic}: pipewiresrc → 48k stereo → fdkaacenc|avenc_aac → aacparse
  src/ring.rs           ReplayBuffer {video, audio[]}: keyframe-aligned eviction, snapshot() → Clip (rebased, shared memory)
  src/save.rs           write_mp4(Clip) via appsrc → parse → mp4mux faststart; clip_path(), default_clips_dir()
  src/recorder.rs       Recorder::start(cfg, CaptureSource{Portal|Kwin})/status/save/stop; appsink callbacks feed the ring
  src/kwin/             KWin client thread: plasma_window_management (window list/pid/state) + zkde_screencast v6
                        (stream_window → PipeWire object serial, read via pipewiresrc target-object). No picker.
                        protocols/*.xml vendored from plasma-wayland-protocols (LGPL-2.1+)
  src/game.rs           detect(pid, title): SteamAppId/SteamGameId in /proc/pid/environ (name from appmanifest), or a wine .exe
  src/auto.rs           AutoRecorder: KWin window events → start recording the first game window, stop on close,
                        save() → <clips>/<game name>/<timestamp>.mp4
  src/encoder.rs        Codec {H264, Hevc, Av1} (parser, caps, make_parser) × Encoder {Nvenc, Vaapi, X264}; probe(codec)
  src/display.rs        max_refresh_hz() via wl_output (this machine: 240 Hz)
  src/media.rs          inspect(): duration, audio track count, JPEG thumbnail
  src/export.rs         render(): trim [start,end] + mix audio tracks at per-track gains → one AAC track, re-encoded
  src/kwin/shortcut.rs  GlobalShortcut via KGlobalAccel D-Bus (no prompt, works in games); parse("Ctrl+Shift+F8")
  examples/auto.rs      Medal-style spike: waits for a game, Enter saves
  examples/kwin_windows.rs  lists windows / streams one by app_id
  examples/record.rs    spike: portal → record N seconds to MP4
  examples/replay.rs    spike: buffer the screen, Enter saves to ~/Videos/clip.sh/Desktop/, q quits
  tests/encoders.rs     checks encoders work on the host
  tests/save.rs         end-to-end without portal: test srcs → x264/AAC → ring → MP4, checked with ffprobe
crates/clip-ipc/        control-socket protocol: $XDG_RUNTIME_DIR/clipsh.sock, one JSON line each way (save/status/show)
crates/clip-cli/        `clipsh save|status|show` (no GStreamer dependency)
app/src-tauri/src/      media_server.rs (loopback HTTP + Range for <video>; token URL; allow-listed roots),
                        system.rs (notifications via D-Bus, save sound via playbin, XDG autostart), storage.rs,
                        lib.rs (commands, tray, close→hide, hotkey task), service.rs (AutoRecorder actor),
                        library.rs (scan clips dir, thumb cache ~/.cache/clip.sh/thumbs), settings.rs (~/.config/clip.sh/settings.json)
app/src/                Svelte 5 UI: Home (drop zone → Trimmer/Timeline), Clips (thumbnail grid), Settings (split pane)
app/                    Svelte+Vite frontend + app/src-tauri (Tauri shell, crate `clip-sh`, lib `clip_sh_lib`)
```
Note: the frontend lives directly in `app/`, not `app/ui/` as the plan said, because the Tauri CLI expects frontend and src-tauri to share a parent folder.

## Status
- [x] **M1 toolchain + skeleton.** `cargo check --workspace` and `pnpm build` (in `app/`) pass.
- [x] **M2 capture spike.** Verified a real 10s capture: 3440×1440 at 60fps, NVENC, 1s keyframes, ~16 Mbps.
  - After that capture, a caps filter now requests H.264 `profile=high` (otherwise encoders default to Baseline). All three encoders were verified on videotestsrc; the real-screen capture wasn't re-run after the change.
- [x] **M3 replay buffer + audio + save.** Unit tests (ring) + e2e save test pass; clippy clean.
  - Real-screen spike verified: NVENC 3440×1440@60, High, desktop + mic AAC tracks, 10s buffer → 10.5s clip, audio starts within ~8ms of video. A/V lip-sync not yet checked by eye in mpv.
- [x] **Auto game capture (added after M3, user requirement: no picker).** Verified on KWin 6.8.1 with a fake Steam game
  (gst-launch window with SteamAppId set): detected, recorded with NVENC, saved under the game's name, stopped on close.
  Not yet tried with a real game (Proton/fullscreen).
- [ ] **M4 daemon wiring.** Partly done: Tauri hosts AutoRecorder, KGlobalAccel hotkey (default F8), tray (Open/Save/Quit), close hides to tray. Still: Unix socket + `clipsh` CLI, notifications. Host `AutoRecorder` (not the portal Recorder). Hotkey: prefer KGlobalAccel over D-Bus on KDE
  (the GlobalShortcuts portal shows a confirm dialog once). Also: Tauri hosts the recorder, Unix socket `$XDG_RUNTIME_DIR/clipsh.sock`, `clipsh` CLI, GlobalShortcuts portal, tray, notifications.
- [~] M5 library + trim/export UI: done without SQLite (filesystem scan + JSON thumb cache). Trimmer + Render verified by type-check/build only — not yet clicked through.
- [ ] M6 settings, persist restore token, disk quota, autostart, PKGBUILD.

Nothing has been committed to git yet (repo initialized, no commits).

## Gotchas learned
- Shell is zsh: `source ~/.cargo/env` doesn't exist (Rust comes from the system package), and unquoted `$var` doesn't word-split (use `${=var}`).
- `pnpm tauri init` wrongly generated path deps to `../../crates/tauri` (it mistook our `crates/` for a Tauri source checkout). Fixed to crates.io `tauri = "2"`.
- ashpd 0.13: `PersistMode` and `Session` are in `ashpd::desktop`. Keep `Session<Screencast>` alive while capturing.
- nvh264enc accepts BGRx system memory directly (it converts on the GPU). `cudaconvertscale` isn't available on this machine.
- pipewiresrc only sends frames on damage, so `keepalive-time` + `videorate` give a constant fps.
- Run the spike with `CLIPSH_RESTORE_TOKEN=<token>` to skip the picker. `CLIPSH_ENCODER=nvenc|vaapi|x264` forces an encoder.
- Ring timestamps are **running time** (`ring::running_time_buffer`), not raw PTS: x264enc offsets PTS by 1000h and compensates in the segment, which silently dropped all audio from clips before the fix.
- Desktop audio = `pipewiresrc` with stream-properties `stream.capture.sink=true` and no target (follows the default sink).
- Caps changes mid-stream (e.g. resolution change) aren't handled: the ring keeps only the latest caps per track.
- Audio tracks are trimmed to the oldest buffered *video* PTS, not to the window: video cuts at keyframes and keeps up to one extra GOP, which otherwise left a ~0.5s audio gap at the clip start.
- KWin 6.8.1 here offered the restricted protocols **without** any .desktop grant. Still ship `X-KDE-Wayland-Interfaces=org_kde_plasma_window_management,zkde_screencast_unstable_v1` in the app's .desktop file, since other KWin builds enforce it.
- Window streams can have odd sizes → `videocrop` in the video branch trims to even (set from a caps probe on pipewiresrc).
- When a recorded window closes, pipewiresrc logs `target not found` just before KWin's unmapped event. Harmless, but it's logged at ERROR level; quiet it in M4.
- No desktop/non-game capture yet. Option without a picker: zkde_screencast `stream_output` (needs a wl_output binding).

## UI design rules (user, 2026-10-28; third pass — supersedes earlier specs)
Tailwind 4; tokens in app/src/app.css (canvas #0D0D0F, panel #17181B, raised #1E1F23, line #26272C, accent orange #FF7A1A,
danger #FF3B5C, radius 6px controls / 8px cards). Fonts bundled: Inter Variable (UI) + Archivo Variable at 112% width
(`display-face` utility: headings, game names, numbers).
Anti-"vibecoded" rules (from research; audit against these):
- No colored accent strips on nav/cards, no glows, no dashed upload boxes, no uppercase eyebrow labels, no status pills.
- One dominant element per screen; vary weight/size instead of uniform cards.
- Recorder status lives in ONE place: right side of the top bar (StatusBar.svelte). Storage ring = storage only.
- Drag-and-drop = full-window overlay only while dragging; "Open video" is an icon in the top bar.
- Motion only when it means something (shutter flash on the Clip button when a clip saves).
- Real, specific copy; design empty/error states.
Layout (games-first, user-approved 2026-10-28): top bar (logo left, StatusBar right) + 72px icon rail
(Games, All clips, spacer, StorageRing with size label, Settings). Views: GamesView (hero = playing/last game with Steam
library_hero + logo.png; "Recently played" grid of library_capsule covers, fallback = darkened clip frame + title),
GameView (banner + clips grouped by local day with "Played 1h 20m"), ClipViewer (replaces trimmer: view mode with
custom controls + video box sized to the clip's aspect via container units; Trim toggles in/out/levels/Render in place),
ClipsView (all clips grid). Focus ring: :focus-visible only, orange.
Data: app/src-tauri/src/games.rs — sessions.jsonl in $XDG_DATA_HOME/clip.sh (logged by service.rs on game start/stop),
list_games joins sessions + clips + steam_art(); clips get `<clip>.mp4.json` {game, steam_app_id} on save (edits copy it);
older clips resolve the app id by name against installed appmanifests.
- Settings: General / Recording / Audio / Hotkeys / Storage; rows in cards with title+description left, control right.
Ad slots: AdSlot component, off unless `"ads": true` in settings.json (728×90 under the main view).
- Screenshot the app without focus: `CLIPSH_SHOT=out.png ./target/debug/examples/kwin_windows clip-sh`.
- uridecodebin `caps` property breaks with nvh264dec (CUDA output never exposed → hang). Let downstream negotiate instead.
- WebKitGTK can't play <video> from Tauri's asset:// scheme (images work). Videos go through media_server.rs instead.
- Settings JSON gained: launch_at_login, start_in_tray, close_to_tray, notify_on_save, sound_on_save, codec, encoder,
  cursor, desktop_audio, mic_device, audio_bitrate_kbps. FPS clamp 24–480, bitrate 2–100 Mbps.
- All three codecs verified end-to-end on NVENC at 100 Mbps (tests/save.rs `saves_every_supported_codec`).
- Follow-up for very high FPS: capture is BGRx in system memory (≈20 MB/frame at 3440×1440). For 240+ FPS, negotiate
  DMA-BUF from pipewiresrc and import into the encoder (zero-copy) instead of copying.
- Main window starts hidden (tauri.conf visible=false) and is shown in setup unless launched with --autostart + start_in_tray.
- Capture judder: clip timestamps are perfect, but under heavy GPU load (game at ~97%) KWin delivers fewer window
  frames than requested, so videorate duplicates (seen: 6–60% dup frames). DMA-BUF→glupload→nvenc did NOT help (worse),
  so the loss is inside KWin. Menus fool mpdecimate (near-identical frames); measure with a moving test pattern
  (examples/record_window.rs + Recorder::stop logs videorate in/out/dup/drop).
- In-app playback stutter (user-reported, trimmer): diagnostics via playbackStats action → `trimmer playback` log line
  (frames, dropped, fps). Not yet measured. WebKit 2.54 knobs: WEBKIT_GST_DISABLE_GL_SINK, WEBKIT_GST_DMABUF_SINK_DISABLED,
  WEBKIT_DISABLE_DMABUF_RENDERER, WEBKIT_GST_VIDEO_DECODING_LIMIT. Check which decoder loaded via /proc/<WebKitWebProcess>/maps.
- Fixed: KWin thread died on EAGAIN from Wayland read (stopped game detection).
- Blank grey app window in screenshots = monitor off (KWin sends no frame callbacks, WebKit doesn't paint). Not a bug.
- Dev: Svelte component CSS is injected from JS (`svelte({ emitCss: false })` in vite.config.ts). With separate
  `?svelte&type=style` modules, a webview reloading against a fresh dev server requested them before compile and
  Tailwind parsed raw .svelte as CSS ("Invalid declaration: FolderOpen") → blank page.
- Shell: `pgrep -f`/`pkill -f` patterns match the invoking shell's own command line; use `pgrep -x clip-sh`.
- In-app playback measured fine: WebKit rendered 289 frames in 4.85 s, 0 dropped. Perceived choppiness = duplicate
  frames inside the recording (capture under GPU load), not the player.
- ClipViewer now overlays controls on the video (bare glyphs, hairline seek, hover volume, fullscreen F/double-click,
  auto-hide while playing); header actions are borderless text. Not yet seen on screen.

## Multi-desktop support (steps 1–3 of the plan, done 2026-10-28)
- Detection: game::running_games() scans /proc (SteamAppId / Wine .exe), no compositor needed.
- auto.rs backends: Kwin (window capture, per-window detection) | Portal (monitor via ScreenCast portal + restore token
  in $XDG_DATA_HOME/clip.sh/portal-restore-token; /proc scan every 2 s). Auto = KWin if available, else portal.
  Settings `capture_backend` (null/kwin/portal) applies on restart. Portal never pops the picker mid-game: without a
  token it reports needs_screen and the UI shows "Choose a screen to record" (choose_screen command).
- Hotkey: clip_core::hotkey tries KGlobalAccel, then the GlobalShortcuts portal (CLIPSH_HOTKEY_PORTAL=1 forces it);
  none → UI tells the user to bind `clipsh save`. Portal hotkey path compiles but is NOT yet tested on a real desktop.
- Control socket (app/src-tauri/src/ipc_server.rs) also gives single-instance: a 2nd launch sends `show` and exits.
- Verified in the real app with capture_backend=portal: /proc detection → portal screen recording (no prompt) →
  `clipsh save` → stop on exit. Not yet tried on GNOME/Hyprland/wlroots themselves.
- Next for coverage: ext-image-copy-capture (prompt-free window capture where offered), X11, optional KMS helper.
