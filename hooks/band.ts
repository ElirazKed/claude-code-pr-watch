// The band layout: a few lines above the prompt instead of the pane. Which PRs get a line,
// in what order, what each line says, and what fits on the action line under them.
import type { TrackedPr } from '../types'
import { PILL } from './pr'
import type { PillKind } from './pr'
import { cells } from './review'

export type Layout = 'pane' | 'band'

// The `layout` option: `band`, in any case, or the pane.
export const layoutOf = (value: unknown): Layout => (typeof value === 'string' && value.trim().toLowerCase() === 'band' ? 'band' : 'pane')

// The band stays short whatever room the bottom slot has, the action line included; and
// shorter still beside another plugin's lines.
export const BAND_ROWS = 5
export const SHARED_ROWS = 2

// Most urgent first: what blocks the merge (conflicts, then CI, then review), what the person
// can act on now, then what waits on others, then what is done.
const URGENCY: readonly PillKind[] = [
  'conflicts',
  'ciFailed',
  'ciFailing',
  'changes',
  'comments',
  'ready',
  'behind',
  'blocked',
  'ciRunning',
  'inReview',
  'checking',
  'autoMerging',
  'draft',
  'loading',
  'merged',
  'closed',
]

const KIND = new Map(Object.entries(PILL).map(([kind, pill]) => [pill as string, kind as PillKind]))

// A row kept by an older version of this mod may carry a pill no longer said: it goes last
// but one, before the finished ones.
const kindOf = (pr: TrackedPr): PillKind | null => KIND.get(pr.pill) ?? null

function rank(pr: TrackedPr): number {
  const kind = kindOf(pr)
  // A PR GitHub won't show is worth a look before the ones that only wait.
  if (pr.error !== null && pr.state === 'UNKNOWN') return URGENCY.indexOf('ready') - 0.5
  if (kind === null) return URGENCY.indexOf('merged') - 0.5

  return URGENCY.indexOf(kind)
}

// The watched PRs, most urgent first; a tie keeps the order they were watched in.
export function byUrgency(list: readonly TrackedPr[]): TrackedPr[] {
  return list
    .map((pr, i) => ({ pr, i, rank: rank(pr) }))
    .sort((a, b) => a.rank - b.rank || a.i - b.i)
    .map(entry => entry.pr)
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`

// A line's short state: "2 checks failing", "changes requested", "3 comments to address".
export function stateOf(pr: TrackedPr): string {
  switch (kindOf(pr)) {
    case 'conflicts':
      return 'conflicts'
    case 'ciFailed':
    case 'ciFailing':
      return `${plural(pr.checks.failed, 'check')} failing`
    case 'ciRunning':
      return `CI running · ${pr.checks.passed}/${pr.checks.total}`
    case 'changes':
      return 'changes requested'
    // "3 comments to address" is the headline already.
    case 'comments':
      return pr.headline
    case 'inReview':
      return pr.headline.includes('approval') ? 'waiting for approval' : 'waiting for review'
    case 'ready':
      return 'ready to merge'
    case 'behind':
      return 'behind base'
    case 'blocked':
      return 'blocked'
    case 'checking':
      return 'checking mergeability'
    case 'autoMerging':
      return 'auto-merging'
    case 'draft':
      return 'draft'
    case 'loading':
      return pr.error !== null ? "couldn't read it" : 'loading…'
    case 'merged':
      return 'merged'
    case 'closed':
      return 'closed'
    case null:
      return pr.headline.toLowerCase()
  }
}

// `text` cut to `width` cells, an ellipsis standing for what's cut.
export function clip(text: string, width: number): string {
  if (cells(text) <= width) return text
  if (width <= 0) return ''
  let out = ''
  for (const ch of text) {
    if (cells(out + ch) > width - 1) break
    out += ch
  }

  return `${out.trimEnd()}…`
}

// A title narrower than this says nothing: the line drops it and keeps the state.
const MIN_TITLE = 8
const SEP = ' · '

export type Row = { icon: string; number: string; title: string; state: string }

// One PR's line, to fit `columns` cells and never wrap: "✗ #13 Retry uploads… · 2 checks
// failing". The title gives way first, then the state.
export function rowOf(pr: TrackedPr, columns: number): Row {
  // The card's icon; a spinner (one cell) while it moves.
  const icon = pr.isMoving ? '◌' : ([...pr.pill][0] ?? '●')
  const number = `#${pr.number}`
  const state = stateOf(pr)
  const lead = cells(icon) + 1 + cells(number) + 1
  const room = columns - lead - cells(SEP) - cells(state)
  const title = pr.title.trim()
  if (title !== '' && room >= Math.min(MIN_TITLE, cells(title))) return { icon, number, title: clip(title, room), state }

  return { icon, number, title: '', state: clip(state, columns - lead) }
}

export type Plan = { shown: TrackedPr[]; more: number; hasActions: boolean }

// Which PRs get a line: as many as the rows allow (`maxRows`, the band's cap), the action line
// last; the rest are counted as "+N more" there. Nothing watched, nothing drawn.
export function planOf(list: readonly TrackedPr[], maxRows: number, cap = BAND_ROWS): Plan {
  const rows = Math.min(cap, Math.floor(maxRows))
  if (list.length === 0 || rows <= 0) return { shown: [], more: 0, hasActions: false }
  const sorted = byUrgency(list)
  // One row: the most urgent PR alone.
  if (rows === 1) return { shown: sorted.slice(0, 1), more: 0, hasActions: false }
  const shown = sorted.slice(0, rows - 1)

  return { shown, more: sorted.length - shown.length, hasActions: true }
}

// A plain Button with a hotkey draws as "o: Open pane".
export const buttonCells = (hotkey: string, label: string) => cells(hotkey) + 2 + cells(label)

const GAP = 2

export type Actions = { more: string | null; handoff: boolean; notice: string | null; gh: string | null }

// What the action line holds beside its two buttons (`fixed`, their cells), to fit `columns`:
// "+N more", then the hand-off button (`handoff`, its cells), then the update notice in its
// long form or its short one, then the gh note (rate.ts) the same way. What doesn't fit is
// left out, in the reverse order; a pause, though, comes right after the count.
export function actionsOf(
  columns: number,
  fixed: number,
  more: number,
  handoff: number | null,
  notice: { long: string; short: string } | null,
  gh: { long: string; short: string; isPause: boolean } | null = null,
): Actions {
  let used = fixed
  const fits = (width: number) => used + GAP + width <= columns
  const take = (width: number) => (used += GAP + width)
  const pick = (text: { long: string; short: string }) => {
    const said = [text.long, text.short].find(t => fits(cells(t))) ?? null
    if (said !== null) take(cells(said))

    return said
  }
  const out: Actions = { more: null, handoff: false, notice: null, gh: null }
  const tail = more > 0 ? `+${more} more` : null
  if (tail !== null && fits(cells(tail))) {
    out.more = tail
    take(cells(tail))
  }
  if (gh?.isPause === true) out.gh = pick(gh)
  if (handoff !== null && fits(handoff)) {
    out.handoff = true
    take(handoff)
  }
  if (notice !== null) out.notice = pick(notice)
  if (gh !== null && !gh.isPause) out.gh = pick(gh)

  return out
}
