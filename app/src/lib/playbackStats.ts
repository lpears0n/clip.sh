import { invoke } from '@tauri-apps/api/core'

/** Svelte action: while a <video> plays, track frames rendered vs dropped
 *  (getVideoPlaybackQuality) and log them to the app log on pause/end. Used
 *  to diagnose stutter in the webview's video path. */
export function playbackStats(video: HTMLVideoElement, path: string) {
  let current = path
  let start: { at: number; total: number; dropped: number; corrupted: number } | null = null

  const onplay = () => {
    const q = video.getVideoPlaybackQuality()
    start = { at: performance.now(), total: q.totalVideoFrames, dropped: q.droppedVideoFrames, corrupted: q.corruptedVideoFrames }
  }
  const onstop = () => {
    if (!start) return
    const q = video.getVideoPlaybackQuality()
    const seconds = (performance.now() - start.at) / 1000
    if (seconds > 1) {
      invoke('log_playback', {
        path: current,
        total: q.totalVideoFrames - start.total,
        dropped: q.droppedVideoFrames - start.dropped,
        corrupted: q.corruptedVideoFrames - start.corrupted,
        seconds,
      }).catch(() => {})
    }
    start = null
  }

  video.addEventListener('play', onplay)
  video.addEventListener('pause', onstop)
  video.addEventListener('ended', onstop)
  return {
    update(p: string) {
      current = p
    },
    destroy() {
      onstop()
      video.removeEventListener('play', onplay)
      video.removeEventListener('pause', onstop)
      video.removeEventListener('ended', onstop)
    },
  }
}
