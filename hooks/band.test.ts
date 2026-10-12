import { describe, expect, test } from 'claude-code/testing'

import type { TrackedPr } from '../types'
import { BAND_ROWS, actionsOf, buttonCells, byUrgency, clip, layoutOf, planOf, rowOf, stateOf } from './band'
import { derive, placeholder } from './pr'
import type { CheckItem, GhPr } from './pr'
import { cells } from './review'

const check = (name: string, status: string, conclusion = ''): CheckItem => ({ __typename: 'CheckRun', name, status, conclusion })
const passed = check('unit', 'COMPLETED', 'SUCCESS')
const failed = (name: string) => check(name, 'COMPLETED', 'FAILURE')
const threads = (open: number) => ({
  latestReviews: { totalCount: 1, nodes: [{ author: { __typename: 'User', login: 'alice' }, state: 'COMMENTED', submittedAt: '2026-10-07T10:00:00Z' }] },
  latestOpinionatedReviews: { nodes: [] },
  reviewRequests: { totalCount: 0, nodes: [] },
  reviewThreads: { totalCount: open, nodes: Array.from({ length: open }, () => ({ isResolved: false, isOutdated: false })) },
})

// A card as the pane holds it, from what GitHub says of #`number`.
function card(number: number, extra: Partial<GhPr> = {}, title = 'Retry uploads with exponential backoff'): TrackedPr {
  const ref = { url: `https://github.com/acme/app/pull/${number}`, repo: 'acme/app', number }
  const gh: GhPr = {
    ...ref,
    title,
    state: 'OPEN',
    isDraft: false,
    reviewDecision: 'APPROVED',
    mergeStateStatus: 'CLEAN',
    autoMergeRequest: null,
    statusCheckRollup: [passed],
    mergedAt: null,
    additions: 1,
    deletions: 0,
    headRefName: 'feat',
    baseRefName: 'main',
    author: { login: 'octocat' },
    ...extra,
  }

  return { ...placeholder(ref), title, state: gh.state, ...derive(gh) }
}

const conflicts = card(1, { mergeStateStatus: 'DIRTY' })
const ciFailed = card(2, { statusCheckRollup: [failed('build'), failed('lint'), passed] })
const changes = card(3, { reviewDecision: 'CHANGES_REQUESTED' })
const comments = card(4, { reviewDecision: 'REVIEW_REQUIRED', ...threads(3) })
const ready = card(5)
const running = card(6, { statusCheckRollup: [passed, check('e2e', 'IN_PROGRESS')] })
const inReview = card(7, { reviewDecision: 'REVIEW_REQUIRED', mergeStateStatus: 'BLOCKED' })
const merged = card(8, { state: 'MERGED', mergedAt: '2026-10-07T10:00:00Z' })
const closed = card(9, { state: 'CLOSED' })
const draft = card(10, { isDraft: true })

describe('the layout option', () => {
  test('band in any case is the band; anything else is the pane', async () => {
    expect(layoutOf('band')).toBe('band')
    expect(layoutOf(' Band ')).toBe('band')
    expect(layoutOf('BAND')).toBe('band')
    for (const other of ['pane', 'bands', '', 'top', 1, true, undefined, null]) expect(layoutOf(other)).toBe('pane')
  })
})

describe('order', () => {
  test('conflicts, then CI, then review, then what can merge, then what waits, then what is done', async () => {
    const watched = [closed, merged, draft, inReview, running, ready, comments, changes, ciFailed, conflicts]
    expect(byUrgency(watched).map(pr => pr.number)).toEqual([1, 2, 3, 4, 5, 6, 7, 10, 8, 9])
  })

  test('a tie keeps the order the PRs were watched in', async () => {
    const a = card(20, { statusCheckRollup: [failed('build')] })
    const b = card(21, { statusCheckRollup: [failed('lint')] })
    expect(byUrgency([b, a]).map(pr => pr.number)).toEqual([21, 20])
  })

  test("a PR GitHub won't show comes before the ones that only wait", async () => {
    const lost = { ...placeholder({ url: 'https://github.com/acme/app/pull/30', repo: 'acme/app', number: 30 }), error: 'Not found' }
    expect(byUrgency([running, lost, ready]).map(pr => pr.number)).toEqual([30, 5, 6])
  })
})

describe('states', () => {
  test('each line says its state in a few words', async () => {
    expect(stateOf(conflicts)).toBe('conflicts')
    expect(stateOf(ciFailed)).toBe('2 checks failing')
    expect(stateOf(card(11, { statusCheckRollup: [failed('build'), check('e2e', 'IN_PROGRESS')] }))).toBe('1 check failing')
    expect(stateOf(changes)).toBe('changes requested')
    expect(stateOf(comments)).toBe('3 comments to address')
    expect(stateOf(ready)).toBe('ready to merge')
    expect(stateOf(running)).toBe('CI running · 1/2')
    expect(stateOf(inReview)).toBe('waiting for review')
    expect(stateOf(card(12, { mergeStateStatus: 'BEHIND' }))).toBe('behind base')
    expect(stateOf(card(13, { autoMergeRequest: { mergeMethod: 'SQUASH' } }))).toBe('auto-merging')
    expect(stateOf(draft)).toBe('draft')
    expect(stateOf(merged)).toBe('merged')
    expect(stateOf(closed)).toBe('closed')
    expect(stateOf(placeholder({ url: 'https://github.com/acme/app/pull/14', repo: 'acme/app', number: 14 }))).toBe('loading…')
  })
})

