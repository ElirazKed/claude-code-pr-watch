// The gh line: how much of GitHub's hourly budget each gh login has left, so the person can
// put off heavy gh work before it runs out, and what holds the poll back. GraphQL counts
// points (the poll's own replies say how many are left, for free); REST counts requests
// (GitHub's /rate_limit, which costs nothing, asked by the round at most every REST_MS per
// login). Secondary limits, on bursts, have no counter: the line never says they're fine,
// only that one holds the poll once it does.
import type { Bucket, Limits, LoginRates } from '../types'
import type { RateLimit } from './batch'
import { cells } from './review'

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

// ---- The line -----------------------------------------------------------------------------

export type Tone = 'warning' | 'error' | null

// A bucket as it stands at `at`: past its reset, GitHub has filled it again.
export const current = (bucket: Bucket, at: number): Bucket => (at >= bucket.resetAt ? { ...bucket, remaining: bucket.limit } : bucket)

export const shareOf = (bucket: Bucket) => (bucket.limit > 0 ? bucket.remaining / bucket.limit : 1)

// Healthy (null) is dim; below WARN_SHARE a warning, below ERROR_SHARE an error.
export function toneOf(bucket: Bucket): Tone {
  const share = shareOf(bucket)

  return share < ERROR_SHARE ? 'error' : share < WARN_SHARE ? 'warning' : null
}

// 4321 → "4.3k", 5000 → "5k", 812 → "812": rounded down, so the line never says more is left
// than is.
export function kilo(n: number): string {
  if (n < 1_000) return String(Math.max(0, Math.floor(n)))
  const k = Math.floor(n / 100) / 10

  return `${Number.isInteger(k) ? k : k.toFixed(1)}k`
}

// "13:40" in the person's time zone; `offsetMin` is minutes east of UTC.
export function clockAt(ms: number, offsetMin: number): string {
  const local = new Date(ms + offsetMin * 60_000)
  const two = (n: number) => String(n).padStart(2, '0')

  return `${two(local.getUTCHours())}:${two(local.getUTCMinutes())}`
}

// `date +%z`'s "+0300" or "-0530" as minutes east of UTC; null where it says nothing like it.
export function offsetOf(text: string): number | null {
  const m = text.trim().match(/^([+-])(\d{2})(\d{2})$/)

  return m === null ? null : (m[1] === '-' ? -1 : 1) * (Number(m[2]) * 60 + Number(m[3]))
}

// "45s", "2m", "1m 30s": a wait, rounded up to the second.
export function countdown(ms: number): string {
  const s = Math.max(0, Math.ceil(ms / 1_000))
  if (s < 60) return `${s}s`
  const m = Math.floor(s / 60)

  return s % 60 === 0 ? `${m}m` : `${m}m ${s % 60}s`
}

export const pauseText = (heldUntil: number, at: number) => `⏸ GitHub rate limit · polling resumes in ${countdown(heldUntil - at)}`

export type Segment = { text: string; tone: Tone }
export type GhLine = Segment[]

const seg = (text: string, tone: Tone = null): Segment => ({ text, tone })
const widthOf = (line: GhLine) => line.reduce((n, s) => n + cells(s.text), 0)

const PREFIX = 'gh  '
const INDENT = ' '.repeat(PREFIX.length)
const SEP = ' · '

type Shown = { login: string; graphql?: Bucket; core?: Bucket }

// The logins with a bucket known, each bucket as it stands now.
function shownOf(limits: Limits, at: number): Shown[] {
  return limits.logins.flatMap(rate => {
    const graphql = rate.graphql && current(rate.graphql, at)
    const core = rate.core && current(rate.core, at)

    return graphql !== undefined || core !== undefined ? [{ login: rate.login, graphql, core }] : []
  })
}

// The bucket with the least share left that has a reset ahead (one past it is full again),
// and the line of the login it belongs to.
function lowestOf(shown: readonly Shown[], at: number): { bucket: Bucket; index: number } | null {
  let low: { bucket: Bucket; index: number } | null = null
  for (const [index, rate] of shown.entries()) {
    for (const bucket of [rate.graphql, rate.core]) {
      if (bucket === undefined || bucket.resetAt <= at) continue
      if (low === null || shareOf(bucket) < shareOf(low.bucket)) low = { bucket, index }
    }
  }

  return low
}

// The pane's gh line, to fit `columns`: a line per login, the active one first,
//   gh  octocat 4.3k/5k GraphQL · 4.9k/5k REST · resets 13:40
//       hubot 4.9k/5k GraphQL · 5k/5k REST
// and the reset time of the lowest bucket at the end of its login's line, or on a line of its
// own where it won't fit there. While a secondary limit holds, the pause line instead. None
// known yet: no line.
export function ghLines(limits: Limits, at: number, columns: number, offsetMin: number): GhLine[] {
  if (limits.heldUntil > at) return [[seg(pauseText(limits.heldUntil, at), 'warning')]]
  const shown = shownOf(limits, at)
  const lines = shown.map((rate, i) => {
    const line: GhLine = [seg(i === 0 ? PREFIX : INDENT)]
    if (rate.login !== '') line.push(seg(`${rate.login} `))
    const parts = [
      ...(rate.graphql !== undefined ? [seg(`${kilo(rate.graphql.remaining)}/${kilo(rate.graphql.limit)} GraphQL`, toneOf(rate.graphql))] : []),
      ...(rate.core !== undefined ? [seg(`${kilo(rate.core.remaining)}/${kilo(rate.core.limit)} REST`, toneOf(rate.core))] : []),
    ]
    parts.forEach((part, j) => line.push(...(j > 0 ? [seg(SEP)] : []), part))

    return line
  })
  const low = lowestOf(shown, at)
  const line = low === null ? undefined : lines[low.index]
  if (low !== null && line !== undefined) {
    const reset = seg(`resets ${clockAt(low.bucket.resetAt, offsetMin)}`, toneOf(low.bucket))
    if (widthOf(line) + cells(SEP) + cells(reset.text) <= columns) line.push(seg(SEP), reset)
    else lines.push([seg(INDENT), reset])
  }

  return lines
}

export type GhNote = { long: string; short: string; tone: Tone; isPause: boolean }

// The band's gh note, for its action line: the pause while a hold lasts; otherwise only while
// some bucket is below WARN_SHARE, the share it has left, "gh 12% · resets 13:40", or short
// "gh 12%". Healthy: nothing.
export function bandNote(limits: Limits, at: number, offsetMin: number): GhNote | null {
  if (limits.heldUntil > at) {
    return { long: pauseText(limits.heldUntil, at), short: `⏸ resumes in ${countdown(limits.heldUntil - at)}`, tone: 'warning', isPause: true }
  }
  const low = lowestOf(shownOf(limits, at), at)
  if (low === null || shareOf(low.bucket) >= WARN_SHARE) return null
  const share = `gh ${Math.floor(shareOf(low.bucket) * 100)}%`

  return { long: `${share} · resets ${clockAt(low.bucket.resetAt, offsetMin)}`, short: share, tone: toneOf(low.bucket), isPause: false }
}
