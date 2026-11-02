// The last library listing, kept per machine so the window can paint the
// grid before the backend has answered. Reconciled as soon as it does.

import type { Clip, GameEntry } from './api'

const KEY = 'clip.sh/library'
/** Don't let a huge library make startup slower than it saves. */
const MAX_CLIPS = 2000

export interface CachedLibrary {
  clips: Clip[]
  games: GameEntry[]
}

export function readLibraryCache(): CachedLibrary | null {
  try {
    const raw = localStorage.getItem(KEY)
    if (!raw) return null
    const v = JSON.parse(raw) as CachedLibrary
    return Array.isArray(v.clips) && Array.isArray(v.games) ? v : null
  } catch {
    return null
  }
}

export function writeLibraryCache(clips: Clip[], games: GameEntry[]) {
  try {
    localStorage.setItem(KEY, JSON.stringify({ clips: clips.slice(0, MAX_CLIPS), games }))
  } catch {
    // Quota or private mode: next launch just loads the slow way.
  }
}
