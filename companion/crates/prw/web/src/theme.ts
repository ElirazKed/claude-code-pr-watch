// The page's theme: a palette (themes.css), dark or light or as the system is. Kept in this
// browser only.

import { useEffect, useState } from 'react'

export type Palette = 'graphite' | 'mono' | 'nord'
export type Mode = 'system' | 'light' | 'dark'

export interface Theme {
  palette: Palette
  mode: Mode
}

/** The palettes, each with the colours its swatch shows: ground, accent, a failure, a pass. */
export const PALETTES: { id: Palette; label: string; swatch: string[] }[] = [
  { id: 'graphite', label: 'Graphite', swatch: ['#222326', '#7fa8dc', '#dd7b7b', '#8dbd85'] },
  { id: 'mono', label: 'Mono', swatch: ['#1c1c1c', '#e3a54a', '#cf8a8a', '#9fb59c'] },
  { id: 'nord', label: 'Nord', swatch: ['#2b303b', '#88c0d0', '#d57780', '#a3be8c'] },
]

export const MODES: { id: Mode; label: string }[] = [
  { id: 'system', label: 'System' },
  { id: 'light', label: 'Light' },
  { id: 'dark', label: 'Dark' },
]

const KEY = 'prw-theme'
const DEFAULT: Theme = { palette: 'graphite', mode: 'system' }

export function readTheme(): Theme {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? 'null') as Partial<Theme> | null
    return {
      palette: PALETTES.some((p) => p.id === saved?.palette) ? saved!.palette! : DEFAULT.palette,
      mode: MODES.some((m) => m.id === saved?.mode) ? saved!.mode! : DEFAULT.mode,
    }
  } catch {
    return DEFAULT
  }
}

export function saveTheme(theme: Theme) {
  try {
    localStorage.setItem(KEY, JSON.stringify(theme))
  } catch {
    // Not kept: it lasts until a reload.
  }
}

const systemDark = () => matchMedia('(prefers-color-scheme: dark)').matches

/** Puts a theme on the page: its palette, and light or dark as its mode (or the system) says. */
export function applyTheme(theme: Theme) {
  const root = document.documentElement
  root.dataset.palette = theme.palette
  const isDark = theme.mode === 'system' ? systemDark() : theme.mode === 'dark'
  root.dataset.theme = isDark ? 'dark' : 'light'
}

/** The theme picked, put on the page and kept; the page follows the system while it says so. */
export function useTheme(): [Theme, (theme: Theme) => void] {
  const [theme, setTheme] = useState<Theme>(readTheme)
  useEffect(() => {
    applyTheme(theme)
    if (theme.mode !== 'system') return
    const query = matchMedia('(prefers-color-scheme: dark)')
    const follow = () => applyTheme(theme)
    query.addEventListener('change', follow)
    return () => query.removeEventListener('change', follow)
  }, [theme])
  const pick = (next: Theme) => {
    setTheme(next)
    saveTheme(next)
  }
  return [theme, pick]
}
