// Typed wrappers around the Tauri commands in src-tauri/src/lib.rs.

import { convertFileSrc, invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

export type RecorderState = 'unavailable' | 'waiting' | 'recording'

export interface Status {
  state: RecorderState
  game: string | null
  steam_app_id: number | null
  session_start_ms: number | null
  buffered_seconds: number
  replay_seconds: number
  encoder: string | null
  error: string | null
  /** "kwin": game windows (KDE). "portal": the chosen screen. */
  backend: 'kwin' | 'portal' | null
  /** Portal capture needs a screen chosen once. */
  needs_screen: boolean
}

export interface Clip {
  path: string
  game: string
  name: string
  created_ms: number
  size_bytes: number
  duration: number | null
  audio_tracks: number
  is_edit: boolean
  steam_app_id: number | null
  thumbnail: string | null
  /** One row of `sprite_frames` frames for hover scrubbing. */
  sprite: string | null
  sprite_frames: number
  /** User-given name; show the game or time when null. */
  title: string | null
  favorite: boolean
  labels: string[]
}

/** Fields of a clip the user can change; omitted ones are left alone. */
export interface ClipPatch {
  /** Empty string clears the title. */
  title?: string
  favorite?: boolean
  labels?: string[]
}

export interface Settings {
  replay_seconds: number
  mic: boolean
  clips_dir: string
  hotkey: string
  capture_backend: 'kwin' | 'portal' | null
  launch_at_login: boolean
  start_in_tray: boolean
  close_to_tray: boolean
  notify_on_save: boolean
  sound_on_save: boolean
  fps: number
  bitrate_kbps: number
  codec: Codec
  encoder: Encoder | null
  cursor: boolean
  desktop_audio: boolean
  mic_device: string | null
  audio_bitrate_kbps: number
  storage_limit_gb: number | null
  protect_edits: boolean
  use_trash: boolean
  ads: boolean
}

export type Codec = 'h264' | 'hevc' | 'av1'
export type Encoder = 'nvenc' | 'vaapi' | 'x264'

export interface Capabilities {
  display_hz: number | null
  encoders: { encoder: Encoder; codecs: Codec[] }[]
  mic_devices: { id: string; name: string }[]
}

export interface Session {
  game: string
  steam_app_id: number | null
  start_ms: number
  end_ms: number
}

export interface GameEntry {
  name: string
  steam_app_id: number | null
  art: { capsule: string | null; hero: string | null; logo: string | null }
  fallback_image: string | null
  last_played_ms: number | null
  last_activity_ms: number
  total_play_ms: number
  clip_count: number
  clip_bytes: number
  sessions: Session[]
  playing: boolean
}

export interface Usage {
  clips_bytes: number
  clip_count: number
  disk_free: number
  disk_total: number
  limit_bytes: number | null
}

export const api = {
  storage: () => invoke<Usage>('get_storage'),
  capabilities: () => invoke<Capabilities>('get_capabilities'),
  chooseScreen: () => invoke<void>('choose_screen'),
  hotkeyBackend: () => invoke<'kglobalaccel' | 'portal' | null>('hotkey_backend'),
  status: () => invoke<Status>('get_status'),
  save: () => invoke<string>('save_clip'),
  clips: () => invoke<Clip[]>('list_clips'),
  games: () => invoke<GameEntry[]>('list_games'),
  deleteClip: (path: string) => invoke<void>('delete_clip', { path }),
  updateClip: (path: string, patch: ClipPatch) => invoke<Clip>('update_clip', { path, patch }),
  reveal: (path: string) => invoke<void>('reveal_clip', { path }),
  openFolder: () => invoke<void>('open_clips_folder'),
  settings: () => invoke<Settings>('get_settings'),
  setSettings: (settings: Settings) => invoke<void>('set_settings', { settings }),
  openForEdit: (path: string) => invoke<Clip>('open_for_edit', { path }),
  render: (path: string, start: number, end: number, volumes: number[]) =>
    invoke<string>('render_clip', { path, start, end, volumes }),
  cancelRender: () => invoke<void>('cancel_render'),
}

export const onRenderProgress = (f: (p: { path: string; fraction: number }) => void): Promise<UnlistenFn> =>
  listen<{ path: string; fraction: number }>('render-progress', (e) => f(e.payload))

export const onStatusChanged = (f: (s: Status) => void): Promise<UnlistenFn> =>
  listen<Status>('status-changed', (e) => f(e.payload))

export const onLibraryChanged = (f: () => void): Promise<UnlistenFn> =>
  listen('library-changed', () => f())

export const onClipSaved = (f: (path: string) => void): Promise<UnlistenFn> =>
  listen<string>('clip-saved', (e) => f(e.payload))

/** A clip's thumbnail, sprite and duration were generated in the background. */
export const onThumbnailReady = (f: (c: Clip) => void): Promise<UnlistenFn> =>
  listen<Clip>('thumbnail-ready', (e) => f(e.payload))

/** URL the webview can load a local image from. */
export const fileUrl = (path: string) => convertFileSrc(path)

// WebKitGTK can't stream <video> from the asset protocol, so videos come
// from the app's loopback media server.
let mediaBase = ''
export async function initMedia() {
  mediaBase = await invoke<string>('media_base')
}
export const videoUrl = (path: string) => mediaBase + encodeURIComponent(path)
