// The pane's looks: borders, backgrounds, colour and feel, as data. Each theme gives a card
// frame, a pill, a header rule and a spinner, and per mode (the person's light or dark
// terminal) what each tone paints. The render reads a Look; it never asks which theme it is.
import type { BoxHoverProps, BoxProps, Color } from 'claude-code'
import type { Tone } from '../types'
import type { BarPaint } from './look'

export const THEMES = ['classic', 'solid', 'minimal', 'neon', 'retro'] as const
export type ThemeName = (typeof THEMES)[number]
export type Mode = 'light' | 'dark'
export const MODES: readonly Mode[] = ['light', 'dark']
export const TONES: readonly Tone[] = ['success', 'error', 'warning', 'merged', 'subtle', 'suggestion']

export type Rgb = readonly [number, number, number]
// From, to: a run of cells fades from one to the other.
export type Ramp = readonly [Rgb, Rgb]

// What one tone paints in one mode.
export type ToneLook = {
  // Text accents: the pill, the headline, the number, the current step, the band's line.
  ink: Color
  // The card's border, where the card has one.
  border: Color | null
  // The card's background: hex, so a Raster on the card can paint the same behind its cells.
  fill: `#${string}` | null
  // The border while the pointer is on the card.
  glow: Color | null
}

// The Raster paint (look.ts): CI bars by outcome, the light sweeping over pending checks, the
// header rule's gradient stops and the spinner's pulse.
export type RasterPaint = {
  green: Ramp
  red: Ramp
  amber: Ramp
  mauve: Ramp
  shine: Rgb
  rule: readonly Rgb[]
  spinner: Ramp
}

export type Variant = {
  tones: Record<Tone, ToneLook>
  // The pill's text where the pill is filled with the tone.
  pillText: Color
  raster: RasterPaint
}

// The accents a quiet theme leaves uncoloured.
export type Role = 'number' | 'steps' | 'branch' | 'diff'

export type Theme = {
  name: ThemeName
  // One line for the theme listing.
  blurb: string
  card: {
    // A terminal border style (single, double, round, bold, classic…), or none.
    border: string | null
    // Drawn dim, full strength under the pointer.
    isDim: boolean
    // Rows of padding above and below the card's text; a border stands for one row each.
    padY: number
    // Blank rows after each card.
    gap: number
    // A rule drawn between cards, by its glyph.
    separator: string | null
  }
  // `open` and `close` go around the pill's text; filled, the tone is its background.
  pill: { open: string; close: string; isFilled: boolean }
  // The header rule's glyph, and the spinner's frames.
  rule: string
  spin: readonly string[]
  muted: readonly Role[]
  light: Variant
  dark: Variant
}

const BRAILLE = [...'⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏']

// Today's paint, Catppuccin-flavoured, a shade deeper so it still reads on a light terminal:
// one for both modes.
const CATPPUCCIN: RasterPaint = {
  green: [[64, 160, 43], [166, 227, 161]],
  red: [[210, 15, 57], [243, 139, 168]],
  amber: [[223, 142, 29], [249, 226, 175]],
  mauve: [[136, 57, 239], [203, 166, 247]],
  shine: [255, 248, 220],
  rule: [
    [203, 166, 247],
    [137, 180, 250],
    [148, 226, 213],
    [245, 194, 231],
  ],
  spinner: [[223, 142, 29], [249, 226, 175]],
}

// The person's own theme colours: they follow light, dark and daltonized by themselves.
const KEYED: Record<Tone, ToneLook> = {
  success: { ink: 'success', border: 'success', fill: null, glow: null },
  error: { ink: 'error', border: 'error', fill: null, glow: null },
  warning: { ink: 'warning', border: 'warning', fill: null, glow: null },
  merged: { ink: 'merged', border: 'merged', fill: null, glow: null },
  subtle: { ink: 'subtle', border: 'subtle', fill: null, glow: null },
  suggestion: { ink: 'suggestion', border: 'suggestion', fill: null, glow: null },
}

