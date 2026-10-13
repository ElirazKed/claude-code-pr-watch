import { describe, expect, test } from 'claude-code/testing'

import { barCells, fullBarCells, ruleCells, spinnerCell } from './look'
import {
  MODES,
  THEME,
  THEMES,
  TONES,
  barPaint,
  cardBox,
  inkOf,
  lookOf,
  modeOf,
  modeOfColours,
  optionTheme,
  pillOf,
  rasterBg,
  themeListing,
  themeOf,
  unknownTheme,
} from './theme'

// The theme keys Claude Code paints with.
const KEYS = new Set(['text', 'inverseText', 'inactive', 'subtle', 'suggestion', 'success', 'error', 'warning', 'merged'])
// What both a tree takes and the terminal paints: a theme key, hex, or one of the palette's 16
// by number. (A tree refuses `ansi:red`, and the terminal paints nothing for a bare `red`.)
const paints = (colour: unknown) =>
  typeof colour === 'string' && (KEYS.has(colour) || /^#[0-9a-f]{6}$/i.test(colour) || /^ansi256\(([0-9]|1[0-5])\)$/.test(colour))
const isRgb = (rgb: readonly number[]) => rgb.length === 3 && rgb.every(n => Number.isInteger(n) && n >= 0 && n <= 255)

describe('the themes', () => {
  test('every theme paints every tone, in light and in dark', async () => {
    for (const name of THEMES) {
      expect(THEME[name].name).toBe(name)
      for (const mode of MODES) {
        const variant = THEME[name][mode]
        expect(Object.keys(variant.tones).sort()).toEqual([...TONES].sort())
        for (const tone of TONES) {
          const paint = variant.tones[tone]
          expect(paints(paint.ink)).toBe(true)
          for (const colour of [paint.border, paint.fill, paint.glow]) if (colour !== null) expect(paints(colour)).toBe(true)
          // A frame with a border has a colour for it, and a frame without one none.
          expect(paint.border !== null).toBe(THEME[name].card.border !== null)
        }
        expect(paints(variant.pillText)).toBe(true)
        const { green, red, amber, mauve, shine, rule, spinner } = variant.raster
        for (const rgb of [...green, ...red, ...amber, ...mauve, shine, ...rule, ...spinner]) expect(isRgb(rgb)).toBe(true)
        expect(rule.length).toBeGreaterThanOrEqual(2)
      }
      expect(THEME[name].spin.length).toBeGreaterThan(0)
      expect([...THEME[name].rule]).toHaveLength(1)
    }
  })

  test('a theme that paints raw hex has a light and a dark of it', async () => {
    for (const name of ['solid', 'neon'] as const) {
      expect(THEME[name].light.tones).not.toEqual(THEME[name].dark.tones)
    }
  })

  test('a name in any case finds its theme; any other is none', async () => {
    expect(themeOf('Neon')).toBe('neon')
    expect(themeOf('  RETRO ')).toBe('retro')
    for (const name of THEMES) expect(themeOf(name.toUpperCase())).toBe(name)
    expect(themeOf('sparkly')).toBeNull()
    expect(themeOf('')).toBeNull()
    expect(themeOf(3)).toBeNull()
    expect(optionTheme('Solid')).toBe('solid')
    expect(optionTheme('sparkly')).toBe('classic')
    expect(optionTheme(undefined)).toBe('classic')
    expect(unknownTheme('sparkly')).toBe('No theme "sparkly". Themes: classic, solid, minimal, neon, retro.')
  })

  test('the listing names every theme and marks the current one', async () => {
    const lines = themeListing('neon').split('\n')
    expect(lines).toHaveLength(THEMES.length + 2)
    expect(lines.filter(line => line.startsWith('●'))).toEqual([expect.stringMatching(/^● neon +bold borders.*\(current\)$/)])
    for (const name of THEMES) expect(lines.some(line => line.slice(2).startsWith(name))).toBe(true)
  })
})

describe('light or dark', () => {
  test("Claude Code's theme row says it, but for auto and a custom theme", async () => {
    expect(modeOf('light')).toBe('light')
    expect(modeOf('light-daltonized')).toBe('light')
    expect(modeOf('light-ansi')).toBe('light')
    expect(modeOf('dark')).toBe('dark')
    expect(modeOf('dark-ansi')).toBe('dark')
    expect(modeOf('auto')).toBeNull()
    expect(modeOf('custom:paper')).toBeNull()
    expect(modeOf(undefined)).toBeNull()
  })

  test("the terminal's COLORFGBG, by its background", async () => {
    expect(modeOfColours('15;0')).toBe('dark')
    expect(modeOfColours('0;15')).toBe('light')
    expect(modeOfColours('0;default;7')).toBe('light')
    expect(modeOfColours('7;8')).toBe('dark')
    expect(modeOfColours('/home/me')).toBeNull()
    expect(modeOfColours(undefined)).toBeNull()
  })
})

describe('the classic look', () => {
  test("is today's card: a round border in the tone, dim but under the pointer, a filled pill", async () => {
    for (const mode of MODES) {
      const look = lookOf('classic', mode)
      expect(cardBox(look, 'error')).toEqual({
        borderStyle: 'round',
        borderColor: 'error',
        borderDimColor: true,
        hover: { borderDimColor: false },
        paddingX: 1,
        marginBottom: 1,
      })
      expect(pillOf(look, 'success', '✓ READY')).toEqual({
        open: ' ',
        close: ' ',
        text: '✓ READY',
        props: { backgroundColor: 'success', color: 'inverseText', bold: true },
      })
      for (const tone of TONES) expect(inkOf(look, tone, 'number')).toBe(tone)
      expect(rasterBg(look, 'success')).toBeNull()
    }
  })

  // As look.ts drew them before themes, with its palette then.
  test("paints today's Raster cells", async () => {
    const look = lookOf('classic', 'dark')
    const checks = { passed: 3, failed: 1, pending: 2, total: 6, failing: [], running: [], queued: 0 }
    expect(barCells(checks, 12, 5, barPaint(look, 'warning'))).toBe(
      'ASUAACugQAAAAAABASUAAEOtVAAAAAABASUAAFq7aQAAAAABASUAAHLIfQAAAAABASUAAInWkgAAAAABASUAAKHjpgAAAAABASUAADkP0gAAAAABASUAAKiL8wAAAAABASUAAInK8QAAAAABASUAAMfs/AAAAAABASUAALPi+QAAAAABASUAALzo+wAAAAAB',
    )
    expect(fullBarCells(8, look.variant.raster.mauve)).toBe(
      'ASUAAO85iAAAAAABASUAAPBJkgAAAAABASUAAPFYmwAAAAABASUAAPJopQAAAAABASUAAPR3rgAAAAABASUAAPWHuAAAAAABASUAAPaWwQAAAAABASUAAPemywAAAAAB',
    )
    expect(ruleCells(16, look.variant.raster.rule, look.theme.rule)).toBe(
      'ACUAAPemywAAAAABACUAAPipvgAAAAABACUAAPissQAAAAABACUAAPmuowAAAAABACUAAPmxlgAAAAABACUAAPq0iQAAAAABACUAAPO9iwAAAAABACUAAOvGjQAAAAABACUAAOTQkAAAAAABACUAANzZkgAAAAABACUAANXilAAAAAABACUAANncpwAAAAABACUAANzVuwAAAAABACUAAODPzgAAAAABACUAAOPI4gAAAAABACUAAOfC9QAAAAAB',
    )
    expect(spinnerCell(7, look.variant.raster.spinner, look.theme.spin)).toBe('JygAAJvW9QAAAAAB')
  })
})

describe('the other looks', () => {
  test('a card keeps its width in every theme: a border, or padding standing for it', async () => {
    for (const name of THEMES) {
      for (const tone of TONES) {
        const box = cardBox(lookOf(name, 'dark'), tone)
        const side = (box.borderStyle === undefined ? 0 : 1) + Number(box.paddingX)
        expect(side).toBe(2)
      }
    }
  })

  test('solid: a borderless tile, tinted by state, a padding row for each border row', async () => {
    const dark = lookOf('solid', 'dark')
    const light = lookOf('solid', 'light')
    const tile = cardBox(dark, 'success')
    expect(tile.borderStyle).toBeUndefined()
    expect(tile).toMatchObject({ paddingX: 2, paddingY: 1, marginBottom: 1 })
    expect(tile.backgroundColor).toMatch(/^#[0-9a-f]{6}$/)
    expect(cardBox(light, 'success').backgroundColor).not.toBe(tile.backgroundColor)
    // Green ready, red failing, amber waiting, purple merged, neutral otherwise.
    const fills = TONES.map(tone => cardBox(dark, tone).backgroundColor)
    expect(new Set(fills).size).toBe(5)
    expect(cardBox(dark, 'subtle').backgroundColor).toBe(cardBox(dark, 'suggestion').backgroundColor)
    // The bar's cells sit on the tile's own colour.
    expect(rasterBg(dark, 'error')).toBe(parseInt(String(cardBox(dark, 'error').backgroundColor).slice(1), 16))
  })

  test('minimal: no box, a rule between PRs, colour on the pill and headline only', async () => {
    const look = lookOf('minimal', 'light')
    const box = cardBox(look, 'error')
    expect(box.borderStyle).toBeUndefined()
    expect(box.backgroundColor).toBeUndefined()
    expect(look.theme.card.separator).toBe('─')
    expect(pillOf(look, 'error', '✗ CI FAILED')).toMatchObject({ open: '', close: '', props: { color: 'error', bold: true } })
    expect(inkOf(look, 'error')).toBe('error')
    for (const role of ['number', 'steps', 'branch', 'diff'] as const) expect(inkOf(look, 'error', role)).toBeUndefined()
  })

  test('neon: bold vivid borders that glow under the pointer', async () => {
    for (const mode of MODES) {
      const box = cardBox(lookOf('neon', mode), 'merged')
      expect(box.borderStyle).toBe('bold')
      expect(box.borderColor).toMatch(/^#/)
      expect(box.borderDimColor).toBeUndefined()
      expect(box.hover?.borderColor).toMatch(/^#/)
      expect(box.hover?.borderColor).not.toBe(box.borderColor)
    }
  })

  test('retro: double borders in the 16 colours, a bracketed pill, a ticking spinner', async () => {
    const look = lookOf('retro', 'dark')
    expect(cardBox(look, 'warning')).toMatchObject({ borderStyle: 'double', borderColor: 'ansi256(3)' })
    expect(pillOf(look, 'success', '✓ READY')).toMatchObject({ open: '[', close: ']', props: { color: 'ansi256(2)' } })
    expect(look.theme.spin).toEqual(['|', '/', '-', '\\'])
    expect(look.theme.rule).toBe('═')
  })
})
