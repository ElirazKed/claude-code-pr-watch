// The pane's looks: borders, backgrounds, colour and feel, as data. Each theme gives a card
// frame, a pill, a header rule and a spinner, and per mode (the person's light or dark
// terminal) what each tone paints. The render reads a Look; it never asks which theme it is.
import type { BoxHoverProps, BoxProps, Color } from 'claude-code'
import type { Tone } from '../types'
import type { BarPaint } from './look'
import { x256 } from './look'

export const THEMES = ['classic', 'solid', 'minimal', 'neon', 'retro', 'tide', 'aurora'] as const
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
  // The card's background: hex, so a Raster on the card can paint the same behind its cells (a
  // banner theme's 256-colour paint: the palette entry its art paints, `ansi256(n)`).
  fill: Color | null
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
  // The empty part of the gh meter's bars.
  track: Rgb
  // Every colour one of the 256-colour palette's (a theme's ansi256 paint): a bar is painted in
  // them alone.
  isPalette?: true
}

// How a card is laid out. stack: a framed box of text rows, a CI bar and a stepper. banner: a
// painted card (art.ts), the text drawn into its cells, its progress the art itself, and the
// links and buttons on rows of their own under it.
export type CardLayout = 'stack' | 'banner'
// What a banner card's cells are painted with: a liquid that advances with the PR's progress
// behind a wavy front, or soft curtains of light.
export type ArtKind = 'tide' | 'aurora'

// One tone's paint on a banner card.
export type ArtTone = {
  // The card where the art hasn't reached: tide's empty glass, aurora's sky.
  glass: Rgb
  // Tide: the liquid at its surface and deep down, and the light on its crest. Aurora: the
  // curtains' colours, across the card.
  surface: Rgb
  deep: Rgb
  crest: Rgb
  // The state badge's chip.
  badge: Rgb
  // The tone's text accents on the art (each kept only where it reads, art.ts).
  ink: Rgb
}

// A tone's extra paint where the terminal shows 256 colours: tide's middle layer of water, and
// aurora's curtains dimmed (for `deep`, `crest` and `surface`, in that order).
export type ArtSteps = { mid: Rgb; dim: readonly [Rgb, Rgb, Rgb] }

export type ArtPaint = {
  tones: Record<Tone, ArtTone>
  // The two text colours, whichever reads better on a cell's background is drawn there.
  light: Rgb
  dark: Rgb
  // Present: the art is painted for 256 colours (art.ts), in a few palette colours each tone
  // has rather than gradients, which the terminal would round to blotches of other colours.
  steps?: Record<Tone, ArtSteps>
}

export type Variant = {
  tones: Record<Tone, ToneLook>
  // The pill's text where the pill is filled with the tone.
  pillText: Color
  raster: RasterPaint
  // A banner card's paint (null: the theme stacks its cards).
  art: ArtPaint | null
  // Behind a note drawn over the pane (the gh meter's explanation under the pointer).
  panel: Color
}

// The accents a quiet theme leaves uncoloured.
export type Role = 'number' | 'steps' | 'branch' | 'diff'

export type Theme = {
  name: ThemeName
  // One line for the theme listing.
  blurb: string
  layout: CardLayout
  // What a banner card is painted with (null for a stacked one).
  art: ArtKind | null
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
  // The header rule's glyphs, repeated across, and the spinner's frames.
  rule: string
  spin: readonly string[]
  muted: readonly Role[]
  light: Variant
  dark: Variant
  // Where the terminal shows 256 colours (depthOf), the paint made of the palette's own
  // colours; without one, the same paint (the terminal rounds it).
  ansi256?: { light: Variant; dark: Variant }
}

const BRAILLE = [...'⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏']

