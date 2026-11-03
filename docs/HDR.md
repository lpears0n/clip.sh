# HDR

Status and plan for HDR games. Windows first; Linux (KWin HDR) later.

## Where it stands

- **Recording is SDR only.** The Windows capture element,
  `d3d11screencapturesrc` (GStreamer 1.26), hands over 8-bit BGRA in
  both its WGC and DXGI paths (the DXGI path even carries a FIXME about
  10-bit). The encoder writes 8-bit H.264/HEVC/AV1 and nothing in the
  pipeline carries colour-space metadata.
- **On an HDR display, HDR games clip.** Windows converts the HDR
  composition to the 8-bit capture format without tone mapping, so
  highlights above SDR white blow out and colours can look off. SDR
  games on an HDR desktop come out roughly right.
- **Detection exists** (this is step 1 below): when recording starts,
  clip.sh checks whether the game's monitor runs in HDR (DXGI output
  colour space BT.2020/PQ), reads Windows' SDR white level and the
  display's peak brightness, logs them, and shows an "HDR display" row
  under Settings → Recording.

## What GStreamer offers (checked against the 1.26, 1.28 and main sources)

- `d3d11screencapturesrc`: BGRA only, WGC and DXGI. No HDR.
- `d3d12screencapturesrc`: `{ BGRA, RGBA64_LE }`. On an HDR monitor its
  **DXGI** path duplicates in `R16G16B16A16_FLOAT` and tone-maps to SDR
  (`tonemap` = linear or Reinhard, using the SDR white level), output
  labelled sRGB. Its **WGC** path is still `B8G8R8A8`, so window capture
  gets no HDR. Unchanged on main.

So stock elements give HDR-correct output only for whole-monitor DXGI
capture, and clip.sh records only the game's window (owner's decision:
like Medal, nothing else on screen ends up in clips).

## Plan

1. **Detect and report** (done). Gives the owner's machine a way to say
   whether it is affected, and gives step 2 its parameters.
2. **Tone-mapped SDR from HDR games, window capture kept.** clip.sh's own
   WGC window capture (Rust, `windows` crate) requesting
   `R16G16B16A16Float` frames (scRGB, linear, BT.709 primaries, 1.0 =
   80 nits), a D3D11 pixel shader that scales by the SDR white level,
   tone-maps highlights (Reinhard or a filmic curve with the display's
   peak as the knee), converts to sRGB and writes BGRA, then hands the
   texture to GStreamer as D3D11 memory (`gst_d3d11_device_new_wrapped`,
   `gst_d3d11_allocator_alloc_wrapped`) so the existing zero-copy chain
   into NVENC/AMF/QSV is unchanged. Only used when the display is in
   HDR; SDR displays keep `d3d11screencapturesrc`. This is what Medal
   does, and the result plays correctly everywhere (Discord, phones).
3. **Optional HDR10 recording.** Same FP16 capture; the shader writes
   BT.2020 primaries with the PQ curve into P010; HEVC Main10 or AV1
   10-bit with mastering-display and content-light metadata
   (`nvd3d11h265enc` and the AMF/QSV elements take these from caps);
   `mp4mux` writes `colr`. The app's player and thumbnails then need to
   tone-map for SDR screens. Off by default: most places clips are
   shared show HDR video washed out.

Per-frame cost of step 2 on the GPU is one full-screen shader pass,
the same order as the `d3d11convert` the zero-copy path already runs.

## Open questions

- What WGC's 8-bit capture of an HDR game actually looks like on the
  owner's machine (blown out vs washed out): a clip from an HDR game
  with HDR on answers it.
- Whether `nvd3d11h265enc` accepts mastering-display caps on the
  bundled 1.26 (needed only for step 3).
