# Copies the GStreamer runtime the Windows build needs into app/src-tauri/gst,
# where tauri.windows.conf.json bundles it next to the executable.
#
#   pwsh scripts/collect-gstreamer.ps1 [-Root $env:GSTREAMER_1_0_ROOT_MSVC_X86_64]
#
# Everything in bin/ is taken (the core libraries and their dependencies,
# about 100 MB); plugins are limited to the ones clip.sh uses, which keeps
# the installer well under the full 400 MB runtime.
param(
  [string]$Root = $env:GSTREAMER_1_0_ROOT_MSVC_X86_64
)
$ErrorActionPreference = 'Stop'
if (-not $Root) { throw 'Set GSTREAMER_1_0_ROOT_MSVC_X86_64 or pass -Root' }

$dest = Join-Path $PSScriptRoot '..\app\src-tauri\gst'
New-Item -ItemType Directory -Force "$dest\bin" | Out-Null
New-Item -ItemType Directory -Force "$dest\plugins" | Out-Null

Copy-Item "$Root\bin\*.dll" "$dest\bin\" -Force

$plugins = @(
  # core and glue
  'coreelements', 'app', 'typefindfunctions', 'playback', 'videoconvertscale',
  'videorate', 'videocrop', 'videotestsrc', 'videoparsersbad', 'audioconvert',
  'audioresample', 'audiomixer', 'volume', 'audiotestsrc', 'audioparsers',
  'isomp4', 'libav', 'webp', 'jpeg', 'png',
  # capture and encode on Windows
  'd3d11', 'd3d12', 'nvcodec', 'amfcodec', 'qsv', 'mediafoundation', 'wasapi2',
  'x264', 'openh264', 'fdkaac', 'voaacenc', 'dxgiscreencap'
)
foreach ($p in $plugins) {
  $f = Join-Path $Root "lib\gstreamer-1.0\gst$p.dll"
  if (Test-Path $f) { Copy-Item $f "$dest\plugins\" -Force } else { Write-Host "skipping absent plugin $p" }
}
Write-Host "collected $(@(Get-ChildItem "$dest\bin" -Filter *.dll).Count) libraries and $(@(Get-ChildItem "$dest\plugins" -Filter *.dll).Count) plugins"

# Every element the app makes by name must come from the bundled plugins
# alone (hardware encoders and decoders are left out: they only appear on a
# machine with that GPU). A missing one here would otherwise only show up as
# "no working encoder" on a user's machine.
$required = @(
  'videotestsrc', 'audiotestsrc', 'capsfilter', 'fakesink', 'filesink', 'queue', 'appsrc', 'appsink',
  'videoconvert', 'videoscale', 'videorate', 'videocrop',
  'h264parse', 'h265parse', 'av1parse', 'aacparse', 'mp4mux', 'uridecodebin', 'decodebin',
  'audioconvert', 'audioresample', 'audiomixer', 'volume',
  'd3d11screencapturesrc', 'd3d11convert', 'd3d11upload', 'wasapi2src',
  'pngenc', 'jpegenc', 'x264enc', 'mfh264enc', 'avdec_h264', 'avenc_aac'
)
$env:GST_PLUGIN_PATH = "$dest\plugins"
$env:GST_PLUGIN_SYSTEM_PATH = "$dest\plugins"
$env:GST_REGISTRY = Join-Path $env:TEMP 'clipsh-bundle-registry.bin'
Remove-Item $env:GST_REGISTRY -ErrorAction SilentlyContinue
$missing = @()
foreach ($e in $required) {
  & "$Root\bin\gst-inspect-1.0.exe" $e *> $null
  if ($LASTEXITCODE -ne 0) { $missing += $e }
}
if ($missing.Count -gt 0) { throw "elements missing from the bundle: $($missing -join ', ')" }
Write-Host "all $($required.Count) required elements load from the bundle"
