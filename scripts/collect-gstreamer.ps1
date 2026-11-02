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
  'videorate', 'videocrop', 'videoparsersbad', 'audioconvert', 'audioresample',
  'audioparsers', 'isomp4', 'libav', 'webp', 'jpeg', 'png',
  # capture and encode on Windows
  'd3d11', 'd3d12', 'nvcodec', 'amfcodec', 'qsv', 'mediafoundation', 'wasapi2',
  'x264', 'openh264', 'fdkaac', 'voaacenc', 'dxgiscreencap'
)
foreach ($p in $plugins) {
  $f = Join-Path $Root "lib\gstreamer-1.0\gst$p.dll"
  if (Test-Path $f) { Copy-Item $f "$dest\plugins\" -Force } else { Write-Host "skipping absent plugin $p" }
}
Write-Host "collected $(@(Get-ChildItem "$dest\bin" -Filter *.dll).Count) libraries and $(@(Get-ChildItem "$dest\plugins" -Filter *.dll).Count) plugins"
