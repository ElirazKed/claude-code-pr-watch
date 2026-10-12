import { describe, expect, test } from 'claude-code/testing'

import type { Bucket, Limits } from '../types'
import {
  LOW_RATE,
  REST_MS,
  bandNote,
  bucketOf,
  clockAt,
  countdown,
  ghLines,
  isRestDue,
  kilo,
  legacyRate,
  lowUntil,
  offsetOf,
  restOf,
  roundWaitsUntil,
  toneOf,
  withSeen,
} from './rate'
import type { GhLine } from './rate'

const T = Date.parse('2026-10-07T12:00:00Z')
const RESET = Date.parse('2026-10-07T12:40:00Z')
// Jerusalem in summer: 12:40Z reads 15:40.
const ZONE = 180
const bucket = (remaining: number, extra: Partial<Bucket> = {}): Bucket => ({ limit: 5_000, remaining, resetAt: RESET, seenAt: T, ...extra })
const text = (line: GhLine) => line.map(part => part.text).join('')
const limits = (logins: Limits['logins'], heldUntil = 0): Limits => ({ logins, heldUntil })

describe('what GitHub said', () => {
  test('a reply\'s rate, as a bucket; GitHub\'s usual 5,000 where it names no limit', async () => {
    expect(bucketOf({ limit: 5_000, remaining: 4_321, resetAt: '2026-10-07T12:40:00Z' }, T)).toEqual(bucket(4_321))
    expect(bucketOf({ remaining: 12, resetAt: '2026-10-07T12:40:00Z' }, T)).toEqual(bucket(12))
    expect(bucketOf({ remaining: 12, resetAt: 'soon' }, T)).toBe(null)
    expect(bucketOf(null, T)).toBe(null)
  })

  test("/rate_limit's REST and GraphQL buckets, reset in epoch seconds", async () => {
    const resources = {
      core: { limit: 5_000, used: 100, remaining: 4_900, reset: RESET / 1_000 },
      graphql: { limit: 5_000, used: 5, remaining: 4_995, reset: RESET / 1_000 },
      search: { limit: 30, used: 0, remaining: 30, reset: RESET / 1_000 },
    }
    expect(restOf(JSON.stringify({ resources, rate: resources.core }), T)).toEqual({ core: bucket(4_900), graphql: bucket(4_995) })
    expect(restOf(JSON.stringify({ resources: { core: resources.core } }), T)).toEqual({ core: bucket(4_900) })
    expect(restOf('gh: Bad credentials (HTTP 401)', T)).toBe(null)
    expect(restOf('{"message":"Not Found"}', T)).toBe(null)
  })

  test("each bucket keeps its latest sighting, and the latest /rate_limit ask, whoever wrote them", async () => {
    let rates = withSeen({}, 'octocat', { graphql: bucket(4_000, { seenAt: T + 10 }) })
    rates = withSeen(rates, 'octocat', { graphql: bucket(4_500, { seenAt: T }), core: bucket(4_900), askedAt: T })
    expect(rates).toEqual({ octocat: { graphql: bucket(4_000, { seenAt: T + 10 }), core: bucket(4_900), askedAt: T } })
    rates = withSeen(rates, 'octocat', { askedAt: T - 1 })
    expect(rates.octocat?.askedAt).toBe(T)
    expect(Object.keys(withSeen(rates, 'hubot', { core: bucket(1) }))).toEqual(['octocat', 'hubot'])
  })

  test('a round asks /rate_limit for a login at most every REST_MS, answered or not', async () => {
    expect(REST_MS).toBe(300_000)
    expect(isRestDue({}, 'octocat', T)).toBe(true)
    const rates = { octocat: { askedAt: T } }
    expect(isRestDue(rates, 'octocat', T + REST_MS - 1)).toBe(false)
    expect(isRestDue(rates, 'octocat', T + REST_MS)).toBe(true)
    expect(isRestDue(rates, 'hubot', T + 1)).toBe(true)
  })
})

describe('backing off, per login', () => {
  const rates = { octocat: { graphql: bucket(4_000) }, hubot: { graphql: bucket(LOW_RATE - 1, { resetAt: RESET + 600_000 }) } }

  test('a login low on points is left out until its reset, and only it', async () => {
    expect(lowUntil(rates, 'hubot', T)).toBe(RESET + 600_000)
    expect(lowUntil(rates, 'hubot', RESET + 600_000)).toBe(null)
    expect(lowUntil(rates, 'octocat', T)).toBe(null)
    expect(lowUntil(rates, 'nobody', T)).toBe(null)
    expect(lowUntil({ octocat: { graphql: bucket(LOW_RATE) } }, 'octocat', T)).toBe(null)
  })

  test('the round itself waits only once every login it needs is low, for the first reset', async () => {
    expect(roundWaitsUntil(rates, ['octocat', 'hubot'], T)).toBe(null)
    expect(roundWaitsUntil(rates, ['hubot'], T)).toBe(RESET + 600_000)
    const both = { ...rates, octocat: { graphql: bucket(0) } }
    expect(roundWaitsUntil(both, ['octocat', 'hubot'], T)).toBe(RESET)
    expect(roundWaitsUntil(both, [], T)).toBe(null)
  })

  test("older versions read `rate` as the login with the most points left", async () => {
    expect(legacyRate(rates)).toEqual({ limit: 5_000, remaining: 4_000, resetAt: '2026-10-07T12:40:00.000Z' })
    expect(legacyRate({ hubot: { core: bucket(1) } })).toBe(null)
  })
})

