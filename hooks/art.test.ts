import { describe, expect, test } from 'claude-code/testing'

import type { Checks, Tone, TrackedPr } from '../types'
import { ACCENT_CONTRAST, MIN_FILL, REVIEW_FILL, artCard, artCells, artGrid, bannerLines, contrast, edgeAt, fillOf, inkOn, layoutLine, narrowed } from './art'
import type { ArtCard, Glyph, Role } from './art'
import { MODES, TONES, lookOf } from './theme'
import type { Rgb } from './theme'
import { PILL, placeholder } from './pr'

const checks = (passed: number, failed: number, pending: number, failing: string[] = []): Checks => ({
  passed,
  failed,
  pending,
  total: passed + failed + pending,
  failing,
  running: [],
  queued: 0,
})
const RUNNING: TrackedPr = {
  ...placeholder({ url: 'https://github.com/acme/app/pull/12', repo: 'acme/app', number: 12 }),
  title: 'Retry uploads with exponential backoff',
  state: 'OPEN',
  stage: 'checks',
  tone: 'warning',
  pill: PILL.ciRunning,
  headline: 'Running build · 2/5 passed',
  isMoving: true,
  checks: checks(2, 0, 3),
  branch: 'feat/retry',
  base: 'main',
  additions: 120,
  deletions: 14,
  author: 'octocat',
  checkedAt: 0,
}
const rgb = (n: number): Rgb => [(n >> 16) & 255, (n >> 8) & 255, n & 255]
const text = (row: readonly Glyph[]) => row.map(g => String.fromCodePoint(g.cp)).join('')
const decoded = (cells: string) => [...new Uint32Array(Uint8Array.from(atob(cells), c => c.charCodeAt(0)).buffer)]

describe('how far the art has come', () => {
  test("CI's finished checks of all of them while they run; nearly full in review; full when ready or merged", async () => {
    expect(fillOf(RUNNING)).toBe(0.4)
    expect(fillOf({ ...RUNNING, checks: checks(1, 2, 1) })).toBe(0.75)
    // Nothing finished yet: a sliver, so the liquid still shows it moving.
    expect(fillOf({ ...RUNNING, checks: checks(0, 0, 4) })).toBe(MIN_FILL)
    expect(fillOf({ ...RUNNING, checks: checks(4, 1, 0) })).toBe(1)
    expect(fillOf({ ...RUNNING, stage: 'review' })).toBe(REVIEW_FILL)
    expect(fillOf({ ...RUNNING, stage: 'ready', checks: checks(5, 0, 0) })).toBe(1)
    expect(fillOf({ ...RUNNING, state: 'MERGED', stage: 'merged' })).toBe(1)
    expect(fillOf({ ...RUNNING, state: 'CLOSED', stage: 'closed' })).toBe(0)
    // A draft is half way at most: it isn't heading for a merge yet.
    expect(fillOf({ ...RUNNING, stage: 'draft', checks: checks(4, 0, 0) })).toBe(0.5)
    expect(fillOf(placeholder({ url: RUNNING.url, repo: 'acme/app', number: 12 }))).toBe(MIN_FILL)
  })

  test('the front sways inside the card, and never leaves it nor drains from it', async () => {
    for (const width of [24, 50, 120]) {
      for (let fill = 0; fill <= 1; fill += 0.05) {
        for (let row = 0; row < 8; row++) {
          for (let phase = 0; phase < 12; phase += 0.7) {
            const edge = edgeAt(fill, width, row, phase)
            expect(edge).toBeGreaterThanOrEqual(0)
            expect(edge).toBeLessThanOrEqual(width)
            if (fill > 0.001 && fill < 0.999) {
              expect(edge).toBeGreaterThan(0)
              expect(edge).toBeLessThan(width)
            }
          }
        }
      }
    }
    expect(edgeAt(0, 50, 3, 1)).toBe(0)
    expect(edgeAt(1, 50, 3, 1)).toBe(50)
    // The front is a wave: its rows stand at different places.
    expect(new Set([0, 1, 2, 3].map(row => edgeAt(0.4, 50, row, 0).toFixed(2))).size).toBeGreaterThan(1)
  })
})

