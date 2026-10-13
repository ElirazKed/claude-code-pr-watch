// The gh meter: how much of GitHub's hourly budget each gh login has left, so the person can
// put off heavy gh work before it runs out, and what holds the poll back. GraphQL counts
// points (the poll's own replies say how many are left, for free); REST counts requests
// (GitHub's /rate_limit, which costs nothing, asked by the round at most every REST_MS per
// login). Secondary limits, on bursts, have no counter: the line never says they're fine,
// only that one holds the poll once it does.
import type { Bucket, Limits, LoginRates } from '../types'
import type { RateLimit } from './batch'

// By login, as gh names it.
export type Rates = Record<string, LoginRates>

// A login with fewer points than this left is not polled until its reset.
export const LOW_RATE = 300
// How often a round asks /rate_limit for each login.
export const REST_MS = 300_000
// Below these shares of its limit a bucket shows as a warning, then as an error.
export const WARN_SHARE = 0.2
export const ERROR_SHARE = 0.05
// GitHub's hourly limit for a person's token, where a reply doesn't say.
const DEFAULT_LIMIT = 5_000

// ---- What GitHub said ---------------------------------------------------------------------

// A bucket from a reply's headers or its rateLimit field, seen at `at`.
export function bucketOf(rate: RateLimit | null, at: number): Bucket | null {
  if (rate === null) return null
  const resetAt = Date.parse(rate.resetAt)
  if (!Number.isFinite(resetAt) || !Number.isFinite(rate.remaining)) return null

  return { limit: rate.limit ?? DEFAULT_LIMIT, remaining: rate.remaining, resetAt, seenAt: at }
}

type RestBucket = { limit?: unknown; remaining?: unknown; reset?: unknown }

// `gh api rate_limit`'s reply: resources.core (REST's requests) and resources.graphql (the
// points), each reset in epoch seconds. Null where it says neither.
export function restOf(stdout: string, at: number): Pick<LoginRates, 'core' | 'graphql'> | null {
  let reply: { resources?: Record<string, RestBucket | undefined> } | null
  try {
    reply = JSON.parse(stdout) as typeof reply
  } catch {
    return null
  }
  const one = (b: RestBucket | undefined): Bucket | undefined =>
    typeof b?.limit === 'number' && typeof b.remaining === 'number' && typeof b.reset === 'number'
      ? { limit: b.limit, remaining: b.remaining, resetAt: b.reset * 1_000, seenAt: at }
      : undefined
  const core = one(reply?.resources?.core)
  const graphql = one(reply?.resources?.graphql)
  if (core === undefined && graphql === undefined) return null

  return { ...(core !== undefined && { core }), ...(graphql !== undefined && { graphql }) }
}

// The later of two sightings of a bucket.
const later = (a: Bucket | undefined, b: Bucket | undefined) => (a === undefined ? b : b === undefined || a.seenAt >= b.seenAt ? a : b)

// `rates` with what was just seen of `login` folded in: the later sighting of each bucket,
// and when /rate_limit was last asked.
export function withSeen(rates: Rates, login: string, seen: LoginRates): Rates {
  const was = rates[login] ?? {}
  const graphql = later(seen.graphql, was.graphql)
  const core = later(seen.core, was.core)
  const askedAt = seen.askedAt === undefined || (was.askedAt ?? 0) > seen.askedAt ? was.askedAt : seen.askedAt

  return {
    ...rates,
    [login]: { ...(graphql !== undefined && { graphql }), ...(core !== undefined && { core }), ...(askedAt !== undefined && { askedAt }) },
  }
}

// Whether a round asks /rate_limit for `login` now: REST_MS after it last did, answered or not.
export const isRestDue = (rates: Rates, login: string, at: number) => at - (rates[login]?.askedAt ?? 0) >= REST_MS

// ---- Backing off, per login ---------------------------------------------------------------

// Until when `login` is left out of the poll: while its points are low, until their reset.
// Null: it isn't.
export function lowUntil(rates: Rates, login: string, at: number): number | null {
  const points = rates[login]?.graphql

  return points !== undefined && points.remaining < LOW_RATE && points.resetAt > at ? points.resetAt : null
}

// Until when the whole round waits: only once every login it needs is low, and then until
// the first of them resets. Null: some login still has points, so the round runs for it.
export function roundWaitsUntil(rates: Rates, logins: readonly string[], at: number): number | null {
  const resets = logins.map(login => lowUntil(rates, login, at))
  if (resets.length === 0 || resets.some(reset => reset === null)) return null

  return Math.min(...resets.map(reset => reset ?? 0))
}

// The `rate` field older versions read: the login with the most points left, so their
// backoff, like this one's, waits only once every login is low.
export function legacyRate(rates: Rates): RateLimit | null {
  const best = Object.values(rates)
    .flatMap(rate => (rate.graphql !== undefined ? [rate.graphql] : []))
    .sort((a, b) => b.remaining - a.remaining)[0]

  return best === undefined ? null : { limit: best.limit, remaining: best.remaining, resetAt: new Date(best.resetAt).toISOString() }
}

// ---- The meter -----------------------------------------------------------------------------

export type Tone = 'warning' | 'error' | null

// A bucket as it stands at `at`: past its reset, GitHub has filled it again.
export const current = (bucket: Bucket, at: number): Bucket => (at >= bucket.resetAt ? { ...bucket, remaining: bucket.limit } : bucket)

