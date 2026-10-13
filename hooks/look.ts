// The pane's true-colour paint: gradient bars, a shimmer over pending checks and a
// hue-cycling spinner, all packed as Raster cells so the animation can blit frames. The
// colours, glyphs and the background behind the cells are the theme's (theme.ts).
import type { Checks } from '../types'
import type { Ramp, RasterPaint, Rgb } from './theme'

export type Cell = readonly [codePoint: number, fg: number, bg: number]

// A cell's colour for the terminal's own background (or foreground).
export const DEFAULT_BG = 0x01000000

// What a CI bar is painted with: the theme's ramps, and the card's fill behind (null: the
// terminal's own background).
export type BarPaint = Pick<RasterPaint, 'green' | 'red' | 'amber' | 'shine'> & { bg: number | null }

export const clamp01 = (n: number) => Math.min(1, Math.max(0, n))

export function lerp(a: Rgb, b: Rgb, t: number): Rgb {
  const k = clamp01(t)

  return [0, 1, 2].map(i => Math.round(a[i]! + (b[i]! - a[i]!) * k)) as unknown as Rgb
}

export function hex([r, g, b]: Rgb): number {
  return (r << 16) | (g << 8) | b
}

// The xterm cube's six levels a channel.
const LEVELS = [0, 95, 135, 175, 215, 255] as const

// The xterm 256-colour palette's colour `n` (16 to 255: the 6×6×6 cube, then 24 greys), as the
// terminal paints it. Where Claude Code draws 256 colours (inside tmux, by default) it rounds
// every colour to one of these; one of them stays itself.
export function x256(n: number): Rgb {
  if (n >= 232) {
    const v = 8 + (n - 232) * 10

    return [v, v, v]
  }
  const i = Math.max(0, n - 16)

  return [LEVELS[Math.floor(i / 36) % 6]!, LEVELS[Math.floor(i / 6) % 6]!, LEVELS[i % 6]!]
}

// The palette colour Claude Code paints `rgb` as in 256 colours: the cube's nearest level on
// each channel, or the grey ramp's nearest where that is closer (as its painter rounds).
export function nearest256([r, g, b]: Rgb): Rgb {
  const level = (c: number) => (c < 48 ? 0 : c < 115 ? 1 : c < 155 ? 2 : c < 195 ? 3 : c < 235 ? 4 : 5)
  const cube: Rgb = [LEVELS[level(r)]!, LEVELS[level(g)]!, LEVELS[level(b)]!]
  const grey = 8 + 10 * Math.max(0, Math.min(23, Math.round(((r + g + b) / 3 - 8) / 10)))
  const far = (to: Rgb) => (r - to[0]) ** 2 + (g - to[1]) ** 2 + (b - to[2]) ** 2

  return far([grey, grey, grey]) < far(cube) ? [grey, grey, grey] : cube
}

export function gradientAt(stops: readonly Rgb[], t: number): Rgb {
  const span = (stops.length - 1) * clamp01(t)
  const i = Math.min(stops.length - 2, Math.floor(span))

  return lerp(stops[i]!, stops[i + 1]!, span - i)
}

export function encode(cells: readonly Cell[]): string {
  const words = new Uint32Array(cells.length * 3)
  cells.forEach(([cp, fg, bg], i) => words.set([cp, fg, bg], i * 3))
  const bytes = new Uint8Array(words.buffer)
  let binary = ''
  for (const byte of bytes) binary += String.fromCharCode(byte)

  return btoa(binary)
}

const cell = (glyph: string, rgb: Rgb, bg: number | null = null): Cell => [glyph.codePointAt(0)!, hex(rgb), bg ?? DEFAULT_BG]

// Splits `width` cells among the counts, every non-zero count keeping at least one.
export function allot(counts: readonly number[], width: number): number[] {
  const total = counts.reduce((a, b) => a + b, 0)
  if (total === 0) return counts.map(() => 0)
  const raw = counts.map(n => (n / total) * width)
  const out = raw.map((r, i) => (counts[i]! > 0 ? Math.max(1, Math.floor(r)) : 0))
  let left = width - out.reduce((a, b) => a + b, 0)
  const order = raw.map((r, i) => [r - Math.floor(r), i] as const).sort((a, b) => b[0] - a[0])
  for (let k = 0; left > 0 && k < order.length * 4; k++) {
    const i = order[k % order.length]![1]
    if (counts[i]! > 0) {
      out[i]! += 1
      left -= 1
    }
  }
  while (left < 0) {
    const i = out.indexOf(Math.max(...out))
    out[i]! -= 1
    left += 1
  }

  return out
}