const classic: Theme = {
  name: 'classic',
  blurb: 'round borders coloured by state, a filled pill',
  card: { border: 'round', isDim: true, padY: 0, gap: 1, separator: null },
  pill: { open: ' ', close: ' ', isFilled: true },
  rule: '─',
  spin: BRAILLE,
  muted: [],
  light: { tones: KEYED, pillText: 'inverseText', raster: CATPPUCCIN },
  dark: { tones: KEYED, pillText: 'inverseText', raster: CATPPUCCIN },
}

// A soft tint per state, the theme's own keys for the text on it. Neutral where nothing is
// asked of the person.
const tile = (fills: Record<'green' | 'red' | 'amber' | 'purple' | 'neutral', `#${string}`>): Record<Tone, ToneLook> => ({
  success: { ink: 'success', border: null, fill: fills.green, glow: null },
  error: { ink: 'error', border: null, fill: fills.red, glow: null },
  warning: { ink: 'warning', border: null, fill: fills.amber, glow: null },
  merged: { ink: 'merged', border: null, fill: fills.purple, glow: null },
  subtle: { ink: 'subtle', border: null, fill: fills.neutral, glow: null },
  suggestion: { ink: 'suggestion', border: null, fill: fills.neutral, glow: null },
})

const solid: Theme = {
  name: 'solid',
  blurb: 'no borders: each card a tile tinted by its state',
  // A row of padding each side stands for the border's rows, so a card keeps its size.
  card: { border: null, isDim: false, padY: 1, gap: 1, separator: null },
  pill: { open: ' ', close: ' ', isFilled: true },
  rule: '─',
  spin: BRAILLE,
  muted: [],
  light: {
    tones: tile({ green: '#e1f3e4', red: '#fae1e5', amber: '#faefd6', purple: '#ece3f9', neutral: '#eaecf1' }),
    pillText: 'inverseText',
    raster: CATPPUCCIN,
  },
  dark: {
    tones: tile({ green: '#1d3524', red: '#3d1f26', amber: '#3a3020', purple: '#2e2442', neutral: '#272a33' }),
    pillText: 'inverseText',
    raster: CATPPUCCIN,
  },
}

// The same keys with no frame to colour.
const BARE = Object.fromEntries(TONES.map(tone => [tone, { ...KEYED[tone], border: null }])) as Record<Tone, ToneLook>

const minimal: Theme = {
  name: 'minimal',
  blurb: 'no boxes, a thin rule between PRs, colour on the pill and headline only',
  card: { border: null, isDim: false, padY: 0, gap: 1, separator: '─' },
  pill: { open: '', close: '', isFilled: false },
  rule: '─',
  spin: BRAILLE,
  muted: ['number', 'steps', 'branch', 'diff'],
  light: {
    tones: BARE,
    pillText: 'inverseText',
    raster: {
      green: [[78, 138, 88], [104, 158, 112]],
      red: [[176, 70, 88], [196, 104, 118]],
      amber: [[168, 120, 50], [190, 148, 86]],
      mauve: [[124, 96, 176], [150, 126, 196]],
      shine: [236, 236, 244],
      rule: [[176, 179, 192], [176, 179, 192]],
      spinner: [[120, 120, 140], [70, 70, 90]],
    },
  },
  dark: {
    tones: BARE,
    pillText: 'inverseText',
    raster: {
      green: [[86, 140, 96], [124, 172, 132]],
      red: [[170, 80, 96], [204, 124, 136]],
      amber: [[170, 140, 80], [204, 178, 122]],
      mauve: [[130, 110, 170], [166, 146, 202]],
      shine: [214, 214, 226],
      rule: [[88, 91, 112], [88, 91, 112]],
      spinner: [[140, 140, 160], [200, 200, 216]],
    },
  },
}

const vivid = (inks: Record<Tone, readonly [ink: `#${string}`, glow: `#${string}`]>): Record<Tone, ToneLook> =>
  Object.fromEntries(TONES.map(tone => [tone, { ink: inks[tone][0], border: inks[tone][0], fill: null, glow: inks[tone][1] }])) as Record<Tone, ToneLook>

