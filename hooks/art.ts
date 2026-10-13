// The banner cards (a theme's `layout: 'banner'`): each card one Raster, its text drawn into
// the cells over painted art, so the art can sit behind the words. Tide: a liquid that fills the
// card from the left as the PR moves on, deeper toward the bottom, behind a wavy front that
// sways while the PR moves and is still when it doesn't. Aurora: curtains of light in the
// state's colours, brighter as the PR moves on. Every text cell takes whichever of the theme's
// two text colours reads better on that cell's background, so words read over both parts.
// Pure: the render lays a card out once, and the animation repaints it per frame.
import type { Tone, TrackedPr } from '../types'
import type { ArtKind, ArtPaint, Rgb } from './theme'
import { clamp01, encode, gradientAt, hex, lerp } from './look'
import type { Cell } from './look'
import { ago } from './pr'
import { threadsBeside, verdictLine } from './review'

// What a run of text is, so the painter colours it: the theme's text colours (`text`, `strong`,
// `dim`), a tone's accent, the state badge's chip, the spinner, or no text at all.
export type Role = 'text' | 'strong' | 'dim' | 'accent' | 'branch' | 'add' | 'del' | 'ok' | 'bad' | 'warn' | 'note' | 'badge' | 'spin' | 'blank'
export type Seg = { text: string; role: Role }
// A row of the card: runs from the left, and runs ending at the right edge (the badge).
// `isRightSpare`: where the left runs won't fit beside them, the right runs give way.
export type Line = { left: Seg[]; right?: Seg[]; isRightSpare?: boolean }
export type Glyph = { cp: number; role: Role }

// ---- How far the art has come --------------------------------------------------------------

// A sliver, so a card that has only just started still shows its liquid moving.
export const MIN_FILL = 0.04
// CI done, a review to come: nearly full.
export const REVIEW_FILL = 0.85

// How much of the card the art fills, 0 to 1: CI's finished checks of all of them while they
// run (a draft, half of that: it isn't heading for a merge yet), nearly full in review, full once
// it is ready or merged; nothing for a closed PR, a sliver while it loads.
export function fillOf(pr: Pick<TrackedPr, 'state' | 'stage' | 'checks'>): number {
  if (pr.state === 'MERGED') return 1
  if (pr.state === 'CLOSED') return 0
  const { passed, failed, total } = pr.checks
  const ci = total > 0 ? Math.min(1, (passed + failed) / total) : 0
  switch (pr.stage) {
    case 'merged':
    case 'ready':
      return 1
    case 'review':
      return REVIEW_FILL
    case 'checks':
      return Math.max(MIN_FILL, ci)
    case 'draft':
      return Math.max(MIN_FILL, ci / 2)
    case 'closed':
      return 0
    case 'unknown':
      return MIN_FILL
  }
}

// Where tide's front stands on `row`, in cells from the left: the fill's share of the width,
// swayed by two sines (`phase` moves them), the sway shrinking near either edge so the liquid
// never leaves the card nor drains from it. Empty is 0 and full is `width`, unswayed.
export function edgeAt(fill: number, width: number, row: number, phase: number): number {
  const f = clamp01(fill)
  if (f === 0) return 0
  if (f === 1) return width
  const base = f * width
  const amp = Math.min(3.5, 0.8 * Math.min(base, width - base))
  const wave = Math.sin(row * 1.1 + phase) * 0.65 + Math.sin(row * 2.3 - phase * 1.7) * 0.35

  return Math.min(width, Math.max(0, base + amp * wave))
}

// ---- The cells' text ----------------------------------------------------------------------