// One row of `━`: passed, failed, then pending (with a light sweeping across it).
export function barCells(checks: Checks, width: number, frame: number, paint: BarPaint): string {
  const [passed = 0, failed = 0, pending = 0] = allot([checks.passed, checks.failed, checks.pending], width)
  const cells: Cell[] = []
  const run = (n: number, [from, to]: Ramp, shine?: number) => {
    for (let i = 0; i < n; i++) {
      let rgb = lerp(from, to, n === 1 ? 1 : i / (n - 1))
      if (shine !== undefined) rgb = lerp(rgb, paint.shine, Math.max(0, 1 - Math.abs(i - shine) / 3) * 0.85)
      cells.push(cell('━', rgb, paint.bg))
    }
  }
  run(passed, paint.green)
  run(failed, paint.red)
  run(pending, paint.amber, (frame % (pending + 8)) - 4)

  return encode(cells)
}

export function fullBarCells(width: number, colours: Ramp, bg: number | null = null): string {
  return encode(Array.from({ length: width }, (_, i) => cell('━', lerp(colours[0], colours[1], i / Math.max(1, width - 1)), bg)))
}

// The header's rule: the theme's `glyphs` repeated across (one, or a pattern such as a wave's
// heights), through its gradient stops.
export function ruleCells(width: number, stops: readonly Rgb[], glyphs: string): string {
  const pattern = [...glyphs]

  return encode(Array.from({ length: width }, (_, i) => cell(pattern[i % pattern.length]!, gradientAt(stops, i / Math.max(1, width - 1)))))
}

// The rule as text, `width` cells of the pattern, where no Raster draws.
export function ruleText(width: number, glyphs: string): string {
  const pattern = [...glyphs]

  return Array.from({ length: width }, (_, i) => pattern[i % pattern.length]).join('')
}

// The gh meter's paint: a ramp per health (look.ts's green, amber and red), and the track.
export type MeterPaint = Pick<RasterPaint, 'green' | 'amber' | 'red' | 'track' | 'isPalette'>
export type Health = 'warning' | 'error' | null

// One login's budget as one thin bar: the top half of each cell of a row of `▀`, the share left
// coloured by its health over the track, the cell where it ends blended by how much of it is
// filled; the bottom half is the terminal's own background, so the bars of two logins on rows
// one over the other stay apart. Not known yet: no bar.
export function meterCells(bar: { share: number; health: Health } | null, width: number, paint: MeterPaint): string {
  const ramp = bar?.health === 'error' ? paint.red : bar?.health === 'warning' ? paint.amber : paint.green
  const at = (i: number): Cell => {
    if (bar === null) return [0x20, DEFAULT_BG, DEFAULT_BG]
    const filled = clamp01(bar.share) * width
    const colour = lerp(ramp[0], ramp[1], width === 1 ? 1 : i / (width - 1))

    // In palette colours, the cell where the share ends is the bar's or the track's, never between;
    // any share left keeps its first cell, so a nearly spent budget still shows red.
    const part = paint.isPalette === true ? (i === 0 && filled > 0 ? 1 : Math.round(clamp01(filled - i))) : clamp01(filled - i)

    return [0x2580, hex(lerp(paint.track, colour, part)), DEFAULT_BG]
  }

  return encode(Array.from({ length: width }, (_, i) => at(i)))
}

// One frame of the spinner: the theme's `glyphs` in turn, pulsing between its two colours.
export function spinnerCell(frame: number, colours: Ramp, glyphs: readonly string[], bg: number | null = null): string {
  const glyph = glyphs[frame % glyphs.length]!
  const pulse = (Math.sin(frame / 3) + 1) / 2

  return encode([cell(glyph, lerp(colours[0], colours[1], pulse), bg)])
}

// Text fallback for surfaces without Raster: the same bar as coloured runs.
export function barRuns(checks: Checks, width: number): { text: string; colour: 'success' | 'error' | 'warning' }[] {
  const [passed = 0, failed = 0, pending = 0] = allot([checks.passed, checks.failed, checks.pending], width)

  return [
    { text: '━'.repeat(passed), colour: 'success' as const },
    { text: '━'.repeat(failed), colour: 'error' as const },
    { text: '━'.repeat(pending), colour: 'warning' as const },
  ].filter(run => run.text.length > 0)
}