// Behind a note drawn over the pane, by mode.
const PANEL = { light: '#f3f4f8', dark: '#1c1f2b' } as const

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
  track: [108, 112, 134],
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
  layout: 'stack',
  art: null,
  card: { border: 'round', isDim: true, padY: 0, gap: 1, separator: null },
  pill: { open: ' ', close: ' ', isFilled: true },
  rule: '─',
  spin: BRAILLE,
  muted: [],
  light: { tones: KEYED, pillText: 'inverseText', raster: CATPPUCCIN, art: null, panel: PANEL.light },
  dark: { tones: KEYED, pillText: 'inverseText', raster: CATPPUCCIN, art: null, panel: PANEL.dark },
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
  layout: 'stack',
  art: null,
  card: { border: null, isDim: false, padY: 1, gap: 1, separator: null },
  pill: { open: ' ', close: ' ', isFilled: true },
  rule: '─',
  spin: BRAILLE,
  muted: [],
  light: {
    tones: tile({ green: '#e1f3e4', red: '#fae1e5', amber: '#faefd6', purple: '#ece3f9', neutral: '#eaecf1' }),
    pillText: 'inverseText',
    art: null,
    panel: PANEL.light,
    raster: CATPPUCCIN,
  },
  dark: {
    tones: tile({ green: '#1d3524', red: '#3d1f26', amber: '#3a3020', purple: '#2e2442', neutral: '#272a33' }),
    pillText: 'inverseText',
    art: null,
    panel: PANEL.dark,
    raster: CATPPUCCIN,
  },
}

// The same keys with no frame to colour.
const BARE = Object.fromEntries(TONES.map(tone => [tone, { ...KEYED[tone], border: null }])) as Record<Tone, ToneLook>

const minimal: Theme = {
  name: 'minimal',
  blurb: 'no boxes, a thin rule between PRs, colour on the pill and headline only',
  layout: 'stack',
  art: null,
  card: { border: null, isDim: false, padY: 0, gap: 1, separator: '─' },
  pill: { open: '', close: '', isFilled: false },
  rule: '─',
  spin: BRAILLE,
  muted: ['number', 'steps', 'branch', 'diff'],
  light: {
    tones: BARE,
    pillText: 'inverseText',
    art: null,
    panel: PANEL.light,
    raster: {
      green: [[78, 138, 88], [104, 158, 112]],
      red: [[176, 70, 88], [196, 104, 118]],
      amber: [[168, 120, 50], [190, 148, 86]],
      mauve: [[124, 96, 176], [150, 126, 196]],
      shine: [236, 236, 244],
      rule: [[176, 179, 192], [176, 179, 192]],
      spinner: [[120, 120, 140], [70, 70, 90]],
      track: [214, 216, 224],
    },
  },
  dark: {
    tones: BARE,
    pillText: 'inverseText',
    art: null,
    panel: PANEL.dark,
    raster: {
      green: [[86, 140, 96], [124, 172, 132]],
      red: [[170, 80, 96], [204, 124, 136]],
      amber: [[170, 140, 80], [204, 178, 122]],
      mauve: [[130, 110, 170], [166, 146, 202]],
      shine: [214, 214, 226],
      rule: [[88, 91, 112], [88, 91, 112]],
      spinner: [[140, 140, 160], [200, 200, 216]],
      track: [58, 60, 74],
    },
  },
}

const vivid = (inks: Record<Tone, readonly [ink: `#${string}`, glow: `#${string}`]>): Record<Tone, ToneLook> =>
  Object.fromEntries(TONES.map(tone => [tone, { ink: inks[tone][0], border: inks[tone][0], fill: null, glow: inks[tone][1] }])) as Record<Tone, ToneLook>

const neon: Theme = {
  name: 'neon',
  blurb: 'bold borders in vivid colour that glow under the pointer, a bright gradient rule',
  layout: 'stack',
  art: null,
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
    art: null,
    panel: PANEL.light,
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
      track: [222, 222, 236],
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
    art: null,
    panel: PANEL.dark,
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
      track: [44, 44, 70],
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
  layout: 'stack',
  art: null,
  card: { border: 'double', isDim: true, padY: 0, gap: 1, separator: null },
  pill: { open: '[', close: ']', isFilled: false },
  rule: '═',
  spin: [...'|/-\\'],
  muted: [],
  light: {
    tones: SIXTEEN,
    pillText: 'inverseText',
    art: null,
    panel: PANEL.light,
    raster: {
      green: flat([0, 140, 0]),
      red: flat([180, 0, 0]),
      amber: flat([160, 120, 0]),
      mauve: flat([150, 0, 150]),
      shine: [255, 255, 255],
      rule: [[0, 128, 128], [0, 128, 128]],
      spinner: flat([0, 128, 128]),
      track: [192, 192, 192],
    },
  },
  dark: {
    tones: SIXTEEN,
    pillText: 'inverseText',
    art: null,
    panel: PANEL.dark,
    raster: {
      green: flat([0, 205, 0]),
      red: flat([205, 0, 0]),
      amber: flat([205, 205, 0]),
      mauve: flat([205, 0, 205]),
      shine: [255, 255, 255],
      rule: [[0, 205, 205], [0, 205, 205]],
      spinner: flat([0, 205, 205]),
      track: [96, 96, 96],
    },
  },
}

