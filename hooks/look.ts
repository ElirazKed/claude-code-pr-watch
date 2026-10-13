// The pane's true-colour paint: gradient bars, a shimmer over pending checks and a
// hue-cycling spinner, all packed as Raster cells so the animation can blit frames. The
// colours, glyphs and the background behind the cells are the theme's (theme.ts).
import type { Checks } from '../types'
import type { Ramp, RasterPaint, Rgb } from './theme'

type Cell = readonly [codePoint: number, fg: number, bg: number]

const DEFAULT_BG = 0x01000000

// What a CI bar is painted with: the theme's ramps, and the card's fill behind (null: the
// terminal's own background).
export type BarPaint = Pick<RasterPaint, 'green' | 'red' | 'amber' | 'shine'> & { bg: number | null }

function lerp(a: Rgb, b: Rgb, t: number): Rgb {
  const k = Math.min(1, Math.max(0, t))

  return [0, 1, 2].map(i => Math.round(a[i]! + (b[i]! - a[i]!) * k)) as unknown as Rgb
}

function hex([r, g, b]: Rgb): number {
  return (r << 16) | (g << 8) | b
}

function gradientAt(stops: readonly Rgb[], t: number): Rgb {
  const span = (stops.length - 1) * Math.min(1, Math.max(0, t))
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

// The header's rule: `glyph` across, through the theme's gradient stops.
export function ruleCells(width: number, stops: readonly Rgb[], glyph: string): string {
  return encode(Array.from({ length: width }, (_, i) => cell(glyph, gradientAt(stops, i / Math.max(1, width - 1)))))
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