describe('the gh line', () => {
  test('thousands rounded down to a tenth: never more left than there is', async () => {
    expect([kilo(4_321), kilo(5_000), kilo(4_999), kilo(1_000), kilo(999), kilo(0), kilo(12_500)]).toEqual(['4.3k', '5k', '4.9k', '1k', '999', '0', '12.5k'])
  })

  test('dim while healthy, a warning below 20% of the limit, an error below 5%', async () => {
    expect([toneOf(bucket(1_000)), toneOf(bucket(999)), toneOf(bucket(250)), toneOf(bucket(249)), toneOf(bucket(0))]).toEqual([null, 'warning', 'warning', 'error', 'error'])
  })

  test('the reset in the local time, and a wait counted down', async () => {
    expect([clockAt(RESET, ZONE), clockAt(RESET, 0), clockAt(RESET, -330)]).toEqual(['15:40', '12:40', '07:10'])
    expect([offsetOf('+0300\n'), offsetOf('-0530'), offsetOf('+0000'), offsetOf('IDT')]).toEqual([180, -330, 0, null])
    expect([countdown(45_000), countdown(44_001), countdown(120_000), countdown(90_000), countdown(-5)]).toEqual(['45s', '45s', '2m', '1m 30s', '0s'])
  })

  test('a line per login: points for GraphQL, requests for REST, and the lowest bucket\'s reset', async () => {
    const two = limits([
      { login: 'octocat', graphql: bucket(4_321), core: bucket(4_900, { resetAt: RESET + 900_000 }) },
      { login: 'hubot', graphql: bucket(4_950), core: bucket(5_000) },
    ])
    expect(ghLines(two, T, 60, ZONE).map(text)).toEqual(['gh  octocat 4.3k/5k GraphQL · 4.9k/5k REST · resets 15:40', '    hubot 4.9k/5k GraphQL · 5k/5k REST'])
    // Healthy: every part dim.
    expect(ghLines(two, T, 60, ZONE).flat().every(part => part.tone === null)).toBe(true)
  })

  test('where the reset does not fit beside its login, it goes on a line of its own', async () => {
    const two = limits([
      { login: 'octocat', graphql: bucket(4_321), core: bucket(4_900) },
      { login: 'hubot', graphql: bucket(800), core: bucket(5_000) },
    ])
    const lines = ghLines(two, T, 46, ZONE)
    expect(lines.map(text)).toEqual(['gh  octocat 4.3k/5k GraphQL · 4.9k/5k REST', '    hubot 800/5k GraphQL · 5k/5k REST', '    resets 15:40'])
    for (const line of lines) expect(text(line).length).toBeLessThanOrEqual(46)
    // The low bucket and its reset say so; the rest stays dim.
    expect(lines.flat().filter(part => part.tone !== null)).toEqual([
      { text: '800/5k GraphQL', tone: 'warning' },
      { text: 'resets 15:40', tone: 'warning' },
    ])
  })

  test('a bucket under 5% is an error; one past its reset reads full again, with no reset to say', async () => {
    const spent = limits([{ login: 'octocat', graphql: bucket(120), core: bucket(10, { resetAt: T - 1 }) }])
    expect(ghLines(spent, T, 60, ZONE).map(text)).toEqual(['gh  octocat 120/5k GraphQL · 5k/5k REST · resets 15:40'])
    expect(ghLines(spent, T, 60, ZONE)[0]?.find(part => part.text.includes('GraphQL'))?.tone).toBe('error')
  })

  test('only what is known: GraphQL alone before a round has asked REST; nothing known, no line', async () => {
    expect(ghLines(limits([{ login: 'octocat', graphql: bucket(4_000) }]), T, 60, ZONE).map(text)).toEqual(['gh  octocat 4k/5k GraphQL · resets 15:40'])
    expect(ghLines(limits([]), T, 60, ZONE)).toEqual([])
  })

  test('while a secondary limit holds, the pause line instead, counted down', async () => {
    const held = limits([{ login: 'octocat', graphql: bucket(4_000) }], T + 45_000)
    expect(ghLines(held, T, 46, ZONE)).toEqual([[{ text: '⏸ GitHub rate limit · polling resumes in 45s', tone: 'warning' }]])
    expect(ghLines(held, T + 45_000, 46, ZONE).map(text)).toEqual(['gh  octocat 4k/5k GraphQL · resets 15:40'])
  })
})

describe("the band's gh note", () => {
  test('nothing while every bucket has 20% or more', async () => {
    expect(bandNote(limits([{ login: 'octocat', graphql: bucket(1_000), core: bucket(5_000) }]), T, ZONE)).toBe(null)
    expect(bandNote(limits([]), T, ZONE)).toBe(null)
  })

  test("below 20%, the lowest bucket's share and reset, whichever login it is", async () => {
    const low = limits([
      { login: 'octocat', graphql: bucket(4_000) },
      { login: 'hubot', graphql: bucket(600), core: bucket(200) },
    ])
    expect(bandNote(low, T, ZONE)).toEqual({ long: 'gh 4% · resets 15:40', short: 'gh 4%', tone: 'error', isPause: false })
  })

  test('the pause, whatever the buckets', async () => {
    expect(bandNote(limits([], T + 45_000), T, ZONE)).toEqual({
      long: '⏸ GitHub rate limit · polling resumes in 45s',
      short: '⏸ resumes in 45s',
      tone: 'warning',
      isPause: true,
    })
  })
})