// ---- Banner themes: painted cards (art.ts) ------------------------------------------------

export const hexOf = ([r, g, b]: Rgb): `#${string}` => `#${[r, g, b].map(n => n.toString(16).padStart(2, '0')).join('')}`

// The text keys of a banner theme: each tone's accent as hex, its glass behind the card's rows of
// links and buttons.
const painted = (art: ArtPaint): Record<Tone, ToneLook> =>
  Object.fromEntries(
    TONES.map(tone => [tone, { ink: hexOf(art.tones[tone].ink), border: null, fill: hexOf(art.tones[tone].glass), glow: null }]),
  ) as Record<Tone, ToneLook>

// A night sea: the liquid deep and dark enough under light text, its crest lit.
const TIDE_DARK: ArtPaint = {
  light: [246, 249, 252],
  dark: [8, 10, 14],
  tones: {
    success: { glass: [17, 29, 26], surface: [24, 100, 72], deep: [8, 58, 48], crest: [96, 214, 160], badge: [72, 214, 150], ink: [132, 240, 186] },
    error: { glass: [33, 19, 23], surface: [146, 36, 62], deep: [84, 14, 36], crest: [244, 128, 150], badge: [255, 104, 132], ink: [255, 154, 172] },
    warning: { glass: [33, 28, 17], surface: [126, 80, 14], deep: [84, 46, 8], crest: [240, 190, 96], badge: [255, 192, 72], ink: [255, 212, 124] },
    merged: { glass: [26, 21, 40], surface: [92, 54, 172], deep: [48, 24, 106], crest: [186, 150, 255], badge: [178, 136, 255], ink: [204, 176, 255] },
    subtle: { glass: [23, 26, 33], surface: [64, 72, 94], deep: [36, 42, 58], crest: [150, 162, 188], badge: [152, 164, 192], ink: [178, 188, 210] },
    suggestion: { glass: [17, 27, 39], surface: [28, 88, 156], deep: [12, 48, 98], crest: [120, 190, 250], badge: [98, 178, 255], ink: [144, 204, 255] },
  },
}

// A day sea: pale water deepening to colour, under dark text.
const TIDE_LIGHT: ArtPaint = {
  light: [255, 255, 255],
  dark: [26, 29, 36],
  tones: {
    success: { glass: [236, 246, 240], surface: [160, 226, 192], deep: [92, 190, 146], crest: [226, 255, 240], badge: [22, 128, 82], ink: [10, 98, 60] },
    error: { glass: [251, 237, 240], surface: [246, 180, 192], deep: [226, 120, 142], crest: [255, 232, 237], badge: [190, 30, 66], ink: [150, 20, 50] },
    warning: { glass: [251, 245, 231], surface: [248, 212, 140], deep: [226, 166, 70], crest: [255, 243, 210], badge: [168, 98, 0], ink: [124, 74, 0] },
    merged: { glass: [243, 238, 252], surface: [206, 186, 248], deep: [158, 126, 230], crest: [240, 232, 255], badge: [110, 56, 200], ink: [86, 36, 168] },
    subtle: { glass: [239, 241, 246], surface: [206, 212, 224], deep: [160, 170, 190], crest: [247, 249, 253], badge: [84, 94, 116], ink: [64, 74, 94] },
    suggestion: { glass: [235, 244, 252], surface: [170, 210, 248], deep: [104, 166, 230], crest: [228, 242, 255], badge: [20, 100, 190], ink: [8, 78, 160] },
  },
}