// A Raster's cell holds one printable width-1 character of the Basic Multilingual Plane. What
// a terminal draws one cell wide, by range; zero-width marks are dropped; anything else (an
// emoji, CJK, which take two) stands as one cell that says something was there.
const NARROW: readonly (readonly [number, number])[] = [
  [0x20, 0x7e],
  [0xa0, 0x2ff],
  [0x370, 0x482],
  [0x48a, 0x52f],
  [0x531, 0x58a],
  [0x5d0, 0x5f4],
  [0x620, 0x64a],
  [0x660, 0x66f],
  [0x671, 0x6d3],
  [0x10a0, 0x10ff],
  [0x1e00, 0x1fff],
  [0x2010, 0x2027],
  [0x2030, 0x205e],
  [0x2070, 0x209c],
  [0x20a0, 0x20c0],
  [0x2100, 0x218b],
  [0x2190, 0x2426],
  [0x2440, 0x244a],
  [0x2500, 0x2b73],
]
// Inside those, the ones drawn as an emoji (two cells).
const WIDE: readonly (readonly [number, number])[] = [
  [0x231a, 0x231b],
  [0x2329, 0x232a],
  [0x23e9, 0x23ec],
  [0x23f0, 0x23f0],
  [0x23f3, 0x23f3],
  [0x25fd, 0x25fe],
  [0x2614, 0x2615],
  [0x2648, 0x2653],
  [0x267f, 0x267f],
  [0x2693, 0x2693],
  [0x26a1, 0x26a1],
  [0x26aa, 0x26ab],
  [0x26bd, 0x26be],
  [0x26c4, 0x26c5],
  [0x26ce, 0x26ce],
  [0x26d4, 0x26d4],
  [0x26ea, 0x26ea],
  [0x26f2, 0x26f3],
  [0x26f5, 0x26f5],
  [0x26fa, 0x26fa],
  [0x26fd, 0x26fd],
  [0x2705, 0x2705],
  [0x270a, 0x270b],
  [0x2728, 0x2728],
  [0x274c, 0x274c],
  [0x274e, 0x274e],
  [0x2753, 0x2755],
  [0x2757, 0x2757],
  [0x2795, 0x2797],
  [0x27b0, 0x27b0],
  [0x27bf, 0x27bf],
  [0x2b1b, 0x2b1c],
  [0x2b50, 0x2b50],
  [0x2b55, 0x2b55],
]
const ZERO: readonly (readonly [number, number])[] = [
  [0xad, 0xad],
  [0x300, 0x36f],
  [0x483, 0x489],
  [0x591, 0x5c7],
  [0x610, 0x61a],
  [0x64b, 0x65f],
  [0x670, 0x670],
  [0x200b, 0x200f],
  [0x2028, 0x202e],
  [0x2060, 0x2064],
  [0x20d0, 0x20ff],
  [0xfe00, 0xfe0f],
  [0xfeff, 0xfeff],
]
// The card's own two-cell glyphs, drawn as one-cell ones that say the same.
const STAND_IN: Record<string, string> = { '💬': '❝', '🎉': '✦' }

const within = (ranges: readonly (readonly [number, number])[], cp: number) => ranges.some(([from, to]) => cp >= from && cp <= to)

// `text` as Raster cells: one character a cell.
export function narrowed(text: string): string[] {
  return [...text].flatMap(ch => {
    const cp = ch.codePointAt(0) ?? 0x3f
    if (STAND_IN[ch] !== undefined) return [STAND_IN[ch]]
    if (cp < 0x20 || (cp >= 0x7f && cp < 0xa0)) return [' ']
    if (within(ZERO, cp)) return []
    if (within(NARROW, cp) && !within(WIDE, cp)) return [ch]

    return [cp > 0xffff ? '•' : '?']
  })
}

const glyphsOf = (segs: readonly Seg[]): Glyph[] => segs.flatMap(seg => narrowed(seg.text).map(ch => ({ cp: ch.codePointAt(0) ?? 0x3f, role: seg.role })))

// Left runs room the badge won't take from.
const MIN_LEFT = 4

