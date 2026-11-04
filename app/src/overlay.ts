// The in-game popup window. The backend positions and shows it (without
// taking focus from the game) and sends what to say; this page only draws
// the card and fades it out before the window is hidden again.

import { listen } from '@tauri-apps/api/event'
import './overlay.css'
import { ACCENTS, type Accent } from './lib/prefs.svelte'

interface Popup {
  kind: 'ready' | 'saved' | 'screenshot' | 'marked'
  title: string
  detail: string
  /** Hotkey to show as a key cap after the detail, if any. */
  key: string | null
  /** How long the window stays up. */
  ms: number
}

const popup = document.getElementById('popup')!
const title = document.getElementById('title')!
const detail = document.getElementById('detail')!
let fade: ReturnType<typeof setTimeout> | undefined

/** The user's accent and motion preference, from the main window's storage. */
function applyPrefs() {
  let accent: Accent = 'orange'
  let reduceMotion = false
  try {
    const p = JSON.parse(localStorage.getItem('reruns/prefs') ?? '{}')
    if (p.accent in ACCENTS) accent = p.accent
    reduceMotion = !!p.reduceMotion
  } catch {
    // Storage unavailable: defaults.
  }
  document.documentElement.style.setProperty('--accent', ACCENTS[accent].css)
  document.documentElement.classList.toggle('reduce-motion', reduceMotion)
}

export function show(p: Popup) {
  applyPrefs()
  popup.dataset.kind = p.kind
  title.textContent = p.title
  detail.textContent = ''
  // The key hint comes first: a long game name is what gets cut short.
  if (p.key) {
    const kbd = document.createElement('kbd')
    kbd.textContent = p.key
    detail.append('Press ', kbd, ' to clip')
    if (p.detail) detail.append(' · ')
  }
  detail.append(p.detail)
  // Restart the entrance even if a popup is already showing.
  popup.classList.remove('in')
  void popup.offsetWidth
  popup.classList.add('in')
  clearTimeout(fade)
  fade = setTimeout(() => popup.classList.remove('in'), Math.max(0, p.ms - 220))
}

listen<Popup>('overlay-show', (e) => show(e.payload))