// Ocean water for the gh meter and the header's wave.
const tideRaster = (mode: Mode): RasterPaint =>
  mode === 'dark'
    ? {
        green: [[24, 150, 104], [96, 224, 168]],
        red: [[196, 44, 76], [255, 128, 150]],
        amber: [[196, 128, 20], [255, 204, 110]],
        mauve: [[110, 66, 200], [186, 150, 255]],
        shine: [230, 248, 255],
        rule: [[22, 70, 132], [28, 140, 168], [104, 214, 200], [28, 140, 168]],
        spinner: [[96, 214, 200], [144, 204, 255]],
        track: [40, 50, 66],
      }
    : {
        green: [[26, 150, 98], [92, 200, 150]],
        red: [[196, 36, 70], [232, 110, 134]],
        amber: [[190, 120, 10], [232, 170, 70]],
        mauve: [[110, 56, 200], [158, 126, 230]],
        shine: [255, 255, 255],
        rule: [[30, 96, 170], [26, 150, 170], [84, 190, 170], [26, 150, 170]],
        spinner: [[20, 110, 190], [26, 150, 170]],
        track: [214, 224, 234],
      }

const tide: Theme = {
  name: 'tide',
  blurb: 'each card fills like a glass as its PR moves on, a wave on the front; links on their own row',
  layout: 'banner',
  art: 'tide',
  // The painted rows are the card's whole width: no frame, no padding.
  card: { border: null, isDim: false, padY: 0, gap: 1, separator: null },
  pill: { open: ' ', close: ' ', isFilled: true },
  rule: '▁▂▃▄▅▄▃▂',
  spin: BRAILLE,
  muted: [],
  light: { tones: painted(TIDE_LIGHT), pillText: '#ffffff', raster: tideRaster('light'), art: TIDE_LIGHT, panel: '#eef5fb' },
  dark: { tones: painted(TIDE_DARK), pillText: '#0d1117', raster: tideRaster('dark'), art: TIDE_DARK, panel: '#111b27' },
}

// A polar night: curtains of colour over a deep sky, kept dim enough under light text.
const AURORA_DARK: ArtPaint = {
  light: [240, 244, 250],
  dark: [10, 12, 20],
  tones: {
    success: { glass: [10, 18, 26], surface: [40, 150, 190], deep: [20, 120, 100], crest: [60, 200, 110], badge: [92, 226, 150], ink: [140, 244, 190] },
    error: { glass: [22, 12, 24], surface: [220, 110, 50], deep: [150, 26, 96], crest: [226, 50, 90], badge: [255, 112, 140], ink: [255, 160, 178] },
    warning: { glass: [22, 18, 14], surface: [210, 80, 110], deep: [176, 104, 16], crest: [226, 166, 36], badge: [255, 196, 80], ink: [255, 214, 130] },
    merged: { glass: [16, 12, 30], surface: [226, 86, 176], deep: [80, 46, 196], crest: [156, 76, 228], badge: [190, 146, 255], ink: [210, 182, 255] },
    subtle: { glass: [14, 16, 24], surface: [80, 96, 130], deep: [54, 64, 96], crest: [96, 110, 140], badge: [156, 168, 196], ink: [182, 192, 214] },
    suggestion: { glass: [10, 14, 30], surface: [126, 86, 228], deep: [26, 84, 196], crest: [36, 166, 226], badge: [110, 186, 255], ink: [150, 208, 255] },
  },
}

// Dawn: the same curtains in pastel over a pale sky.
const AURORA_LIGHT: ArtPaint = {
  light: [255, 255, 255],
  dark: [24, 26, 36],
  tones: {
    success: { glass: [244, 248, 250], surface: [160, 214, 244], deep: [140, 222, 200], crest: [172, 236, 168], badge: [20, 126, 86], ink: [10, 96, 62] },
    error: { glass: [251, 244, 247], surface: [252, 196, 150], deep: [240, 160, 200], crest: [250, 160, 176], badge: [186, 28, 72], ink: [148, 20, 54] },
    warning: { glass: [251, 248, 240], surface: [248, 174, 190], deep: [248, 206, 130], crest: [252, 226, 140], badge: [166, 96, 0], ink: [122, 72, 0] },
    merged: { glass: [247, 244, 252], surface: [246, 176, 222], deep: [180, 166, 246], crest: [212, 170, 248], badge: [108, 54, 198], ink: [84, 34, 166] },
    subtle: { glass: [244, 245, 249], surface: [204, 212, 228], deep: [192, 198, 216], crest: [214, 220, 232], badge: [84, 94, 116], ink: [64, 74, 94] },
    suggestion: { glass: [243, 246, 253], surface: [206, 186, 248], deep: [160, 196, 246], crest: [158, 226, 246], badge: [20, 100, 190], ink: [8, 78, 160] },
  },
}

