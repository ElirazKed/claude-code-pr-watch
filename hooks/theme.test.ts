import { describe, expect, test } from 'claude-code/testing'

import { barCells, fullBarCells, meterCells, ruleCells, ruleText, spinnerCell } from './look'
import {
  MODES,
  THEME,
  THEMES,
  TONES,
  barPaint,
  cardBox,
  hexOf,
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
const decode = (cells: string) => new Uint32Array(Uint8Array.from(atob(cells), c => c.charCodeAt(0)).buffer)
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
      // One glyph, or a pattern of them, each a single cell.
      expect([...THEME[name].rule].length).toBeGreaterThan(0)
      for (const glyph of THEME[name].rule) expect(glyph.codePointAt(0)!).toBeLessThan(0x10000)
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
    expect(unknownTheme('sparkly')).toBe('No theme "sparkly". Themes: classic, solid, minimal, neon, retro, tide, aurora.')
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

describe('the banner looks', () => {
  test('tide and aurora paint banner cards; every other theme stacks its cards', async () => {
    for (const name of THEMES) {
      const theme = THEME[name]
      const isBanner = name === 'tide' || name === 'aurora'
      expect(theme.layout).toBe(isBanner ? 'banner' : 'stack')
      expect(theme.art).toBe(isBanner ? name : null)
      for (const mode of MODES) {
        const variant = theme[mode]
        expect(variant.art !== null).toBe(isBanner)
        expect(variant.panel).toMatch(/^#[0-9a-f]{6}$/)
        expect(isRgb(variant.raster.track)).toBe(true)
      }
    }
  })

  test('a banner theme paints every tone in light and in dark: glass, liquid or curtains, crest, badge and accent', async () => {
    for (const name of ['tide', 'aurora'] as const) {
      for (const mode of MODES) {
        const art = THEME[name][mode].art!
        expect(Object.keys(art.tones).sort()).toEqual([...TONES].sort())
        for (const tone of TONES) {
          const { glass, surface, deep, crest, badge, ink } = art.tones[tone]
          for (const rgb of [glass, surface, deep, crest, badge, ink]) expect(isRgb(rgb)).toBe(true)
          // The text keys the band and the rows under a card use: the accent, on the glass.
          expect(THEME[name][mode].tones[tone]).toEqual({ ink: hexOf(ink), border: null, fill: hexOf(glass), glow: null })
        }
        expect(isRgb(art.light) && isRgb(art.dark)).toBe(true)
      }
      // Each mode its own paint.
      expect(THEME[name].light.art).not.toEqual(THEME[name].dark.art)
      expect(THEME[name].card.border).toBeNull()
    }
    expect(lookOf('tide', 'dark').theme.rule).toBe('▁▂▃▄▅▄▃▂')
  })

  test("the header's rule repeats a pattern of glyphs across, through the gradient", async () => {
    const words = decode(ruleCells(10, [[0, 0, 0], [255, 255, 255]], '▁▂▃'))
    expect([0, 3, 6, 9, 12, 15, 18, 21, 24, 27].map(i => String.fromCodePoint(words[i]!)).join('')).toBe('▁▂▃▁▂▃▁▂▃▁')
    expect(ruleText(7, '▁▂▃')).toBe('▁▂▃▁▂▃▁')
  })
})

describe('the gh meter', () => {
  const paint = lookOf('classic', 'dark').variant.raster
  test("a row of ▀: GraphQL's share on the top half, REST's on the bottom, over the track", async () => {
    const words = decode(meterCells({ share: 0.5, health: null }, { share: 1, health: 'error' }, 10, paint))
    expect(words).toHaveLength(30)
    const cell = (i: number) => ({ cp: words[i * 3]!, top: words[i * 3 + 1]!, bottom: words[i * 3 + 2]! })
    expect(new Set([...Array(10).keys()].map(i => cell(i).cp))).toEqual(new Set([0x2580]))
    const hex = ([r, g, b]: readonly number[]) => (r! << 16) | (g! << 8) | b!
    // Half full: the first five cells in the green ramp, the rest the track.
    expect(cell(0).top).toBe(hex(paint.green[0]))
    expect([5, 6, 9].map(i => cell(i).top)).toEqual([hex(paint.track), hex(paint.track), hex(paint.track)])
    // Full and spent, the bottom ends in red.
    expect(cell(9).bottom).toBe(hex(paint.red[1]))
  })

  test('a half not known yet is the terminal\'s own background; a share between cells blends the cell', async () => {
    const words = decode(meterCells({ share: 0.25, health: 'warning' }, null, 10, paint))
    expect(words[2]).toBe(0x01000000)
    // 2.5 cells: the third cell is half filled.
    const third = words[2 * 3 + 1]!
    expect(third).not.toBe(words[1 * 3 + 1])
    expect(third).not.toBe(words[3 * 3 + 1])
  })

  test("an unknown top half is never the terminal's foreground: ▄ carries the bottom alone", async () => {
    const words = decode(meterCells(null, { share: 0.5, health: null }, 4, paint))
    expect([0, 1, 2, 3].map(i => words[i * 3])).toEqual([0x2584, 0x2584, 0x2584, 0x2584])
    expect([0, 1, 2, 3].map(i => words[i * 3 + 2])).toEqual([0x01000000, 0x01000000, 0x01000000, 0x01000000])
    expect([...decode(meterCells(null, null, 2, paint))]).toEqual([0x20, 0x01000000, 0x01000000, 0x20, 0x01000000, 0x01000000])
  })
})