const neon: Theme = {
  name: 'neon',
  blurb: 'bold borders in vivid colour that glow under the pointer, a bright gradient rule',
  card: { border: 'bold', isDim: false, padY: 0, gap: 1, separator: null },
  pill: { open: ' ', close: ' ', isFilled: true },
  rule: '━',
  spin: BRAILLE,
  muted: [],
  // Deeper on a light terminal, so the vivid colour still reads on white; the pill's text white.
  light: {
    tones: vivid({
      success: ['#00a63e', '#00d452'],
      error: ['#d6006f', '#ff2b93'],
      warning: ['#c27a00', '#f09c00'],
      merged: ['#8f00d6', '#b33bff'],
      subtle: ['#5c5c7a', '#8080a8'],
      suggestion: ['#0077d6', '#1f9bff'],
    }),
    pillText: '#ffffff',
    raster: {
      green: [[0, 150, 56], [0, 212, 82]],
      red: [[214, 0, 111], [255, 43, 147]],
      amber: [[194, 122, 0], [240, 156, 0]],
      mauve: [[143, 0, 214], [179, 59, 255]],
      shine: [255, 255, 255],
      rule: [
        [214, 0, 214],
        [0, 140, 214],
        [0, 166, 62],
        [214, 130, 0],
      ],
      spinner: [[0, 119, 214], [196, 0, 196]],
    },
  },
  // Full strength on a dark one; the pill's text near-black.
  dark: {
    tones: vivid({
      success: ['#39ff14', '#c2ffb0'],
      error: ['#ff2079', '#ffa3c9'],
      warning: ['#ffd300', '#fff2a6'],
      merged: ['#bc13fe', '#e6b0ff'],
      subtle: ['#9a9ac0', '#dcdcf2'],
      suggestion: ['#00f0ff', '#b0fcff'],
    }),
    pillText: '#0b0b16',
    raster: {
      green: [[57, 255, 20], [176, 255, 150]],
      red: [[255, 32, 121], [255, 140, 186]],
      amber: [[255, 211, 0], [255, 242, 150]],
      mauve: [[188, 19, 254], [228, 150, 255]],
      shine: [255, 255, 255],
      rule: [
        [255, 0, 255],
        [0, 255, 255],
        [57, 255, 20],
        [255, 211, 0],
      ],
      spinner: [[0, 240, 255], [255, 0, 255]],
    },
  },
}

// The terminal's own 16 colours, by their numbers (`ansi256(0)` to `ansi256(15)` are the
// palette's), which the terminal fits to light or dark itself.
const sixteen = (n: number): ToneLook => ({ ink: `ansi256(${n})`, border: `ansi256(${n})`, fill: null, glow: null })
const SIXTEEN: Record<Tone, ToneLook> = {
  success: sixteen(2),
  error: sixteen(1),
  warning: sixteen(3),
  merged: sixteen(5),
  subtle: sixteen(8),
  suggestion: sixteen(6),
}

// Flat runs, as an old terminal would draw them: no fade from one end to the other.
const flat = (rgb: Rgb): Ramp => [rgb, rgb]

const retro: Theme = {
  name: 'retro',
  blurb: 'double borders, the 16 terminal colours, [bracketed] pills: an old TUI',
  card: { border: 'double', isDim: true, padY: 0, gap: 1, separator: null },
  pill: { open: '[', close: ']', isFilled: false },
  rule: '═',
  spin: [...'|/-\\'],
  muted: [],
  light: {
    tones: SIXTEEN,
    pillText: 'inverseText',
    raster: {
      green: flat([0, 140, 0]),
      red: flat([180, 0, 0]),
      amber: flat([160, 120, 0]),
      mauve: flat([150, 0, 150]),
      shine: [255, 255, 255],
      rule: [[0, 128, 128], [0, 128, 128]],
      spinner: flat([0, 128, 128]),
    },
  },
  dark: {
    tones: SIXTEEN,
    pillText: 'inverseText',
    raster: {
      green: flat([0, 205, 0]),
      red: flat([205, 0, 0]),
      amber: flat([205, 205, 0]),
      mauve: flat([205, 0, 205]),
      shine: [255, 255, 255],
      rule: [[0, 205, 205], [0, 205, 205]],
      spinner: flat([0, 205, 205]),
    },
  },
}

export const THEME: Record<ThemeName, Theme> = { classic, solid, minimal, neon, retro }