// A row `width` cells wide: `pad` blank cells each side, the right runs against the right
// edge, the left runs cut with "…" where they would reach them. Too narrow for both, the right
// runs go (the status row says the state too).
export function layoutLine(line: Line, width: number, pad = 1): Glyph[] {
  const row: Glyph[] = Array.from({ length: width }, () => ({ cp: 0x20, role: 'blank' }))
  const room = Math.max(0, width - 2 * pad)
  let right = glyphsOf(line.right ?? [])
  let left = glyphsOf(line.left)
  if (right.length > 0 && (room - right.length - 1 < MIN_LEFT || (line.isRightSpare === true && left.length + right.length + 1 > room))) right = []
  const leftRoom = Math.max(0, room - (right.length > 0 ? right.length + 1 : 0))
  if (left.length > leftRoom) {
    // Cut at a word's end where the cut falls on a space: "uploads…", never "uploads …".
    const kept = left.slice(0, Math.max(0, leftRoom - 1))
    while (kept.length > 0 && kept.at(-1)!.cp === 0x20) kept.pop()
    // The "…" in the cut text's colour; never a blank's, which draws no glyph.
    const role = kept.at(-1)?.role ?? left[0]!.role
    left = leftRoom === 0 ? [] : [...kept, { cp: 0x2026, role: role === 'blank' ? 'dim' : role }]
  }
  left.forEach((glyph, i) => (row[pad + i] = glyph))
  right.forEach((glyph, i) => (row[width - pad - right.length + i] = glyph))

  return row
}

const VERDICT_ROLE = { approved: 'ok', changes: 'bad', commented: 'note', requested: 'dim' } as const

// What a banner card says, a Line a row: the number, title and state badge; where it comes
// from and goes (repo · branch → base · +/−); what it is doing now; then, as the stacked card
// has them, failing checks past the first two, the reviewers, open threads and an error. Last,
// an empty row, so the art has room below the words. Links and buttons are not cells: the
// render draws them on rows of their own.
export function bannerLines(pr: TrackedPr, at: number, columns: number): Line[] {
  const isOpen = pr.state === 'OPEN'
  const isMerged = pr.stage === 'merged'
  const meta: Seg[] = [{ text: pr.repo, role: 'dim' }]
  if (pr.branch !== '') {
    meta.push(
      { text: ' · ', role: 'dim' },
      { text: pr.branch, role: 'branch' },
      { text: ' → ', role: 'dim' },
      { text: pr.base, role: 'dim' },
      { text: '  ', role: 'blank' },
      { text: `+${pr.additions}`, role: 'add' },
      { text: ' ', role: 'blank' },
      { text: `−${pr.deletions}`, role: 'del' },
    )
  }
  const threads = isOpen && pr.reviews !== null ? threadsBeside(pr.headline, pr.reviews) : { line: '', note: '' }
  const status: Seg[] = [...(pr.isMoving ? [{ text: '◌', role: 'spin' as const }, { text: ' ', role: 'blank' as const }] : []), { text: pr.headline, role: 'accent' }]
  if (isMerged && pr.mergedAt !== null) status.push({ text: ` · ${ago(at - Date.parse(pr.mergedAt))}`, role: 'dim' })
  if (threads.note !== '') status.push({ text: ` · ${threads.note}`, role: 'dim' })
  // "4/5": CI's tally, unless the headline says it already.
  const tally = `${pr.checks.passed}/${pr.checks.total}`
  const lines: Line[] = [
    { left: [{ text: `#${pr.number}`, role: 'accent' }, { text: ' ', role: 'blank' }, { text: pr.title || 'Loading…', role: 'strong' }], right: [{ text: ` ${pr.pill} `, role: 'badge' }] },
    { left: meta, right: pr.author !== '' ? [{ text: `@${pr.author}`, role: 'dim' }] : [], isRightSpare: true },
    { left: status, right: isOpen && pr.checks.total > 0 && !pr.headline.includes(tally) ? [{ text: tally, role: 'dim' }] : [], isRightSpare: true },
  ]
  for (const name of pr.checks.failing.slice(2, 5)) lines.push({ left: [{ text: `  ✗ ${name}`, role: 'bad' }] })
  if (isOpen && pr.reviews !== null) {
    const pieces = verdictLine(pr.reviews, columns - 2)
    if (pieces.length > 0) {
      lines.push({
        left: pieces.flatMap((piece, i) => [
          ...(i > 0 ? [{ text: ' · ', role: 'dim' as const }] : []),
          { text: piece.text, role: piece.verdict === null ? ('dim' as const) : VERDICT_ROLE[piece.verdict] },
        ]),
      })
    }
    if (threads.line !== '') lines.push({ left: [{ text: threads.line, role: 'warn' }] })
  }
  if (pr.error !== null) lines.push({ left: [{ text: `⚠ ${pr.error}`, role: 'bad' }] })
  lines.push({ left: [] })

  return lines
}

