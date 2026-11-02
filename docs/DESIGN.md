# clip.sh design and performance guide

How clip.sh should look, feel and perform, and why. Written after a research
pass over current UI craft guidance (Linear, Raycast, *Refactoring UI*, Emil
Kowalski's animation work, Nielsen Norman Group), WebKitGTK and Tauri
performance reports, and GStreamer zero-copy capture work. Sources are
linked inline and collected at the end.

The short version: **polish is consistency plus restraint, and performance
is a design feature.** Most of what makes an app feel "generated" is
arbitrary values and effects used because they were available, not because
the design called for them.

---

## 1. Honest audit: why it reads as generic today

Looking at the current UI with a critical eye, these are the tells. None is
a disaster; together they are what people mean by "vibecoded".

| Tell | Where | Why it hurts |
|---|---|---|
| Arbitrary values | `pl-[22px]`, `text-[19px]`, `h-[clamp(220px,36vh,380px)]`, `w-[188px]`, `max-w-[760px]`, `text-[13px]` next to `text-[14px]` next to `text-[12px]` | No rhythm. Linear and Raycast look "tight" because everything sits on a 4 px grid and a type scale with 5 or 6 sizes, not 9. |
| Three corner radii plus ad-hoc ones | `--radius-ctl: 6`, `--radius-card: 8`, `rounded-[4px]`, `rounded-full` pills | Radii should encode one thing: nesting depth. Outer surfaces larger, inner controls smaller, by a fixed step. |
| Depth by shadow and blur | `shadow-xl shadow-black/40`, `shadow-2xl shadow-black/60`, `backdrop-blur-sm` on every card button, `blur-[2px]` on covers | On dark surfaces shadows barely read and on WebKitGTK they are expensive (section 4). Dark UIs get depth from surface tint and 1 px hairlines. |
| Hover reveals by opacity flip | Card buttons `opacity-0 → group-hover:opacity-100` | Reads as "things popping in". Polished apps keep controls present but quiet, or reveal with a 120 ms fade from a visible resting state. |
| Icon-only rail with tooltips | Left nav | Three items do not need a rail. It forces tooltips, costs 72 px, and the storage ring at the bottom is unlabelled. A labelled sidebar or a top segmented nav reads as deliberate. |
| Perpetual animation | `animate-ping` on the recording dot | Keeps the compositor awake forever for a decoration. One subtle 2 s pulse or a static dot. |
| Generic empty thumbnails | Clapperboard icon centred in a dark box | A placeholder should look like the thing it stands in for: a dim 16:9 gradient with the game's colour, or a blurred cover. |
| Mixed voice in copy | "Play something." / "Waiting for a game" / "Can't capture:" / sentence-case titles with periods in some places | Pick one voice: sentence case, no trailing periods on labels, periods on full sentences only. |
| Uniform weight | Everything is 500 or 600 Inter | Hierarchy comes from size *and* weight *and* colour. Headings at 600 with tight tracking, body at 450–500, metadata at 400 in the muted colour. |
| Wordmark in the header | `clip.sh` in the top-left, orange suffix | A fine mark, but the header has nothing else on the left, so it looks like a placeholder. Either make the header carry navigation or drop the header into the sidebar. |
| Tailwind defaults showing through | `zinc-100`, `zinc-700`, `bg-white/10` hover | The palette should be ours end to end, with named roles, so a change to the base tint moves everything. |

What is already right and should be kept: dark-first, a single accent,
content-forward Games page, no modals, keyboard coverage in the viewer,
undo instead of confirm.

---

## 2. Design system

### 2.1 Grid and spacing

- Base unit **4 px**. Every padding, gap and size is a multiple. Allowed
  scale: `4 8 12 16 20 24 32 40 48 64`. Nothing else, no `22px`.
- Component padding: controls 8/12, cards 16, page gutter 24, section gap
  32. Related things sit at 8 or 12, unrelated at 24 or 32 ("group tightly,
  separate generously", *Refactoring UI*).
- Content column max 1280 px for the library, 960 px for settings. Centre
  it; the rail or sidebar does not count.
- Optical, not just mathematical, alignment: icons sit 1 px lower than cap
  height next to text; a play glyph is nudged right by 1–2 px inside its
  circle.

### 2.2 Type

Inter stays. Linear uses Inter Display for headings and Inter for everything
else; we have Archivo for display which is a fine, sportier choice. Keep it
to headings and numbers only.

| Role | Size / line | Weight | Tracking | Colour |
|---|---|---|---|---|
| Page title | 24 / 28 | 600 display | −0.02 em | fg |
| Section title | 15 / 20 | 600 display | −0.01 em | fg |
| Body / controls | 14 / 20 | 500 | 0 | fg |
| Secondary | 13 / 18 | 450 | 0 | fg-muted |
| Caption / meta / kbd | 12 / 16 | 500 | +0.01 em | fg-muted |

Five sizes. Tabular numerals (`font-variant-numeric: tabular-nums`) for
every number that changes: timers, sizes, counts. Weight 450–510 is where
Inter looks crisp on dark backgrounds; 400 goes thin, 600 goes heavy except
in headings.

### 2.3 Colour

Build the palette from one base tint with opacities, the way Linear's theme
generator does (three inputs: base, accent, contrast), so a user accent and
a future light theme fall out of the same maths. Use **OKLCH** for the
derived steps so perceived lightness stays even.

```
--bg-0   canvas       oklch(14% 0.004 260)   #0d0d0f today
--bg-1   panel        +3% L
--bg-2   raised       +6% L
--bg-3   overlay      +9% L   (popovers, toasts)
--line   hairline     white 8%
--line-2 strong       white 14%
--fg     text         white 92%
--fg-2   secondary    white 64%
--fg-3   muted        white 46%
--accent user-chosen, clamped to chroma ≤ 0.2 so it never glares
--danger / --ok        same lightness as accent, fixed hues
```

Rules: accent appears on at most one element per view that is not a
selection state. Hairlines, not fills, separate rows. Hover is a one-step
surface change (`bg-1 → bg-2`), not `white/10`. Selected is accent at 12%
fill plus a 1 px accent border.

Contrast: `fg` ≥ 13:1, `fg-2` ≥ 7:1, `fg-3` ≥ 4.6:1 on `bg-0`, so everything
clears WCAG AA and secondary text clears AAA.

### 2.4 Depth

Dark UIs get elevation from **lighter surface + hairline**, not shadow. Only
floating overlays (toast, popover, help sheet) get a shadow, and one shadow
token: `0 8px 24px rgb(0 0 0 / 0.4)` with a `line-2` border. No
`backdrop-filter` anywhere (section 4 explains why on Linux). Over images
(hero, card buttons) use a solid `black/60` fill, which reads the same and
costs nothing.

Radii encode nesting: window surfaces 12, cards 8, controls 6, chips and
kbd 4. Never `rounded-full` on rectangular controls; pills only for
genuinely round things (dots, avatars) and the selection toolbar.

### 2.5 Motion

From Kowalski's rules and the Linear/Raycast practice:

- Nothing over **200 ms** in the library, 300 ms absolute cap. Overlay enter
  150 ms `ease-out`, exit 100 ms `ease-in`. Hover changes 120 ms.
- Animate **only `transform` and `opacity`**. No `transition-all`, no
  animating `filter`, `box-shadow`, `width` or colour on anything that
  repeats (cards). Colour transitions are fine on single controls.
- **Keyboard-initiated actions do not animate.** Pressing `?` shows the
  sheet instantly; pressing `Esc` hides it instantly. Raycast has no
  animations at all for this reason.
- Elements that move together share one easing and duration (sheet and its
  backdrop).
- No perpetual animations. The recording indicator is a static dot; the
  save flash is one 400 ms fade.
- `prefers-reduced-motion` already collapses durations; keep it.

### 2.6 States

Every list and every async action has four designed states: loading, empty,
error, success. Rules by response time (NN/g):

- < 100 ms: show the result, no indicator. Optimistic updates (already done
  for favourites, deletes) belong here.
- 100 ms – 1 s: no spinner. Keep the previous content and change the cursor
  or disable the control.
- > 1 s: progress. Skeletons only for layouts we know (the clip grid), with
  a slow left-to-right shimmer, never a pulse; spinner otherwise. The
  evidence on skeletons is mixed, so use them sparingly and match the final
  layout exactly.
- > 10 s (rendering a long edit): percent-done and a cancel button.

Empty states are instructions, not apologies: one sentence, one action.

### 2.7 Navigation and structure

Replace the icon rail with a **labelled sidebar** (200 px, collapsible to
icons with a keyboard shortcut) or drop navigation into a segmented control
in the header. Three destinations plus Settings do not justify a rail, and
labels remove the tooltips. The storage indicator becomes a row in the
sidebar footer with its number visible.

The header then carries: current view title on the left, recorder status
and the Clip button on the right. The wordmark moves into the sidebar top
or the window title.

Keyboard: `1 2 3` switch views, `Ctrl+K` command palette later, `/` or
`Ctrl+F` search, `Esc` always goes up one level. Focus rings 2 px accent,
offset 2, only on `:focus-visible`.

### 2.8 Copy

Sentence case everywhere. Labels and buttons have no trailing period;
complete sentences do. Verbs on buttons ("Choose screen", not "Screen").
Numbers with units and a thin space: `3.4 GB`, `0:31`, `144 Hz`. Dates
relative when under a week ("Yesterday 19:20"), absolute otherwise.

---

## 3. Frontend performance

The webview on Linux is WebKitGTK. It is a capable engine with two
well-documented weak spots, and both are things the current UI does.

### 3.1 WebKitGTK rules

1. **No `backdrop-filter`.** Multiple shipped Tauri apps measured it: a
   list scrolling at 50–57 fps dropped to 14 fps with glass panels on
   Intel; folder navigation went from ~450 ms to ~60 ms after removing a
   few blurred controls. Solid fills look the same on dark UI.
2. **No blurred `box-shadow` on repeated elements.** Modrinth found most of
   the frame time in WebKitGTK went to the blur filter while painting a
   shadow, and replaced it with a 1 px border. Keep one shadow token for
   overlays only.
3. **Make sure GPU compositing is actually on.** Luminous discovered it had
   `WEBKIT_DISABLE_COMPOSITING_MODE` and `WEBKIT_DISABLE_DMABUF_RENDERER`
   set unconditionally as an old AppImage workaround; removing them took
   theme-change frames from ~500 ms to ~27 ms and idle CPU from 83% to <1%.
   Tauri's own Linux graphics page says the same: never ship those
   variables unconditionally. clip.sh does not set them. If NVIDIA users
   report blank windows, the first fix is `__NV_DISABLE_EXPLICIT_SYNC=1`
   (no cost), then the DMABUF variable, only for that user.
4. **Scrolling over fixed blurred/shadowed layers is the worst case.** Our
   selection toolbar and toasts are fixed over a scrolling grid: hairline
   plus solid fill, one shadow, no blur.
5. **Perpetual CSS animation keeps the compositor awake.** Remove
   `animate-ping`.
6. **`<video>` decode:** WebKitGTK plays video through GStreamer and uses
   VA-API only when the `va` plugin is present; NVIDIA has no VA-API, so
   playback of 4K60 HEVC/AV1 clips is software-decoded. Keep the trimmer
   preview at display size, and add the proxy-transcode fallback from the
   roadmap for clips the webview reports as dropping frames (we already
   log `getVideoPlaybackQuality`).

### 3.2 Rendering budget

- 60 fps while scrolling the grid on an Intel iGPU; measured with
  `getVideoPlaybackQuality` for video and a `requestAnimationFrame` frame
  timer in dev builds.
- Library of 2 000 clips: first paint < 150 ms after the data arrives,
  scroll without long tasks > 50 ms.
- Idle CPU < 1% with the window open and nothing playing.

How to meet it:

- **Virtualise the grid above ~300 cards.** Svelte 5 has no built-in
  windowing; `@humanspeak/svelte-virtual-list` or a 60-line own
  implementation (row height is known: cards are 16:9 plus 44 px). Below
  that, `content-visibility: auto` with `contain-intrinsic-size` on each
  card lets the engine skip offscreen layout and paint for free.
- **Keep large arrays out of deep `$state`.** A Svelte issue shows 10k-item
  proxied arrays freezing the app. `allClips` should be `$state.raw` and
  replaced wholesale, with filtered/sorted views as `$derived`.
- **Images:** thumbnails at the size they are shown (320×180 for
  comfortable, 240×135 compact), WebP, `loading="lazy"`,
  `decoding="async"`, explicit `width`/`height` so nothing reflows when
  they land. Today thumbnails are JPEG at the capture size.
- **Hover previews** are the right idea but should start after 450 ms
  (done), be limited to one at a time (done by hover), and should use a
  low-bitrate proxy or a sprite sheet rather than the full clip, since a
  100 Mbps HEVC clip is heavy to decode for a 300 px preview. Sprite
  sheets are what every video platform uses: one pass with GStreamer,
  10 frames at 160×90, one WebP, scrub by `background-position`.

### 3.3 IPC and data flow

- **Events, not polling.** Status polls every 2 s, usage every 15 s, the
  library every 60 s. Each poll serialises the whole list over IPC and
  re-renders. The backend already knows when things change: emit
  `status-changed` on every transition (it does), emit `usage-changed`
  after save/delete/enforce, and run a `notify` watcher on the clips
  folder (debounced 300 ms with `notify-debouncer-full`) so external
  changes arrive as `library-changed` within a second. Then remove all
  three intervals.
- **Send deltas for big lists.** `list_clips` returns every clip every
  time. Add `list_clips_since(generation)` or send `clip-added` /
  `clip-removed` events with one `Clip`; a full list only on first load.
  Tauri's guidance is to paginate or stream large payloads, since IPC goes
  through the webview's fetch path.
- **Images through the asset protocol** (already), never base64 through
  commands.
- **Coalesce** bursts: a save triggers `clip-saved`, `library-changed`,
  `usage-changed`; the frontend should debounce re-renders to one frame.

### 3.4 Startup

- Target: window visible with the last library state in < 400 ms on a warm
  cache. Means: cache the last `list_clips` result as JSON in the app data
  dir, render it immediately, reconcile when the real list arrives.
  Encoder probing (test-encodes) must never gate the first paint; it does
  not today, keep it that way.
- Fonts: both Inter and Archivo are bundled; subset to Latin and preload
  the two weights used, or the first paint flashes fallback type.

---

## 4. Backend performance

### 4.1 Capture: zero-copy path

This is the single biggest performance item and the README's known
stutter. Today every frame goes PipeWire → system memory → encoder. The
measured gap, from a project doing exactly our pipeline on an RTX 3060 at
2560×1600: **DMA-BUF → `glupload` → `nvh264enc` held 59.7 fps; system
memory → `cudaupload` → `nvh264enc` managed 37.4 fps.** The copy is the
bottleneck under load, not the encoder.

Plan, by vendor, all inside the existing GStreamer pipeline:

- **NVIDIA:** `pipewiresrc` negotiating `video/x-raw(memory:DMABuf)` →
  `glupload` → `glcolorconvert` → `nvh264enc` / `nvav1enc` (nvcodec accepts
  GL memory via CUDA-GL interop). Fallback to today's path if caps fail.
- **AMD / Intel:** `pipewiresrc` (DMA-BUF) → `vapostproc` → `vah264enc`
  etc. `vapostproc` imports DMA-BUF directly; this is the chain FluxCast
  ships. Known pitfall: `pipewiresrc always-copy=true` yields buffers with
  no memory on DMA-BUF portals, so it must stay `false` on this path.
- **Keep the ring buffer as is.** It stores encoded packets, which are
  small; zero-copy only changes the capture→encode leg.
- Verify with `GST_DEBUG=GST_CAPS:5` that no `videoconvert` sneaks in, and
  measure dropped frames at the encoder (`nvh264enc` exposes QoS stats).

### 4.2 Library index and thumbnails

- `list_clips` stats every file and reads every sidecar on each call, and
  generates missing thumbnails inline, so a first launch with 500 clips
  does 500 decodes before the UI shows anything.
  - Return the list immediately with `thumbnail: null` for anything
    uncached, then generate thumbnails on a bounded worker pool (2 threads;
    GStreamer decode is itself threaded) and emit `thumbnail-ready` per
    clip. The UI already handles a missing thumbnail.
  - Cache the parsed list (path, mtime, size, duration, tracks, meta) in
    one `index.json` or a small SQLite file in the cache dir, keyed by
    mtime+size like the thumbnails are now. Startup then reads one file
    instead of N+N.
- Thumbnails: WebP, 320×180, quality 80, ~8 KB each. Sprite sheet
  alongside for hover scrub. Generate both in one decode pass.
- Watch the clips folder with `notify` (inotify) on a blocking thread, not
  the Tokio runtime, with a 300 ms debounce; the UI never polls.

### 4.3 Threads and the runtime

- Anything that touches the filesystem or GStreamer stays on
  `spawn_blocking` (already the pattern). Audit `list_games` and
  `get_capabilities` for the same.
- Encoder probing results are cached per machine in the config dir with
  the driver version, so the test-encode runs once per driver update, not
  per launch.
- Storage enforcement walks the folder after every save; use the index
  instead of a walk.

### 4.4 Playback

- Serve range requests (done) and `Content-Length`; add `Cache-Control:
  immutable` since clip files never change in place.
- For the trimmer, seek accuracy matters more than throughput: encode
  clips with a 1 s GOP (done), and for `,`/`.` frame stepping request
  `video.requestVideoFrameCallback` where available to land on real frame
  boundaries rather than 1/60 s guesses.

---

## 5. Measurable budgets

| Metric | Budget | How measured |
|---|---|---|
| Capture frame drops under full GPU load | < 0.5% over 60 s | encoder QoS counters, logged |
| Library scroll frame time (p95) | ≤ 16.7 ms | rAF timer in dev builds |
| First paint with cached library | < 400 ms | `performance.now()` at mount, logged |
| 2 000-clip list render | < 150 ms | same |
| Idle CPU, window open | < 1% | `top` |
| Memory, 2 000 clips, 30 s buffer at 50 Mbps | < 450 MB | `smem` |
| Hover preview start | < 300 ms after the 450 ms dwell | manual |
| Any UI animation | ≤ 200 ms | code review |

---

## 6. Plan

Ordered so each step is visible on its own. Steps 1–3 landed first (see
the commit history); the audit table above describes the UI before them.

1. ~~**Tokens and typography pass**~~ Done. Introduce the role-based
   palette in OKLCH, the 4 px scale and the five type sizes as CSS custom
   properties and Tailwind theme values; delete every arbitrary `[…px]`;
   collapse radii to the nesting rule; remove all `backdrop-filter` and all
   but one shadow; remove `animate-ping`. No feature changes, so the diff
   is reviewable screen by screen with the screenshot harness.
2. ~~**Navigation and header**~~ Done. Labelled sidebar with the storage
   row, header carries view title and recorder state, number keys switch
   views.
3. ~~**States**~~ Done. Skeleton grid for first load, designed empty and
   error states for every view, progress with cancel for renders.
4. ~~**Event-driven data**~~ Done. Folder watcher, `usage-changed`,
   thumbnail-ready events, delete the three intervals, `$state.raw` for the
   clip list.
5. ~~**Thumbnail pipeline**~~ Done (WebP + sprite, worker pool, cached first paint in `localStorage` rather than an index file). WebP at display size plus sprite sheet
   in one pass, bounded worker pool, index file for instant startup.
6. ~~**Grid virtualisation**~~ Done: `VirtualGrid` windows rows above 120
   clips; measured 880 clips → 21 cards in the DOM.
7. **Zero-copy capture** (1–2 weeks). NVIDIA GL path first, VA-API second,
   with the measurement harness in `crates/clip-core/examples`.

Steps 1–3 are pure polish and cost nothing in performance. Steps 4–6 are
where "fast" becomes measurable. Step 7 is the one that changes what the
app can capture.

---

## Sources

Design craft
- Linear, "How we redesigned the Linear UI": https://linear.app/blog/how-we-redesigned-the-linear-ui
- Linear design breakdown (third-party, 4 px grid, density): https://www.925studios.co/blog/linear-design-breakdown-saas-ui-2026
- Raycast design teardown (hairline borders at 6–10% white, keyboard-first): https://design.hagicode.com/designs/raycast/
- *Refactoring UI* principles summary (spacing scale, grayscale first, depth): https://smithery.ai/skills/wondelai/refactoring-ui
- Emil Kowalski, "Great animations": https://emilkowal.ski/ui/great-animations
- Animation duration/easing summary: https://skills.cat/skills/mrmps/smry/emilkowal-animations
- Polish checklist (optical alignment, states, copy): https://playbooks.com/skills/pbakaus/impeccable/polish
- Nielsen Norman Group, response-time limits: https://www.nngroup.com/articles/response-times-3-important-limits/
- Skeleton screens evidence, both directions: https://uxdesign.cc/what-you-should-know-about-skeleton-screens-a820c45a571a and https://dev.to/helloashish99/skeleton-screens-vs-spinners-perceived-load-time-1nlj
- KDE HIG typography: https://develop.kde.org/hig/style/typography

Frontend performance
- Tauri, Linux graphics issues (env vars, shipping advice): https://v2.tauri.app/develop/debug/linux-graphics/
- Luminous PR: GPU compositing on Linux, jank fixes, measurements: https://github.com/esoltys/luminous/pull/1159
- Modrinth: box-shadow blur cost in WebKitGTK: https://github.com/modrinth/code/pull/4712
- Backdrop-blur disabled on Linux for perf (block/buzz): https://github.com/block/buzz/pull/3533
- Folder navigation 450 ms → 60 ms after removing blur (wingdrive): https://github.com/bernardopg/wingdrive/pull/199
- Svelte 5 large proxied arrays: https://github.com/sveltejs/svelte/issues/10637
- Svelte virtual list: https://github.com/humanspeak/svelte-virtual-list
- Tauri IPC vs JSI discussion: https://github.com/orgs/tauri-apps/discussions/11915
- Tauri IPC patterns (channels for streaming): https://dev.to/hiyoyok/ipc-in-tauri-tauri-commands-vs-custom-ipc-what-to-use-when-2ab4
- WebKitGTK VA-API decode: https://bugs.webkit.org/show_bug.cgi?id=254340
- Sprite-sheet previews in one pass: https://dev.to/masonwritescode/build-scrub-bar-thumbnail-previews-with-ffmpeg-and-a-webvtt-sprite-3ei2

Backend performance
- DMA-BUF → glupload → NVENC vs system memory, measured: https://github.com/zackees/docker-vm/issues/18
- VA-API zero-copy chain (pipewiresrc → vapostproc → vah264enc): https://github.com/IlyaP358/fluxcast/pull/170
- `always-copy=true` yields NULL-memory buffers on DMA-BUF portals: https://github.com/omarroth/doubletake/issues/40
- `notify` debouncing in Rust: https://oneuptime.com/blog/post/2026-01-25-file-watcher-debouncing-rust/view
- File watching in a Tauri app (blocking thread): https://dev.to/hiyoyok/file-watching-in-rust-with-notify-rs-hot-folders-for-a-sync-app-32d8
- SQLite-indexed media library (Tauri + Rust): https://github.com/hwanjongyu/MediaHub