// A theme by its name, in any case; null for a name no theme has.
export function themeOf(value: unknown): ThemeName | null {
  if (typeof value !== 'string') return null
  const name = value.trim().toLowerCase()

  return (THEMES as readonly string[]).includes(name) ? (name as ThemeName) : null
}

// The `theme` option: a theme's name, else the classic look.
export const optionTheme = (value: unknown): ThemeName => themeOf(value) ?? 'classic'

// Light or dark from Claude Code's own `theme` row (light, light-daltonized, dark-ansi…); null
// where it doesn't say (auto, a custom theme).
export function modeOf(value: unknown): Mode | null {
  if (typeof value !== 'string') return null
  const name = value.trim().toLowerCase()

  return name.startsWith('light') ? 'light' : name.startsWith('dark') ? 'dark' : null
}

// Light or dark from the terminal's COLORFGBG ("15;0": light text on black), where it sets one:
// the background is its last number, 7 and 9 to 15 light.
export function modeOfColours(value: unknown): Mode | null {
  if (typeof value !== 'string') return null
  const bg = value.trim().split(';').at(-1) ?? ''
  if (!/^\d{1,2}$/.test(bg)) return null
  const n = Number(bg)

  return n === 7 || (n >= 9 && n <= 15) ? 'light' : 'dark'
}

export type Look = { theme: Theme; variant: Variant }

export const lookOf = (name: ThemeName, mode: Mode): Look => ({ theme: THEME[name], variant: THEME[name][mode] })

// A text accent's colour; undefined where the theme leaves that accent plain.
export function inkOf(look: Look, tone: Tone, role?: Role): Color | undefined {
  if (role !== undefined && look.theme.muted.includes(role)) return undefined

  return look.variant.tones[tone].ink
}

// A card's frame. Without a border, padding stands for it, so a card keeps its width (and,
// with `padY`, its height) whichever theme draws it: the text always has width - 4.
export function cardBox(look: Look, tone: Tone): BoxProps {
  const { card } = look.theme
  const paint = look.variant.tones[tone]
  const hover: BoxHoverProps = {}
  const box: BoxProps = {}
  if (card.border !== null) {
    box.borderStyle = card.border
    if (paint.border !== null) box.borderColor = paint.border
    if (card.isDim) {
      box.borderDimColor = true
      hover.borderDimColor = false
    }
    if (paint.glow !== null) hover.borderColor = paint.glow
  }
  if (paint.fill !== null) box.backgroundColor = paint.fill
  if (Object.keys(hover).length > 0) box.hover = hover
  box.paddingX = card.border === null ? 2 : 1
  if (card.padY > 0) box.paddingY = card.padY
  box.marginBottom = card.gap

  return box
}

// The pill: its text, and the Text props it is drawn with.
export function pillOf(look: Look, tone: Tone, pill: string) {
  const { open, close, isFilled } = look.theme.pill
  const ink = look.variant.tones[tone].ink

  return {
    open,
    close,
    text: pill,
    props: isFilled ? { backgroundColor: ink, color: look.variant.pillText, bold: true } : { color: ink, bold: true },
  }
}

// What a Raster on a card paints behind its cells: the card's fill, as 0xRRGGBB, or null for
// the terminal's own background.
export function rasterBg(look: Look, tone: Tone): number | null {
  const fill = look.variant.tones[tone].fill

  return fill === null || !/^#[0-9a-f]{6}$/i.test(fill) ? null : parseInt(fill.slice(1), 16)
}

// A card's CI bar: the theme's ramps over the card's fill.
export function barPaint(look: Look, tone: Tone): BarPaint {
  const { green, red, amber, shine } = look.variant.raster

  return { green, red, amber, shine, bg: rasterBg(look, tone) }
}

// `/pr-watch theme`: every theme in a line, the current one marked.
export function themeListing(current: ThemeName): string {
  const width = Math.max(...THEMES.map(name => name.length))

  return [
    'pr-watch themes:',
    ...THEMES.map(name => `${name === current ? '●' : '○'} ${name.padEnd(width)}  ${THEME[name].blurb}${name === current ? ' (current)' : ''}`),
    '/pr-watch theme <name> switches.',
  ].join('\n')
}

export const unknownTheme = (name: string) => `No theme "${name}". Themes: ${THEMES.join(', ')}.`