// ---- The paint ----------------------------------------------------------------------------

// A laid-out card, all the animation needs to paint a frame of it.
export type ArtCard = {
  kind: ArtKind
  grid: Glyph[][]
  fill: number
  isMoving: boolean
  // Sets each card's wave apart (the PR's number).
  seed: number
  tone: Tone
  paint: ArtPaint
  spin: readonly string[]
}

export function artCard(kind: ArtKind, pr: TrackedPr, at: number, columns: number, paint: ArtPaint, spin: readonly string[]): ArtCard {
  return {
    kind,
    grid: bannerLines(pr, at, columns).map(line => layoutLine(line, columns)),
    fill: fillOf(pr),
    isMoving: pr.isMoving,
    seed: pr.number,
    tone: pr.tone,
    paint,
    spin,
  }
}

// WCAG's relative luminance and contrast ratio.
export function luminance([r, g, b]: Rgb): number {
  const lin = (c: number) => {
    const s = c / 255

    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4
  }

  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

export function contrast(a: Rgb, b: Rgb): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number]

  return (hi + 0.05) / (lo + 0.05)
}

// Of the theme's light and dark text, the one that reads better on `bg`.
export const inkOn = (bg: Rgb, paint: Pick<ArtPaint, 'light' | 'dark'>): Rgb => (contrast(paint.light, bg) >= contrast(paint.dark, bg) ? paint.light : paint.dark)

// An accent is kept only where it reads this well on its cell; else the plain text colour.
export const ACCENT_CONTRAST = 3

const ROLE_TONE: Partial<Record<Role, Tone>> = { branch: 'suggestion', note: 'suggestion', add: 'success', ok: 'success', del: 'error', bad: 'error', warn: 'warning' }

// The tone a role's accent comes from (`accent` and the spinner: the card's own); none for the
// plain text colours.
export const roleTone = (role: Role, tone: Tone): Tone | null => (role === 'accent' || role === 'spin' ? tone : (ROLE_TONE[role] ?? null))

// A text cell's colour on `bg`.
export function fgOf(role: Role, bg: Rgb, card: Pick<ArtCard, 'tone' | 'paint'>): Rgb {
  const base = inkOn(bg, card.paint)
  if (role === 'dim') return lerp(base, bg, 0.4)
  const tone = roleTone(role, card.tone)
  if (tone === null) return base
  const ink = card.paint.tones[tone].ink

  return contrast(ink, bg) >= ACCENT_CONTRAST ? ink : base
}

// Left partial blocks, by eighths filled: where the front crosses an empty cell, the liquid ends
// inside it.
const EIGHTHS = [...' ▏▎▍▌▋▊▉█']

// Tide's background at a cell, and, where the front ends inside a cell, how much of it is liquid
// (null: none or all) and the liquid's colour there.
function tideAt(card: ArtCard, x: number, y: number, frame: number, rows: number, width: number): { bg: Rgb; cover: number | null; liquid: Rgb } {
  const { glass, surface, deep, crest } = card.paint.tones[card.tone]
  const phase = card.seed * 0.7 + (card.isMoving ? frame * 0.22 : 0)
  const edge = edgeAt(card.fill, width, y, phase)
  const depth = clamp01(0.55 * (rows <= 1 ? 1 : y / (rows - 1)) + 0.45 * (1 - (x + 0.5) / Math.max(1, edge)))
  let liquid = lerp(surface, deep, depth)
  // The crest catches the light; full, a glint sweeps across instead while the PR moves.
  if (edge < width) liquid = lerp(liquid, crest, Math.max(0, 1 - (edge - (x + 0.5)) / 2.5) * 0.3)
  else if (card.isMoving) {
    const at = ((frame * 0.5 + card.seed * 3) % (width + 16)) - 8
    liquid = lerp(liquid, crest, Math.max(0, 1 - Math.abs(x - at + y * 1.5) / 3) * 0.22)
  }
  const cover = clamp01(edge - x)
  if (cover >= 1) return { bg: liquid, cover: null, liquid }
  // Ahead of the front, the glass picks up a little of the water.
  const ahead = lerp(glass, surface, Math.max(0, 1 - (x - edge) / 3) * 0.12)
  if (cover <= 0) return { bg: ahead, cover: null, liquid }

  return { bg: lerp(ahead, liquid, cover), cover, liquid }
}