const auroraRaster = (mode: Mode): RasterPaint =>
  mode === 'dark'
    ? {
        green: [[20, 140, 120], [80, 226, 150]],
        red: [[176, 30, 100], [250, 110, 120]],
        amber: [[200, 120, 20], [250, 200, 90]],
        mauve: [[90, 56, 210], [220, 110, 200]],
        shine: [240, 250, 255],
        rule: [[40, 200, 120], [36, 166, 226], [126, 86, 228], [226, 86, 176]],
        spinner: [[60, 200, 140], [156, 76, 228]],
        track: [36, 40, 58],
      }
    : {
        green: [[20, 140, 110], [80, 196, 150]],
        red: [[186, 28, 80], [236, 110, 140]],
        amber: [[186, 112, 0], [232, 172, 60]],
        mauve: [[108, 54, 198], [200, 100, 190]],
        shine: [255, 255, 255],
        rule: [[40, 170, 110], [30, 140, 200], [110, 80, 210], [200, 70, 160]],
        spinner: [[30, 140, 200], [110, 80, 210]],
        track: [222, 224, 236],
      }

const aurora: Theme = {
  name: 'aurora',
  blurb: 'each card a sky of soft northern lights in its state, brighter as its PR moves on',
  layout: 'banner',
  art: 'aurora',
  card: { border: null, isDim: false, padY: 0, gap: 1, separator: null },
  pill: { open: ' ', close: ' ', isFilled: true },
  rule: '━',
  spin: BRAILLE,
  muted: [],
  light: { tones: painted(AURORA_LIGHT), pillText: '#ffffff', raster: auroraRaster('light'), art: AURORA_LIGHT, panel: '#f5f3fb' },
  dark: { tones: painted(AURORA_DARK), pillText: '#0a0c14', raster: auroraRaster('dark'), art: AURORA_DARK, panel: '#141426' },
}

// ---- Banner themes in 256 colours ---------------------------------------------------------

// Inside tmux (unless CLAUDE_CODE_TMUX_TRUECOLOR is set), in Apple's Terminal and wherever the
// terminal doesn't say it takes 24-bit colour, Claude Code paints with the 256-colour palette,
// each colour rounded to its nearest there. The palette's darks are few and far apart, so a
// gradient comes out as blotches of other colours (a dark teal goes grey). These paints are
// made of palette colours (x256), a few steps a tone, so each step stays what it was picked as.

// A tone's palette colours: the glass; tide's water at its surface, in the middle and deep down,
// or aurora's curtains bright (deep, crest, surface) and `dim`; the crest; the badge; the accent.
type Tone256 = { glass: number; surface: number; mid: number; deep: number; crest: number; badge: number; ink: number; dim: readonly [number, number, number] }
type Paint256 = { light: number; dark: number; pillText: number; panel: number; tones: Record<Tone, Tone256> }

const byTone = <T>(make: (tone: Tone) => T): Record<Tone, T> => Object.fromEntries(TONES.map(tone => [tone, make(tone)])) as Record<Tone, T>

// A text colour Claude Code paints as that very palette colour.
const ansi = (n: number): Color => `ansi256(${n})`

const art256 = (p: Paint256): ArtPaint => ({
  light: x256(p.light),
  dark: x256(p.dark),
  tones: byTone(tone => {
    const t = p.tones[tone]

    return { glass: x256(t.glass), surface: x256(t.surface), deep: x256(t.deep), crest: x256(t.crest), badge: x256(t.badge), ink: x256(t.ink) }
  }),
  steps: byTone(tone => ({ mid: x256(p.tones[tone].mid), dim: [x256(p.tones[tone].dim[0]), x256(p.tones[tone].dim[1]), x256(p.tones[tone].dim[2])] })),
})