describe('the cells', () => {
  test('every cell is one width-1 character: emoji and wide characters stand in, zero-width marks go', async () => {
    expect(narrowed('fix: ✨ crash')).toEqual([...'fix: ? crash'])
    expect(narrowed('💬 COMMENTS')).toEqual([...'❝ COMMENTS'])
    expect(narrowed('🚀 ship')).toEqual([...'• ship'])
    expect(narrowed('日本')).toEqual(['?', '?'])
    expect(narrowed('⚠\u{FE0F} café́')).toEqual([...'⚠ café'])
    expect(narrowed('a\tb')).toEqual([...'a b'])
    expect(narrowed('✓ ✗ ● ◌ ⏸ ↺ ◷ ⇢ ✕ ✎ → −')).toEqual([...'✓ ✗ ● ◌ ⏸ ↺ ◷ ⇢ ✕ ✎ → −'])
  })

  test('a row: padding each side, the badge against the right edge, the left cut with … before it', async () => {
    const line = { left: [{ text: '#12 Retry uploads with exponential backoff', role: 'strong' as Role }], right: [{ text: ' ● CI RUNNING ', role: 'badge' as Role }] }
    const row = layoutLine(line, 40)
    expect(row).toHaveLength(40)
    expect(text(row)).toBe(' #12 Retry uploads with…  ● CI RUNNING  ')
    expect(row.at(-2)?.role).toBe('badge')
    // Too narrow for both: the badge goes, the left keeps the row.
    expect(text(layoutLine(line, 18))).toBe(' #12 Retry uploa… ')
    // A spare right side gives way where the left won't fit beside it.
    expect(text(layoutLine({ left: [{ text: 'acme/app · feat/retry → main', role: 'dim' }], right: [{ text: '@octocat', role: 'dim' }], isRightSpare: true }, 34))).toBe(
      ' acme/app · feat/retry → main     ',
    )
  })

  test("a banner card's rows: headline and badge, where it goes, what it does, then room below", async () => {
    const rows = bannerLines(RUNNING, 0, 50).map(line => text(layoutLine(line, 50)))
    expect(rows).toEqual([
      ' #12 Retry uploads with exponenti…  ● CI RUNNING  ',
      ' acme/app · feat/retry → main  +120 −14  @octocat ',
      ' ◌ Running build · 2/5 passed                     ',
      '                                                  ',
    ])
    // A failed CI: its tally at the right, the failing past the first two on rows of their own.
    const failed = { ...RUNNING, tone: 'error' as Tone, pill: PILL.ciFailed, headline: 'Failing: a, b +1', isMoving: false, checks: checks(1, 3, 0, ['a', 'b', 'c']) }
    expect(bannerLines(failed, 0, 50).map(line => text(layoutLine(line, 50)).trimEnd())).toEqual([
      ' #12 Retry uploads with exponentia…  ✗ CI FAILED',
      ' acme/app · feat/retry → main  +120 −14  @octocat',
      ' Failing: a, b +1                             1/4',
      '   ✗ c',
      '',
    ])
  })
})

// Every card paint: each banner theme, mode and tone, at a fill.
function cards(fill: number, isMoving = true): ArtCard[] {
  const roles: Role[] = ['text', 'strong', 'dim', 'accent', 'branch', 'add', 'del', 'ok', 'bad', 'warn', 'note', 'blank', 'badge']
  const grid = Array.from({ length: 5 }, (_, y) => Array.from({ length: 44 }, (_, x): Glyph => ({ cp: 0x41, role: roles[(x + y) % roles.length]! })))

  return (['tide', 'aurora'] as const).flatMap(name =>
    MODES.flatMap(mode =>
      TONES.map((tone): ArtCard => {
        const look = lookOf(name, mode)

        return { kind: name, grid, fill, isMoving, seed: 12, tone, paint: look.variant.art!, spin: look.theme.spin }
      }),
    ),
  )
}