export const shareOf = (bucket: Bucket) => (bucket.limit > 0 ? bucket.remaining / bucket.limit : 1)

// Healthy (null) is dim; below WARN_SHARE a warning, below ERROR_SHARE an error.
export function toneOf(bucket: Bucket): Tone {
  const share = shareOf(bucket)

  return share < ERROR_SHARE ? 'error' : share < WARN_SHARE ? 'warning' : null
}

// "45s", "2m", "1m 30s": a wait, rounded up to the second.
export function countdown(ms: number): string {
  const s = Math.max(0, Math.ceil(ms / 1_000))
  if (s < 60) return `${s}s`
  const m = Math.floor(s / 60)

  return s % 60 === 0 ? `${m}m` : `${m}m ${s % 60}s`
}

// "↻42m": how long until GitHub fills a bucket again, rounded up; "↻1h 5m" past the hour.
export function refillIn(ms: number): string {
  const s = Math.max(0, Math.ceil(ms / 1_000))
  if (s < 60) return `↻${s}s`
  const m = Math.ceil(s / 60)

  return m < 60 ? `↻${m}m` : m % 60 === 0 ? `↻${m / 60}h` : `↻${Math.floor(m / 60)}h ${m % 60}m`
}

export const pauseText = (heldUntil: number, at: number) => `⏸ GitHub rate limit · resumes in ${countdown(heldUntil - at)}`

type Shown = { login: string; graphql?: Bucket; core?: Bucket }

// The logins with a bucket known, each bucket as it stands now.
function shownOf(limits: Limits, at: number): Shown[] {
  return limits.logins.flatMap(rate => {
    const graphql = rate.graphql && current(rate.graphql, at)
    const core = rate.core && current(rate.core, at)

    return graphql !== undefined || core !== undefined ? [{ login: rate.login, graphql, core }] : []
  })
}

// The bucket with the least share left that has a reset ahead (one past it is full again).
function lowestIn(buckets: readonly (Bucket | undefined)[], at: number): Bucket | null {
  let low: Bucket | null = null
  for (const bucket of buckets) {
    if (bucket === undefined || bucket.resetAt <= at) continue
    if (low === null || shareOf(bucket) < shareOf(low)) low = bucket
  }

  return low
}

// One bar of a meter: the share a bucket has left, and how healthy that is.
export type Bar = { share: number; tone: Tone }
// One login's meter: GraphQL's bar (the poll's points) and REST's (gh's requests), each null
// until seen (the pane draws the lower); the lower share as "94%", its tone, and when that bucket refills ("↻42m"), null
// where neither has a reset ahead (both full).
export type Meter = { login: string; graphql: Bar | null; core: Bar | null; percent: string; tone: Tone; refill: string | null }
// The pane's gh meter: the hold, while a secondary limit pauses the poll, and a meter per
// login, the active one first.
export type GhMeter = { pause: string | null; meters: Meter[] }

const barOf = (bucket: Bucket | undefined): Bar | null => (bucket === undefined ? null : { share: shareOf(bucket), tone: toneOf(bucket) })

// The pane's gh meter at `at`; null with nothing known and nothing held.
export function ghMeter(limits: Limits, at: number): GhMeter | null {
  const pause = limits.heldUntil > at ? pauseText(limits.heldUntil, at) : null
  const meters = shownOf(limits, at).map((rate): Meter => {
    const known = [rate.graphql, rate.core].filter((b): b is Bucket => b !== undefined)
    const least = known.reduce((a, b) => (shareOf(b) < shareOf(a) ? b : a))
    const low = lowestIn(known, at)

    return {
      login: rate.login,
      graphql: barOf(rate.graphql),
      core: barOf(rate.core),
      percent: `${Math.floor(shareOf(least) * 100)}%`,
      tone: toneOf(least),
      refill: low === null ? null : refillIn(low.resetAt - at),
    }
  })

  return pause === null && meters.length === 0 ? null : { pause, meters }
}

// The `?` under the meter: what the budget is, in plain words.
export const GH_HELP = [
  "GitHub's hourly budget per gh login, a bar each: what is left of its GraphQL points (the poll's) or its REST requests (gh's), whichever is lower.",
  'It is per login, so every session on this machine signed in as it shares it.',
  "Running low, pr-watch stops polling that login's PRs; ⏸ means GitHub asked everyone to wait.",
  '↻ is how long until it refills.',
] as const

export type GhNote = { long: string; short: string; tone: Tone; isPause: boolean }

// The band's gh note, for its action line, in the meter's words: the pause while a hold lasts;
// otherwise only while some bucket is below WARN_SHARE, the share it has left, "gh 12% ↻42m",
// or short "gh 12%". Healthy: nothing.
export function bandNote(limits: Limits, at: number): GhNote | null {
  if (limits.heldUntil > at) {
    return { long: pauseText(limits.heldUntil, at), short: `⏸ resumes in ${countdown(limits.heldUntil - at)}`, tone: 'warning', isPause: true }
  }
  const low = lowestIn(shownOf(limits, at).flatMap(rate => [rate.graphql, rate.core]), at)
  if (low === null || shareOf(low) >= WARN_SHARE) return null
  const share = `gh ${Math.floor(shareOf(low) * 100)}%`

  return { long: `${share} ${refillIn(low.resetAt - at)}`, short: share, tone: toneOf(low), isPause: false }
}