describe('a line', () => {
  test('the title gives way to fit, and the line is never wider than the band', async () => {
    const row = rowOf(ciFailed, 40)
    expect(row).toEqual({ icon: '✗', number: '#2', title: 'Retry uploads w…', state: '2 checks failing' })
    for (const columns of [20, 30, 40, 60, 80, 120]) {
      for (const pr of [conflicts, ciFailed, comments, ready, running, merged]) {
        const { icon, number, title, state } = rowOf(pr, columns)
        const width = cells(icon) + 1 + cells(number) + 1 + (title === '' ? 0 : cells(title) + 3) + cells(state)
        expect(width).toBeLessThanOrEqual(columns)
      }
    }
  })

  test('a wide band shows the whole title', async () => {
    expect(rowOf(ready, 120).title).toBe('Retry uploads with exponential backoff')
  })

  test('too narrow for a useful title, the line keeps the state alone', async () => {
    expect(rowOf(ciFailed, 26)).toMatchObject({ title: '', state: '2 checks failing' })
    // A short title still shows where it fits whole.
    expect(rowOf(card(15, {}, 'Fix'), 26).title).toBe('Fix')
  })

  test('the comments icon is two cells wide, and counted so', async () => {
    const { icon, title, state } = rowOf(comments, 40)
    expect(icon).toBe('💬')
    expect(cells(icon) + 1 + 2 + 1 + cells(title) + 3 + cells(state)).toBeLessThanOrEqual(40)
  })

  test('a moving PR shows the spinner', async () => {
    expect(rowOf(running, 60).icon).toBe('◌')
  })

  test('clip cuts by cells, with an ellipsis', async () => {
    expect(clip('hello', 5)).toBe('hello')
    expect(clip('hello world', 6)).toBe('hello…')
    expect(clip('a 💬 b', 3)).toBe('a…')
    expect(clip('anything', 0)).toBe('')
  })
})

describe('rows', () => {
  test('nothing watched draws nothing', async () => {
    expect(planOf([], 20)).toEqual({ shown: [], more: 0, hasActions: false })
  })

  test('one line per PR and the action line, most urgent first', async () => {
    const plan = planOf([ready, ciFailed, running], 20)
    expect(plan.shown.map(pr => pr.number)).toEqual([2, 5, 6])
    expect(plan).toMatchObject({ more: 0, hasActions: true })
  })

  test('capped at five rows whatever the room: four PRs and the rest counted', async () => {
    const plan = planOf([closed, merged, draft, inReview, running, ready, comments, changes, ciFailed, conflicts], 40)
    expect(BAND_ROWS).toBe(5)
    expect(plan.shown.map(pr => pr.number)).toEqual([1, 2, 3, 4])
    expect(plan.more).toBe(6)
  })

  test('fewer rows to spare, fewer lines', async () => {
    expect(planOf([ready, ciFailed, running], 3)).toMatchObject({ more: 1, hasActions: true })
    expect(planOf([ready, ciFailed, running], 3).shown.map(pr => pr.number)).toEqual([2, 5])
    // One row: the most urgent PR alone, no buttons.
    expect(planOf([ready, ciFailed], 1)).toEqual({ shown: [ciFailed], more: 0, hasActions: false })
    expect(planOf([ready], 0).shown).toEqual([])
  })

  test('beside another plugin, two rows', async () => {
    expect(planOf([ready, ciFailed, running], 20, 2)).toMatchObject({ shown: [ciFailed], more: 2, hasActions: true })
  })
})

describe('the action line', () => {
  const fixed = buttonCells('o', 'Open pane') + 2 + buttonCells('r', 'Refresh')
  const notice = { long: '↑ pr-watch 0.11.0 available', short: '↑ 0.11.0' }
  const fix = buttonCells('f', 'Fix #2 with Claude')

  test('a plain button with a hotkey is drawn "o: Open pane"', async () => {
    expect(buttonCells('o', 'Open pane')).toBe('o: Open pane'.length)
  })

  test('with room, everything: the count, the hand-off, the long notice', async () => {
    expect(actionsOf(120, fixed, 3, fix, notice)).toEqual({ more: '+3 more', handoff: true, notice: notice.long })
  })

  test('narrower, the notice goes short, then goes; then the hand-off; the count stays longest', async () => {
    const all = fixed + 2 + 7 + 2 + fix
    expect(actionsOf(all + 2 + 8, fixed, 3, fix, notice).notice).toBe('↑ 0.11.0')
    expect(actionsOf(all + 2, fixed, 3, fix, notice)).toEqual({ more: '+3 more', handoff: true, notice: null })
    expect(actionsOf(fixed + 2 + 7, fixed, 3, fix, notice)).toEqual({ more: '+3 more', handoff: false, notice: null })
  })

  test('nothing more, no hand-off, no notice: just the buttons', async () => {
    expect(actionsOf(80, fixed, 0, null, null)).toEqual({ more: null, handoff: false, notice: null })
  })
})