const variant256 = (p: Paint256, raster: RasterPaint): Variant => ({
  tones: byTone(tone => ({ ink: ansi(p.tones[tone].ink), border: null, fill: ansi(p.tones[tone].glass), glow: null })),
  pillText: ansi(p.pillText),
  raster,
  art: art256(p),
  panel: ansi(p.panel),
})

// The gh meter, CI bars and spinner in single palette colours: a ramp between two would be
// rounded cell by cell into colours of other hues.
const raster256 = (truecolour: RasterPaint, c: { green: number; red: number; amber: number; mauve: number; shine: number; spin: readonly [number, number]; track: number }): RasterPaint => ({
  green: [x256(c.green), x256(c.green)],
  red: [x256(c.red), x256(c.red)],
  amber: [x256(c.amber), x256(c.amber)],
  mauve: [x256(c.mauve), x256(c.mauve)],
  shine: x256(c.shine),
  // The header's wave keeps its gradient: one row of thin glyphs reads as a gradient still.
  rule: truecolour.rule,
  spinner: [x256(c.spin[0]), x256(c.spin[1])],
  track: x256(c.track),
  isPalette: true,
})

const TIDE_DARK_256: Paint256 = {
  light: 231,
  dark: 233,
  pillText: 233,
  panel: 234,
  tones: {
    success: { glass: 233, surface: 29, mid: 23, deep: 17, crest: 79, badge: 78, ink: 121, dim: [23, 23, 23] },
    error: { glass: 233, surface: 125, mid: 89, deep: 52, crest: 211, badge: 204, ink: 217, dim: [89, 89, 89] },
    warning: { glass: 233, surface: 136, mid: 94, deep: 52, crest: 222, badge: 214, ink: 222, dim: [94, 94, 94] },
    merged: { glass: 233, surface: 97, mid: 55, deep: 54, crest: 183, badge: 141, ink: 183, dim: [55, 55, 55] },
    subtle: { glass: 233, surface: 60, mid: 238, deep: 236, crest: 146, badge: 146, ink: 189, dim: [238, 238, 238] },
    suggestion: { glass: 233, surface: 25, mid: 24, deep: 17, crest: 117, badge: 75, ink: 117, dim: [24, 24, 24] },
  },
}

const TIDE_LIGHT_256: Paint256 = {
  light: 231,
  dark: 235,
  pillText: 231,
  panel: 255,
  tones: {
    success: { glass: 255, surface: 158, mid: 115, deep: 79, crest: 195, badge: 29, ink: 22, dim: [115, 115, 115] },
    error: { glass: 255, surface: 224, mid: 217, deep: 211, crest: 231, badge: 161, ink: 88, dim: [217, 217, 217] },
    warning: { glass: 255, surface: 223, mid: 222, deep: 179, crest: 230, badge: 130, ink: 94, dim: [222, 222, 222] },
    merged: { glass: 255, surface: 189, mid: 183, deep: 141, crest: 231, badge: 55, ink: 54, dim: [183, 183, 183] },
    subtle: { glass: 255, surface: 252, mid: 250, deep: 247, crest: 231, badge: 240, ink: 238, dim: [250, 250, 250] },
    suggestion: { glass: 255, surface: 153, mid: 117, deep: 75, crest: 195, badge: 25, ink: 18, dim: [117, 117, 117] },
  },
}

const AURORA_DARK_256: Paint256 = {
  light: 231,
  dark: 233,
  pillText: 233,
  panel: 234,
  tones: {
    success: { glass: 233, deep: 37, crest: 35, surface: 39, mid: 23, badge: 78, ink: 121, dim: [23, 22, 24] },
    error: { glass: 233, deep: 162, crest: 161, surface: 166, mid: 89, badge: 204, ink: 217, dim: [53, 52, 88] },
    warning: { glass: 233, deep: 172, crest: 178, surface: 168, mid: 94, badge: 214, ink: 222, dim: [94, 94, 95] },
    merged: { glass: 233, deep: 99, crest: 93, surface: 163, mid: 55, badge: 141, ink: 183, dim: [54, 55, 90] },
    subtle: { glass: 233, deep: 103, crest: 109, surface: 67, mid: 238, badge: 146, ink: 189, dim: [237, 238, 60] },
    suggestion: { glass: 233, deep: 33, crest: 39, surface: 99, mid: 24, badge: 75, ink: 117, dim: [17, 18, 54] },
  },
}

