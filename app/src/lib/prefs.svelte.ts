// Appearance preferences. These only affect this machine's UI, so they live
// in localStorage rather than the recorder's settings file.

// OKLCH so every accent has the same lightness and a capped chroma (≤ 0.2):
// none of them glares on the dark surfaces, and dark text reads on all.
export const ACCENTS = {
  orange: { label: 'Orange', css: 'oklch(72% 0.19 50)' },
  blue: { label: 'Blue', css: 'oklch(70% 0.16 250)' },
  green: { label: 'Green', css: 'oklch(74% 0.17 155)' },
  pink: { label: 'Pink', css: 'oklch(68% 0.2 5)' },
  purple: { label: 'Purple', css: 'oklch(70% 0.15 295)' },
  yellow: { label: 'Yellow', css: 'oklch(82% 0.16 90)' },
} as const
export type Accent = keyof typeof ACCENTS

export type Density = 'comfortable' | 'compact'

export interface Prefs {
  accent: Accent
  /** Sidebar collapsed to icons. */
  sidebarCollapsed: boolean
  /** Play a muted preview when the pointer rests on a clip card. */
  hoverPreviews: boolean
  /** Shorten or disable transitions and animations. */
  reduceMotion: boolean
  /** Card size in the clip grids. */
  density: Density
}

const KEY = 'clip.sh/prefs'

const DEFAULTS: Prefs = {
  accent: 'orange',
  sidebarCollapsed: false,
  hoverPreviews: true,
  reduceMotion: false,
  density: 'comfortable',
}

function load(): Prefs {
  try {
    const raw = localStorage.getItem(KEY)
    if (!raw) return { ...DEFAULTS }
    const parsed = JSON.parse(raw) as Partial<Prefs>
    return {
      ...DEFAULTS,
      ...parsed,
      accent: parsed.accent && parsed.accent in ACCENTS ? parsed.accent : DEFAULTS.accent,
    }
  } catch {
    return { ...DEFAULTS }
  }
}

export const prefs = $state<Prefs>(load())

export function setPrefs(patch: Partial<Prefs>) {
  Object.assign(prefs, patch)
  try {
    localStorage.setItem(KEY, JSON.stringify(prefs))
  } catch {
    // Private mode or blocked storage: the choice just lasts this session.
  }
}

/** Minimum card width for the clip grids, per density. */
export const CARD_MIN: Record<Density, number> = { comfortable: 300, compact: 220 }

/** Push the preferences into CSS custom properties / attributes on <html>. */
export function applyPrefs(p: Prefs) {
  const root = document.documentElement
  root.style.setProperty('--color-accent', ACCENTS[p.accent].css)
  root.style.setProperty('--card-min', `${CARD_MIN[p.density]}px`)
  const systemReduce = matchMedia('(prefers-reduced-motion: reduce)').matches
  root.toggleAttribute('data-reduce-motion', p.reduceMotion || systemReduce)
}
