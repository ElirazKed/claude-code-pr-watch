import { describe, expect, test } from 'claude-code/testing'

import { KEEP_MS, NOT_FOUND, QUIET_MS, UNREACHABLE_MS, failedEntry, isMiss, isUnreachable, keptEntries } from './cache'
import type { Entry } from './cache'
import type { GhPr } from './pr'

const pr = { number: 7, title: 'Retry uploads', state: 'OPEN' } as GhPr
const T = Date.parse('2026-10-07T12:00:00Z')

describe('a failed fetch', () => {
  test('a PR with data keeps it, and its time, and says nothing at first', async () => {
    const entry = failedEntry({ at: T, pr }, 'gh: HTTP 502', T + 20_000)
    expect(entry).toEqual({ at: T, pr, failedSince: T + 20_000 })
  })

  test('the error shows once the failures have lasted QUIET_MS, counted from the first', async () => {
    let entry: Entry = { at: T, pr }
    entry = failedEntry(entry, 'gh: HTTP 502', T + 60_000)
    entry = failedEntry(entry, 'gh: HTTP 504', T + 60_000 + QUIET_MS - 1)
    expect(entry.error).toBe(undefined)
    entry = failedEntry(entry, 'gh: HTTP 504', T + 60_000 + QUIET_MS)
    expect(entry).toEqual({ at: T, pr, failedSince: T + 60_000, error: 'gh: HTTP 504' })
  })

  test('a PR never read says so at once, as before', async () => {
    expect(failedEntry(undefined, 'gh: HTTP 502', T)).toEqual({ at: T, error: 'gh: HTTP 502' })
    expect(failedEntry({ at: T - 60_000, error: 'gh: HTTP 502' }, 'Not found, or no gh account can see it', T)).toEqual({
      at: T,
      error: 'Not found, or no gh account can see it',
    })
  })

  test("an older version's failed entry (an error, no failedSince) counts from its time", async () => {
    const older = { at: T, pr, error: 'gh: HTTP 502' }
    expect(failedEntry(older, 'gh: HTTP 502', T + 60_000)).toEqual({ at: T, pr, failedSince: T })
    expect(failedEntry(older, 'gh: HTTP 502', T + QUIET_MS)).toEqual({ at: T, pr, failedSince: T, error: 'gh: HTTP 502' })
  })
})

describe('a PR no account can see', () => {
  test('is flagged, and a round leaves it out for UNREACHABLE_MS', async () => {
    const entry = failedEntry(undefined, NOT_FOUND, T, true)
    expect(entry).toEqual({ at: T, error: NOT_FOUND, isUnreachable: true })
    expect(isUnreachable(entry, T + UNREACHABLE_MS - 1)).toBe(true)
    expect(isUnreachable(entry, T + UNREACHABLE_MS)).toBe(false)
  })

  test('a first fetch that failed on the way (a 502, a hold) is no miss: the next round asks', async () => {
    expect(isUnreachable(failedEntry(undefined, 'gh: HTTP 502', T), T + 10_000)).toBe(false)
    expect(isUnreachable(undefined, T)).toBe(false)
  })

  test('a miss, however old, is one never read that no account could see: its card offers Try again', async () => {
    expect(isMiss(failedEntry(undefined, 'Not found as octocat or hubot', T, true))).toBe(true)
    expect(isMiss({ at: T - 10 * UNREACHABLE_MS, error: NOT_FOUND })).toBe(true)
    expect(isMiss({ at: T, error: 'gh: HTTP 502' })).toBe(false)
    expect(isMiss({ at: T, pr, error: NOT_FOUND })).toBe(false)
    expect(isMiss(undefined)).toBe(false)
  })

  test("an older version's miss has the words and no flag; one with data is never left out", async () => {
    expect(isUnreachable({ at: T, error: NOT_FOUND }, T + 10_000)).toBe(true)
    expect(isUnreachable({ at: T, pr, error: NOT_FOUND }, T + 10_000)).toBe(false)
    // Read at last: the flag goes with the miss.
    expect(failedEntry({ at: T, pr }, NOT_FOUND, T + 10_000, true)).toEqual({ at: T, pr, failedSince: T + 10_000 })
  })
})

describe('what a round keeps', () => {
  test('every live PR however old, and the rest until KEEP_MS old', async () => {
    const at = T + 10 * KEEP_MS
    const prs: Record<string, Entry> = {
      live: { at: at - 3 * KEEP_MS, pr },
      recent: { at: at - KEEP_MS + 1, pr },
      dead: { at: at - KEEP_MS, pr },
      unreachable: { at: at - 2 * KEEP_MS, error: 'Not found, or no gh account can see it' },
    }
    expect(Object.keys(keptEntries(prs, new Set(['live', 'elsewhere']), at)).sort()).toEqual(['live', 'recent'])
    expect(keptEntries({}, new Set(['live']), at)).toEqual({})
  })
})