const AURORA_LIGHT_256: Paint256 = {
  light: 231,
  dark: 235,
  pillText: 231,
  panel: 255,
  tones: {
    success: { glass: 255, deep: 122, crest: 157, surface: 153, mid: 194, badge: 29, ink: 22, dim: [194, 194, 195] },
    error: { glass: 255, deep: 217, crest: 218, surface: 223, mid: 224, badge: 161, ink: 88, dim: [224, 225, 230] },
    warning: { glass: 255, deep: 222, crest: 223, surface: 218, mid: 230, badge: 130, ink: 94, dim: [230, 230, 225] },
    merged: { glass: 255, deep: 183, crest: 219, surface: 147, mid: 189, badge: 55, ink: 54, dim: [189, 225, 189] },
    subtle: { glass: 255, deep: 251, crest: 252, surface: 250, mid: 254, badge: 240, ink: 238, dim: [254, 254, 254] },
    suggestion: { glass: 255, deep: 153, crest: 159, surface: 183, mid: 195, badge: 25, ink: 18, dim: [195, 195, 189] },
  },
}

// Set here, under the palettes they are made of.
tide.ansi256 = {
  dark: variant256(TIDE_DARK_256, raster256(tideRaster('dark'), { green: 42, red: 204, amber: 214, mauve: 141, shine: 195, spin: [80, 117], track: 238 })),
  light: variant256(TIDE_LIGHT_256, raster256(tideRaster('light'), { green: 35, red: 161, amber: 172, mauve: 98, shine: 231, spin: [25, 31], track: 252 })),
}
aurora.ansi256 = {
  dark: variant256(AURORA_DARK_256, raster256(auroraRaster('dark'), { green: 42, red: 204, amber: 214, mauve: 170, shine: 195, spin: [43, 135], track: 237 })),
  light: variant256(AURORA_LIGHT_256, raster256(auroraRaster('light'), { green: 35, red: 161, amber: 172, mauve: 97, shine: 231, spin: [31, 61], track: 252 })),
}

export const THEME: Record<ThemeName, Theme> = { classic, solid, minimal, neon, retro, tide, aurora }

// ---- How many colours the terminal is painted with ---------------------------------------

// truecolor: the 24-bit colours as given. ansi256: rounded to the xterm palette's 256.
export type Depth = 'truecolor' | 'ansi256'

// What Claude Code itself decides from its environment (2.1.x): 24-bit where COLORTERM says
// so or the terminal is one known to take it, cut to 256 inside tmux unless
// CLAUDE_CODE_TMUX_TRUECOLOR is set; 256 wherever it is unsure, the paint that reads anywhere.
export type ColourEnv = { TMUX?: string; CLAUDE_CODE_TMUX_TRUECOLOR?: string; COLORTERM?: string; TERM?: string; TERM_PROGRAM?: string }
const TRUECOLOR_TERMS = new Set(['alacritty', 'contour', 'foot', 'ghostty', 'rio', 'wezterm', 'xterm-ghostty', 'xterm-kitty'])
const TRUECOLOR_PROGRAMS = new Set(['iTerm.app', 'vscode', 'WezTerm', 'ghostty'])
const isSet = (value: string | undefined) => value !== undefined && value !== ''

export function depthOf(env: ColourEnv): Depth {
  if (isSet(env.TMUX) && !isSet(env.CLAUDE_CODE_TMUX_TRUECOLOR)) return 'ansi256'
  const colorterm = (env.COLORTERM ?? '').toLowerCase()
  const isTrue = colorterm === 'truecolor' || colorterm === '24bit' || TRUECOLOR_TERMS.has(env.TERM ?? '') || TRUECOLOR_PROGRAMS.has(env.TERM_PROGRAM ?? '')

  return isTrue ? 'truecolor' : 'ansi256'
}

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

// A theme's look in a mode, in the terminal's colours: its 256-colour paint where it has one.
export const lookOf = (name: ThemeName, mode: Mode, depth: Depth = 'truecolor'): Look => {
  const theme = THEME[name]

  return { theme, variant: depth === 'ansi256' && theme.ansi256 !== undefined ? theme.ansi256[mode] : theme[mode] }
}

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