describe('the paint', () => {
  test('a frame is the same every time it is painted; a moving card changes between frames, a still one never', async () => {
    for (const card of cards(0.4)) {
      expect(artCells(card, 7)).toBe(artCells(card, 7))
      expect(artCells(card, 7)).not.toBe(artCells(card, 13))
      const still = { ...card, isMoving: false }
      expect(artCells(still, 7)).toBe(artCells(still, 13))
    }
  })

  test('the liquid fills only as far as the PR has come: none when empty, all of it when full', async () => {
    const tide = lookOf('tide', 'dark').variant.art!
    const glass = (tone: Tone) => tide.tones[tone].glass
    const card = (fill: number): ArtCard => ({ kind: 'tide', fill, isMoving: true, seed: 12, tone: 'success', paint: tide, spin: ['x'], grid: [Array.from({ length: 40 }, () => ({ cp: 0x20, role: 'blank' as Role }))] })
    const bgs = (fill: number) => artGrid(card(fill), 3)[0]!.map(([, , bg]) => bg)
    // Empty: the glass, but for a faint pick-up of colour at its left edge.
    const empty = bgs(0)
    expect(empty.slice(3).every(bg => bg === empty[3])).toBe(true)
    expect(contrast(rgb(empty[10]!), glass('success'))).toBeLessThan(1.05)
    // Full: liquid from edge to edge, nothing of the glass.
    expect(bgs(1).every(bg => contrast(rgb(bg), glass('success')) > 1.3)).toBe(true)
    // 40%: liquid on the left, glass on the right, the front within the sway of 16 cells.
    const part = bgs(0.4)
    const liquid = part.map(bg => contrast(rgb(bg), glass('success')) > 1.3)
    const front = liquid.indexOf(false)
    expect(front).toBeGreaterThanOrEqual(12)
    expect(front).toBeLessThanOrEqual(20)
    expect(liquid.slice(front + 1).every(isLiquid => !isLiquid)).toBe(true)
  })

  test('every text cell takes the text colour that reads on its own background; accents only where they read', async () => {
    for (const fill of [0, 0.3, 0.85, 1]) {
      for (const card of cards(fill)) {
        for (const frame of [0, 5, 11]) {
          artGrid(card, frame).forEach((row, y) =>
            row.forEach(([, fg, bg], x) => {
              const role = card.grid[y]![x]!.role
              if (role === 'blank') return
              const ratio = contrast(rgb(fg), rgb(bg))
              if (role === 'text' || role === 'strong' || role === 'badge') {
                expect(ratio).toBeGreaterThanOrEqual(4.4)
                // The better of the two.
                expect(rgb(fg)).toEqual(inkOn(rgb(bg), card.paint))
              } else if (role === 'dim') expect(ratio).toBeGreaterThanOrEqual(2.5)
              else expect(ratio).toBeGreaterThanOrEqual(ACCENT_CONTRAST)
            }),
          )
        }
      }
    }
  })

  test('the spinner turns on a moving card, from the theme', async () => {
    const look = lookOf('tide', 'dark')
    const card = artCard('tide', RUNNING, 0, 50, look.variant.art!, look.theme.spin)
    const spinner = (frame: number) => String.fromCodePoint(decoded(artCells(card, frame))[(2 * 50 + 1) * 3]!)
    expect(look.theme.spin).toContain(spinner(0))
    expect(spinner(0)).not.toBe(spinner(1))
    expect(String.fromCodePoint(decoded(artCells({ ...card, isMoving: false }, 1))[(2 * 50 + 1) * 3]!)).toBe('◌')
  })

  test('a card lays out to its width, a row a line, and every cell decodes', async () => {
    const look = lookOf('aurora', 'light')
    const card = artCard('aurora', { ...RUNNING, title: 'fix: ✨ handle 💬 replies' }, 0, 30, look.variant.art!, look.theme.spin)
    expect(card.grid.every(row => row.length === 30)).toBe(true)
    const words = decoded(artCells(card, 0))
    expect(words).toHaveLength(card.grid.length * 30 * 3)
    for (let i = 0; i < words.length; i += 3) expect(words[i]!).toBeLessThan(0x10000)
  })
})