// Aurora's background at a cell: curtains rippling across the sky, through the state's colours
// from left to right, brighter toward the top and as far as the PR has come, drifting while it
// moves. Kept below 60% of the colour, so the text colours read on it.
function auroraAt(card: ArtCard, x: number, y: number, frame: number, rows: number, width: number): Rgb {
  const { glass, surface, deep, crest } = card.paint.tones[card.tone]
  const t = card.isMoving ? frame * 0.06 : 0
  // Two layers of curtains, tall and slow: they bend a little from row to row, never break up.
  const sway = Math.sin(y * 0.45 + x * 0.05 + t + card.seed) * 0.9
  const curtain = 0.6 * (0.5 + 0.5 * Math.sin(x * 0.21 + sway + t * 0.7)) + 0.4 * (0.5 + 0.5 * Math.sin(x * 0.53 - sway * 0.6 - t * 0.4 + card.seed))
  const hue = clamp01(x / Math.max(1, width - 1) + 0.15 * Math.sin(y * 0.9 + t * 0.5 + card.seed))
  const edge = card.fill * width
  const reach = x <= edge ? 1 : Math.max(0.25, 1 - (x - edge) / 8)
  const light = (0.18 + 0.42 * curtain) * reach * (1 - 0.25 * (rows <= 1 ? 0 : y / (rows - 1)))

  return lerp(glass, gradientAt([deep, crest, surface], hue), light)
}

// One frame of a card, a row of cells per row: the art behind, the text over it, the spinner
// on the frame's glyph.
export function artGrid(card: ArtCard, frame: number): Cell[][] {
  const rows = card.grid.length
  const width = card.grid[0]?.length ?? 0
  const badge = card.paint.tones[card.tone].badge
  const spun = card.spin[(frame + card.seed) % card.spin.length]?.codePointAt(0)
  const cpOf = (glyph: Glyph) => (glyph.role === 'spin' && card.isMoving && spun !== undefined ? spun : glyph.cp)

  return card.grid.map((row, y) =>
    row.map((glyph, x): Cell => {
      if (glyph.role === 'badge') return [glyph.cp, hex(inkOn(badge, card.paint)), hex(badge)]
      if (card.kind === 'aurora') {
        const bg = auroraAt(card, x, y, frame, rows, width)

        return [cpOf(glyph), hex(glyph.role === 'blank' ? bg : fgOf(glyph.role, bg, card)), hex(bg)]
      }
      const { bg, cover, liquid } = tideAt(card, x, y, frame, rows, width)
      if (glyph.role === 'blank') {
        const eighths = cover === null ? 0 : Math.round(cover * 8)
        // The front inside an empty cell: a partial block of liquid over the glass beyond it.
        if (eighths > 0 && eighths < 8) return [EIGHTHS[eighths]!.codePointAt(0)!, hex(liquid), hex(tideAt(card, x + 1, y, frame, rows, width).bg)]

        return [0x20, hex(bg), hex(bg)]
      }

      return [cpOf(glyph), hex(fgOf(glyph.role, bg, card)), hex(bg)]
    }),
  )
}

// A frame's cells, encoded for the Raster (RasterProps) and its blits.
export const artCells = (card: ArtCard, frame: number): string => encode(artGrid(card, frame).flat())
