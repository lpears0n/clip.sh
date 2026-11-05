// Getting a clip out of the app: drag it into Discord, a chat or a folder.
// The webview can't start an OS file drag itself, so the drag plugin does,
// with a small preview drawn here (no image loading that could fail).
import { startDrag } from '@crabnebula/tauri-plugin-drag'

let icon: string | null = null

/** A 96×54 PNG: the app's dark tile with an accent play mark. */
function dragIcon(): string {
  if (icon) return icon
  const c = document.createElement('canvas')
  c.width = 96
  c.height = 54
  const g = c.getContext('2d')
  if (!g) return ''
  g.fillStyle = '#17181b'
  g.beginPath()
  g.roundRect(0, 0, 96, 54, 8)
  g.fill()
  g.fillStyle = getComputedStyle(document.documentElement).getPropertyValue('--color-accent').trim() || '#ff7a1a'
  g.beginPath()
  g.moveTo(40, 15)
  g.lineTo(40, 39)
  g.lineTo(60, 27)
  g.closePath()
  g.fill()
  icon = c.toDataURL('image/png')
  return icon
}

/** Start dragging the clip at `path` out of the window, as a file. */
export function dragOut(path: string) {
  startDrag({ item: [path], icon: dragIcon() }).catch(() => {})
}
