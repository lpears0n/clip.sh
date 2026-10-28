import type { Capabilities, Codec, Settings } from './api'

export const CODEC_LABEL: Record<Codec, string> = { h264: 'H.264', hevc: 'HEVC', av1: 'AV1' }
export const ENCODER_LABEL = { nvenc: 'NVIDIA NVENC', vaapi: 'VA-API', x264: 'x264 (CPU)' } as const

export const FPS_OPTIONS = [30, 60, 120, 144, 165, 240, 360, 480]

export type PresetId = 'balanced' | 'high' | 'max'
export interface Preset {
  id: PresetId
  label: string
  fps: number
  bitrate_kbps: number
  codec: Codec
}

/** Codecs some encoder on this machine can actually produce. */
export function supportedCodecs(caps: Capabilities | null): Set<Codec> {
  if (!caps) return new Set(['h264'])
  return new Set(caps.encoders.flatMap((e) => e.codecs))
}

/** Presets adapt to the hardware: the display's refresh rate and the best
 *  codec available. */
export function presets(caps: Capabilities | null): Preset[] {
  const codecs = supportedCodecs(caps)
  const hz = caps?.display_hz ? Math.round(caps.display_hz) : 60
  const best = (...order: Codec[]) => order.find((c) => codecs.has(c)) ?? 'h264'
  const nativeFps = FPS_OPTIONS.filter((f) => f <= hz + 1).at(-1) ?? 60
  return [
    { id: 'balanced', label: 'Balanced', fps: 60, bitrate_kbps: 20_000, codec: 'h264' },
    { id: 'high', label: 'High', fps: Math.min(120, nativeFps), bitrate_kbps: 50_000, codec: best('hevc', 'h264') },
    { id: 'max', label: 'Maximum', fps: nativeFps, bitrate_kbps: 100_000, codec: best('av1', 'hevc', 'h264') },
  ]
}

export function activePreset(s: Settings, caps: Capabilities | null): PresetId | 'custom' {
  const p = presets(caps).find(
    (p) => p.fps === s.fps && p.bitrate_kbps === s.bitrate_kbps && p.codec === s.codec,
  )
  return p?.id ?? 'custom'
}

/** Bytes for `seconds` of recording at the current settings (video + audio). */
export function bytesFor(s: Settings, seconds: number): number {
  const tracks = (s.desktop_audio ? 1 : 0) + (s.mic ? 1 : 0)
  const kbps = s.bitrate_kbps + tracks * s.audio_bitrate_kbps
  return (kbps * 1000 * seconds) / 8
}
