import { describe, expect, test } from 'claude-code/testing'

import type { Bucket, Limits } from '../types'
import {
  LOW_RATE,
  REST_MS,
  GH_HELP,
  bandNote,
  bucketOf,
  countdown,
  ghMeter,
  isRestDue,
  legacyRate,
  lowUntil,
  refillIn,
  restOf,
  roundWaitsUntil,
  toneOf,
  withSeen,
} from './rate'

const T = Date.parse('2026-10-07T12:00:00Z')
const RESET = Date.parse('2026-10-07T12:40:00Z')
const bucket = (remaining: number, extra: Partial<Bucket> = {}): Bucket => ({ limit: 5_000, remaining, resetAt: RESET, seenAt: T, ...extra })
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

describe('the gh meter', () => {
  test('dim while healthy, a warning below 20% of the limit, an error below 5%', async () => {
    expect([toneOf(bucket(1_000)), toneOf(bucket(999)), toneOf(bucket(250)), toneOf(bucket(249)), toneOf(bucket(0))]).toEqual([null, 'warning', 'warning', 'error', 'error'])
  })

  test('a wait counted down, and a refill rounded up to the minute', async () => {
    expect([countdown(45_000), countdown(44_001), countdown(120_000), countdown(90_000), countdown(-5)]).toEqual(['45s', '45s', '2m', '1m 30s', '0s'])
    expect([refillIn(45_000), refillIn(60_000), refillIn(2_460_000), refillIn(2_401_000), refillIn(3_590_000), refillIn(3_900_000), refillIn(-1)]).toEqual([
      '↻45s',
      '↻1m',
      '↻41m',
      '↻41m',
      '↻1h',
      '↻1h 5m',
      '↻0s',
    ])
  })

  test("a meter per login: the lower bucket's bar, its share and its refill", async () => {
    const two = limits([
      { login: 'octocat', graphql: bucket(4_321), core: bucket(4_900, { resetAt: RESET + 900_000 }) },
      { login: 'hubot', graphql: bucket(4_950), core: bucket(5_000) },
    ])
    expect(ghMeter(two, T)).toEqual({
      pause: null,
      meters: [
        { login: 'octocat', bar: { share: 4_321 / 5_000, tone: null }, percent: '86%', tone: null, refill: '↻40m' },
        { login: 'hubot', bar: { share: 0.99, tone: null }, percent: '99%', tone: null, refill: '↻40m' },
      ],
    })
  })

  test('the lower bucket sets the percent and its health, whichever of the two it is', async () => {
    const low = limits([{ login: 'octocat', graphql: bucket(4_000), core: bucket(200, { resetAt: RESET + 1_200_000 }) }])
    expect(ghMeter(low, T)?.meters[0]).toMatchObject({ percent: '4%', tone: 'error', refill: '↻1h', bar: { share: 0.04, tone: 'error' } })
    const warn = limits([{ login: 'octocat', graphql: bucket(800), core: bucket(5_000) }])
    expect(ghMeter(warn, T)?.meters[0]).toMatchObject({ percent: '16%', tone: 'warning', refill: '↻40m' })
  })

  test('a bucket past its reset reads full again, with no refill to say', async () => {
    const reset = limits([{ login: 'octocat', graphql: bucket(120, { resetAt: T - 1 }), core: bucket(10, { resetAt: T - 1 }) }])
    expect(ghMeter(reset, T)?.meters[0]).toEqual({ login: 'octocat', bar: { share: 1, tone: null }, percent: '100%', tone: null, refill: null })
  })

  test('only what is known: GraphQL alone before a round has asked REST; nothing known, no meter', async () => {
    expect(ghMeter(limits([{ login: 'octocat', graphql: bucket(4_000) }]), T)?.meters[0]).toMatchObject({ bar: { share: 0.8 }, percent: '80%' })
    expect(ghMeter(limits([]), T)).toBe(null)
  })

  test('while a secondary limit holds, the pause above the meters, counted down', async () => {
    const held = limits([{ login: 'octocat', graphql: bucket(4_000) }], T + 45_000)
    expect(ghMeter(held, T)).toMatchObject({ pause: '⏸ GitHub rate limit · resumes in 45s', meters: [{ login: 'octocat' }] })
    expect(ghMeter(held, T + 45_000)?.pause).toBe(null)
    expect(ghMeter(limits([], T + 45_000), T)).toEqual({ pause: '⏸ GitHub rate limit · resumes in 45s', meters: [] })
  })

  test('the `?` says it in two to four plain lines', async () => {
    expect(GH_HELP.length).toBeGreaterThanOrEqual(2)
    expect(GH_HELP.length).toBeLessThanOrEqual(4)
    const all = GH_HELP.join(' ')
    for (const word of ['hourly', 'GraphQL', 'REST', 'per login', 'every session', 'stops polling', '⏸', '↻']) expect(all).toContain(word)
  })
})

describe("the band's gh note", () => {
  test('nothing while every bucket has 20% or more', async () => {
    expect(bandNote(limits([{ login: 'octocat', graphql: bucket(1_000), core: bucket(5_000) }]), T)).toBe(null)
    expect(bandNote(limits([]), T)).toBe(null)
  })

  test("below 20%, the lowest bucket's share and refill, whichever login it is, in the meter's words", async () => {
    const low = limits([
      { login: 'octocat', graphql: bucket(4_000) },
      { login: 'hubot', graphql: bucket(600), core: bucket(200) },
    ])
    expect(bandNote(low, T)).toEqual({ long: 'gh 4% ↻40m', short: 'gh 4%', tone: 'error', isPause: false })
  })

  test('the pause, whatever the buckets', async () => {
    expect(bandNote(limits([], T + 45_000), T)).toEqual({
      long: '⏸ GitHub rate limit · resumes in 45s',
      short: '⏸ resumes in 45s',
      tone: 'warning',
      isPause: true,
    })
  })
})
