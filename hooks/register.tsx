import { atom, read, update } from 'claude-code'
import type { EngineInterface, Register } from 'claude-code'

import type { Busy, HandoffAsk, Limits, MergeAsk, Reviews, TrackedPr, Verdict } from '../types'
import { barCells, barRuns, fullBarCells, meterCells, ruleCells, ruleText, spinnerCell } from './look'
import { artCard, artCells, bannerLines, roleTone } from './art'
import type { ArtCard, Seg } from './art'
import { barPaint, cardBox, depthOf, inkOf, lookOf, modeOf, modeOfColours, optionTheme, pillOf, rasterBg, themeListing, themeOf, unknownTheme } from './theme'
import type { Depth, Look, Mode, Role, ThemeName } from './theme'
import {
  MERGE_LABEL,
  STEPS,
  ago,
  cwdOf,
  derive,
  findPrUrls,
  isActive,
  isBusy,
  mergeArgv,
  mergeError,
  mergeOf,
  mergeQuestion,
  metaOf,
  nextMethod,
  pickMethod,
  placeholder,
  shellCode,
  stepIndex,
  touched,
} from './pr'
import { appendLog, buildQuery, callBudget, checksQuery, chunks, failureOf, logLine, parseChecks, parseHttp, parseReply, rateOf, retryAfterOf } from './batch'
import type { CallFailure, CallLog, Http, RateLimit } from './batch'
import { addressHeading, addressPrompt, openThreads, parseThreads, redraft, threadsQuery } from './address'
import { failingChecks, fixHeading, fixPrompt, logError, logProblems, toFetch } from './fix'
import type { Failure, Handoff } from './fix'
import { resolveHeading, resolvePrompt } from './resolve'
import { cells, newReviews, reviewToasts, reviewsOf, threadsBeside, verdictLine } from './review'
import { CHECK_MS, headOf, howToUpdate, isNewer, noticeLines, noticeToast, versionIn } from './update'
import { BAND_ROWS, SHARED_ROWS, actionsOf, buttonCells, layoutOf, planOf, rowOf } from './band'
import type { Layout } from './band'
import { NOT_FOUND, failedEntry, isUnreachable, keptEntries } from './cache'
import type { Entry } from './cache'
import { GH_HELP, bandNote, bucketOf, ghMeter, isRestDue, legacyRate, lowUntil, restOf, roundWaitsUntil, withSeen } from './rate'
import type { Bar, Rates } from './rate'
import type { GhPr, MergeRun, PrRef } from './pr'

const PANE = 'pr-watch'
const TITLE = 'Pull requests'
// A slim dock beside the transcript (a width you drag it to wins over this).
const OPEN = { id: PANE, title: TITLE, columns: 50, rows: 14 } as const
const prs = atom({ plugin: 'pr-watch', key: 'prs' } as const, [])
const now = atom({ plugin: 'pr-watch', key: 'now' } as const, 0)
const suggested = atom({ plugin: 'pr-watch', key: 'suggested' } as const, [])
// PRs the person stopped watching: Claude touching them again doesn't bring them back.
const dropped = atom({ plugin: 'pr-watch', key: 'dropped' } as const, [])
const merging = atom({ plugin: 'pr-watch', key: 'merging' } as const, {})
// Methods GitHub refused for a repo: not offered again this session.
const refused = atom({ plugin: 'pr-watch', key: 'refused' } as const, {})
const fixing = atom({ plugin: 'pr-watch', key: 'fixing' } as const, {})
const addressing = atom({ plugin: 'pr-watch', key: 'addressing' } as const, {})
const resolving = atom({ plugin: 'pr-watch', key: 'resolving' } as const, {})
// A newer pr-watch than this one, for the footer.
const newer = atom({ plugin: 'pr-watch', key: 'newer' } as const, null)
// GitHub's budget per login and a hold, for the gh meter.
const limits = atom({ plugin: 'pr-watch', key: 'limits' } as const, { logins: [], heldUntil: 0 })
// Whether the gh meter's explanation is open (its `?`): shut until asked, per session.
const ghHelp = atom({ plugin: 'pr-watch', key: 'ghHelp' } as const, false)
const WATCH_TOOL = 'mcp__pr-watch__watch'
const FRAME_MS = 90

// Module memory: lost on reload, which only costs one extra poll.
let isTicking = false
let hasHinted = false
// What the pane last drew, so the animation can blit frames of the same size: a stacked card's
// CI bar width, and a banner card as laid out (art.ts).
const barWidth = new Map<string, number>()
const artCards = new Map<string, ArtCard>()
// A banner card repaints every ART_EVERY frames: the wave sways at the spinner's pace or slower.
const ART_EVERY = 2
// The pointer on the gh meter's `?` reveals its explanation over the pane.
const HELP_SCOPE = 'pr-watch-gh-help'
let anim: { cancel: () => void } | null = null
let frame = 0
// Whether the pane is drawn: set by its render, cleared when a spinner blit is refused (closed,
// or never opened in the band layout), so a hidden pane costs no frames.
let isPaneDrawn = false
// The band's instance, and the PRs whose spinner it last drew, so the animation blits there too.
let bandId: string | null = null
const bandSpins = new Set<string>()
// The end of the hold a timer is set for, so the pause line goes the moment it ends, ticks or not.
let pauseEndsAt = 0

// Rows kept by an older version of this mod lack the newer fields.
function full(pr: TrackedPr): TrackedPr {
  return { ...placeholder(pr), ...pr, checks: { ...placeholder(pr).checks, ...pr.checks } }
}

const TOAST_ICON: Record<string, string> = { success: '✓', error: '✗', warning: '◷', merged: '🎉', suggestion: '⇢' }
const VERDICT_COLOUR: Record<Verdict, 'success' | 'error' | 'suggestion' | undefined> = {
  approved: 'success',
  changes: 'error',
  commented: 'suggestion',
  requested: undefined,
}

// ---- One poller for the whole machine -------------------------------------------------
// Every session lists the PRs it watches under ~/.cache/pr-watch/sessions/ (the file's mtime
// is its heartbeat); whichever session finds the shared results due takes a short lease and
// fetches every live session's PRs in one batched query, and the rest read results.json.
// So N sessions cost one GitHub call per round, not N.

type Shared = {
  fetchedAt: number
  nextAt: number
  prs: Record<string, Entry>
  // Which gh login sees an owner's private repos (a login, never a token).
  accounts: Record<string, string>
  // What older versions read; this one keeps it as legacyRate says (rate.ts), and reads `rates`.
  rate: RateLimit | null
  // GitHub's hourly limits per gh login, as the last round and watch saw them (rate.ts).
  rates: Rates
  // pr-watch's newest version as the last check found it (null: none read yet), and when
  // that check rode a round.
  latest: { version: string | null; checkedAt: number }
  // Until when a secondary limit holds every session off GitHub (absent: none yet).
  heldUntil?: number
}

// How often a round runs while some PR moves, and otherwise: the `poll_active_seconds` and
// `poll_idle_seconds` options, kept current by `config.set`.
const MIN_POLL_MS = 10_000
const pace = { fastMs: 20_000, slowMs: 60_000 }
// The `update_notice` option, kept current the same way.
let isNoticeOn = true
// The `layout` option, the same way: the pane (opened as PRs come), or the band above the prompt.
let layout: Layout = 'pane'
// The `theme` option, the same way, or what /pr-watch theme picked; and whether the person's
// terminal is light or dark (readMode), dark until known.
let themeName: ThemeName = 'classic'
let mode: Mode = 'dark'
// How many colours Claude Code paints this terminal with (readDepth): 24-bit until known.
let depth: Depth = 'truecolor'
const look = (): Look => lookOf(themeName, mode, depth)
const seconds = (value: unknown, fallback: number) =>
  typeof value === 'number' && Number.isFinite(value) ? Math.max(MIN_POLL_MS, value * 1_000) : fallback
// A session whose file has not been touched for this long has exited.
const ALIVE_MS = 60_000
const LEASE_MS = 30_000
// A batch that failed on GitHub having a moment (a `timeout`, batch.ts) is asked once more,
// RETRY_MS on, while the lease has time for it (callBudget, as for every call of a round).
const RETRY_MS = 2_000
const GH_MS = 20_000
// /rate_limit answers at once, or not worth waiting for.
const REST_CALL_MS = 5_000
// How long every session holds off after a secondary limit that names no retry-after.
const LIMIT_WAIT_MS = 60_000
const PRUNE_MS = 86_400_000

// Survives /clear (the module stays loaded), unlike the session id.
const INSTANCE = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`
let homeDir: string | null | undefined
let dir: string | null = null
let loginsCache: Promise<{ active: string; others: string[] }> | null = null
const tokens = new Map<string, Promise<string | null>>()
let prunedAt = 0

const empty = (): Shared => ({ fetchedAt: 0, nextAt: 0, prs: {}, accounts: {}, rate: null, rates: {}, latest: { version: null, checkedAt: 0 } })

async function gh($: EngineInterface, argv: string[], token?: string | null, cwd?: string, timeoutMs = GH_MS) {
  return $.process.run(['gh', ...argv], {
    cwd,
    timeoutMs,
    env: token ? { GH_TOKEN: token } : undefined,
  })
}

async function home($: EngineInterface): Promise<string | null> {
  if (homeDir === undefined) homeDir = (await $.env.get('HOME')) ?? null

  return homeDir
}

async function root($: EngineInterface): Promise<string> {
  dir ??= `${(await home($)) ?? '/tmp'}/.cache/pr-watch`

  return dir
}

async function readJson<T>($: EngineInterface, path: string): Promise<T | null> {
  try {
    return JSON.parse(String(await $.fs.read(path))) as T
  } catch {
    return null
  }
}

async function writeJson($: EngineInterface, path: string, value: unknown) {
  try {
    await $.fs.write(path, JSON.stringify(value))
  } catch {
    // The cache is an optimisation: a session that cannot write it still polls for itself.
  }
}

async function readShared($: EngineInterface): Promise<Shared> {
  return { ...empty(), ...(await readJson<Shared>($, `${await root($)}/results.json`)) }
}

// This session's watch list; written every tick, so its mtime says the session is alive.
async function announce($: EngineInterface, urls: readonly string[]) {
  await writeJson($, `${await root($)}/sessions/${INSTANCE}.json`, { urls })
}

async function liveUrls($: EngineInterface, at: number): Promise<string[]> {
  const base = `${await root($)}/sessions`
  let entries: Awaited<ReturnType<EngineInterface['fs']['list']>> = []
  try {
    entries = await $.fs.list(base)
  } catch {
    return []
  }
  const live = entries.filter(f => f.kind === 'file' && f.name.endsWith('.json') && at - f.mtimeMs < ALIVE_MS)
  const lists = await Promise.all(live.map(f => readJson<{ urls?: string[] }>($, `${base}/${f.name}`)))
  if (at - prunedAt > 3_600_000) {
    prunedAt = at
    const dead = entries.filter(f => f.kind === 'file' && at - f.mtimeMs > PRUNE_MS).map(f => `${base}/${f.name}`)
    if (dead.length > 0) void $.process.run(['rm', '-f', ...dead], { timeoutMs: 5_000 }).catch(() => undefined)
  }

  return [...new Set(lists.flatMap(list => list?.urls ?? []))]
}

// A lease, not a lock: plain files have no atomic create, so claim, wait, and re-read. Two
// sessions racing past this costs one duplicate round, never a missed one.
async function takeLease($: EngineInterface, at: number): Promise<boolean> {
  const path = `${await root($)}/lease.json`
  const held = await readJson<{ owner: string; until: number }>($, path)
  if (held !== null && held.owner !== INSTANCE && held.until > at) return false
  await writeJson($, path, { owner: INSTANCE, until: at + LEASE_MS })
  await $.clock.sleep(150)

  return (await readJson<{ owner: string }>($, path))?.owner === INSTANCE
}

async function releaseLease($: EngineInterface) {
  await writeJson($, `${await root($)}/lease.json`, { owner: INSTANCE, until: 0 })
}

async function logins($: EngineInterface) {
  loginsCache ??= (async () => {
    const status = await gh($, ['auth', 'status', '--hostname', 'github.com'])
    const text = `${status.stdout}\n${status.stderr}`
    const all = [...text.matchAll(/account (\S+)[^\n]*\n\s*-\s*Active account: (true|false)/g)]
    const active = all.find(m => m[2] === 'true')?.[1] ?? ''

    return { active, others: all.map(m => m[1] ?? '').filter(login => login !== '' && login !== active) }
  })()

  return loginsCache
}

// The active account needs no token; another account's comes from gh, once per process.
async function tokenFor($: EngineInterface, login: string): Promise<string | null> {
  if (login === '' || login === (await logins($)).active) return null
  if (!tokens.has(login)) {
    tokens.set(
      login,
      gh($, ['auth', 'token', '--hostname', 'github.com', '--user', login]).then(r => (r.exitCode === 0 ? r.stdout.trim() : null)),
    )
  }

  return (await tokens.get(login)) ?? null
}

const ownerOf = (ref: PrRef) => ref.repo.split('/')[0] ?? ''

// One batch's call, timed, and what GitHub said: the status and headers, and the reply.
async function askBatch($: EngineInterface, part: readonly PrRef[], argv: string[], token: string | null, timeoutMs = GH_MS) {
  const startedAt = await $.clock.now()
  // A call past its timeout throws: it fails as a timeout, rather than losing the round.
  const res = await gh($, argv, token, undefined, timeoutMs).catch(() => ({
    stdout: '',
    stderr: `gh api graphql did not finish: timeout after ${Math.round(timeoutMs / 1_000)}s`,
  }))
  const http = parseHttp(res.stdout)

  return { http, reply: parseReply(http.body, res.stderr, part), ms: (await $.clock.now()) - startedAt }
}

// Why a failed call failed (null: it didn't), logged to errors.log on the way.
async function failureIn($: EngineInterface, login: string, size: number, call: { http: Http; reply: { error: string | null }; ms: number }) {
  if (call.reply.error === null) return null
  const failure: CallFailure = failureOf(call.reply.error, call.http)
  await logFailure($, { login: login || (await logins($)).active, size, ms: call.ms, http: call.http, failure, error: call.reply.error })

  return failure
}

// Best effort: a log that can't be read starts over, and one that can't be written is skipped.
async function logFailure($: EngineInterface, call: CallLog) {
  const path = `${await root($)}/errors.log`
  let log = ''
  try {
    log = String(await $.fs.read(path))
  } catch {
    // None yet.
  }
  try {
    await $.fs.write(path, appendLog(log, logLine(await $.clock.now(), call)))
  } catch {
    // Nothing to do: the log is for the person reading it later.
  }
}

// `withLatest`: the first batch also asks for pr-watch's newest version. `until`: when the
// lease ends (a second kept to write the results). A batch starts only with time left before
// it and may take no more than is left (callBudget); the rest are `left` for the next round,
// as they were. A secondary limit stops the asking (`waitMs`: how long every session holds
// off), and spent points stop this login's. Neither is a card's error: the gh meter says it
// (rate.ts), and the PRs keep what they have.
async function query($: EngineInterface, refs: readonly PrRef[], login: string, until: number, withLatest = false) {
  const token = await tokenFor($, login)
  const out = {
    found: new Map<string, GhPr>(),
    missing: [] as PrRef[],
    failed: new Map<string, string>(),
    left: [] as PrRef[],
    // The points as the last reply said, and when.
    rate: null as RateLimit | null,
    rateAt: 0,
    latest: null as string | null,
    isLatestAsked: false,
    waitMs: null as number | null,
  }
  const parts = chunks(refs)
  for (const [i, part] of parts.entries()) {
    const budget = callBudget(until, await $.clock.now(), GH_MS)
    if (budget === null) {
      out.left.push(...parts.slice(i).flat())

      break
    }
    const argv = ['api', 'graphql', '-f', `query=${buildQuery(part, withLatest && i === 0)}`, '-i']
    out.isLatestAsked ||= withLatest && i === 0
    let call = await askBatch($, part, argv, token, budget)
    let failure = await failureIn($, login, part.length, call)
    if (failure === 'timeout') {
      const retry = callBudget(until, (await $.clock.now()) + RETRY_MS, GH_MS)
      if (retry !== null) {
        await $.clock.sleep(RETRY_MS)
        call = await askBatch($, part, argv, token, retry)
        failure = await failureIn($, login, part.length, call)
      }
    }
    const { http, reply } = call
    const isLimit = failure === 'secondary-limit' || failure === 'primary-limit'
    if (reply.error !== null && !isLimit) for (const ref of part) out.failed.set(ref.url, reply.error)
    for (const [url, pr] of reply.found) out.found.set(url, pr)
    out.missing.push(...reply.missing)
    const rate = rateOf(http.headers) ?? reply.rate
    if (rate !== null) {
      out.rate = rate
      out.rateAt = await $.clock.now()
    }
    out.latest = reply.latest ?? out.latest
    if (failure === 'secondary-limit') {
      const after = retryAfterOf(http.headers)
      out.waitMs = after !== null ? after * 1_000 : LIMIT_WAIT_MS

      break
    }
    // Spent points: the login's other batches would only be refused the same way. This one
    // went unanswered too, so it is left as well, never read as "not found".
    if (failure === 'primary-limit') {
      out.left.push(...parts.slice(i).flat())

      break
    }
  }

  return out
}

// One batched query per gh account; PRs the expected account cannot see are retried with
// each other logged-in account, and the one that sees them is remembered per owner. The
// newest version, when asked, rides the first batch that goes; `asked` says whether it went.
// On a round (`isRound`), a login whose points are low (rate.ts) is left out until its reset,
// its PRs keeping what they have, and the others are asked as ever; a watch or a Refresh, one
// call the person asked for, still goes. `rates` goes in as known and comes back with
// the points each login's replies said; `needed`: the logins these PRs are polled as.
async function fetchRefs(
  $: EngineInterface,
  refs: readonly PrRef[],
  accounts: Record<string, string>,
  rates: Rates,
  at: number,
  until: number,
  isRound: boolean,
  withLatest = false,
) {
  const found = new Map<string, GhPr>()
  const errors = new Map<string, { error: string; isMiss: boolean }>()
  let seen = rates
  let latest: string | null = null
  let isAsking = withLatest
  let waitMs: number | null = null
  const { active, others } = await logins($)
  // The active account polls the owners no other account was found for.
  const nameOf = (login: string) => login || active
  const isLow = (login: string) => isRound && lowUntil(seen, login, at) !== null
  const saw = (login: string, r: { rate: RateLimit | null; rateAt: number }) => {
    const graphql = bucketOf(r.rate, r.rateAt)
    if (graphql !== null) seen = withSeen(seen, nameOf(login), { graphql })
  }
  const byLogin = new Map<string, PrRef[]>()
  for (const ref of refs) byLogin.set(accounts[ownerOf(ref)] ?? '', [...(byLogin.get(accounts[ownerOf(ref)] ?? '') ?? []), ref])
  const needed = [...new Set([...byLogin.keys()].map(nameOf))]

  let missing: PrRef[] = []
  for (const [login, group] of byLogin) {
    if (waitMs !== null) break
    if (isLow(nameOf(login))) continue
    const r = await query($, group, login, until, isAsking)
    if (r.isLatestAsked) isAsking = false
    waitMs = r.waitMs
    saw(login, r)
    r.found.forEach((pr, url) => found.set(url, pr))
    r.failed.forEach((error, url) => errors.set(url, { error, isMiss: false }))
    missing.push(...r.missing)
    latest = r.latest ?? latest
  }
  if (missing.length > 0 && waitMs === null) {
    // "Not found" only once every other account was asked and answered: one that failed, ran
    // out of time or is low on points might see them yet.
    let isSure = true
    for (const login of [active, ...others]) {
      const untried = missing.filter(ref => (accounts[ownerOf(ref)] ?? '') !== login && (login !== active || ownerOf(ref) in accounts))
      if (untried.length === 0) continue
      if (isLow(login)) {
        isSure = false

        continue
      }
      const r = await query($, untried, login, until)
      waitMs = r.waitMs
      saw(login, r)
      if (r.failed.size > 0 || r.left.length > 0) isSure = false
      r.found.forEach((pr, url) => {
        found.set(url, pr)
        const ref = untried.find(x => x.url === url)
        if (ref !== undefined) accounts[ownerOf(ref)] = login
      })
      missing = missing.filter(ref => !r.found.has(ref.url))
      if (missing.length === 0 || waitMs !== null) break
    }
    // Held by a secondary limit, the rest were not all asked: they wait, as they were.
    if (waitMs === null && isSure) for (const ref of missing) errors.set(ref.url, { error: NOT_FOUND, isMiss: true })
  }

  return { found, errors, rates: seen, needed, latest, waitMs, asked: withLatest && !isAsking }
}

// GitHub's /rate_limit for each logged-in login once REST_MS is up (rate.ts): REST's requests,
// and the points again, for the gh meter. It counts against no limit. Only a round asks, in
// what is left of its lease, never every session; asked counts as checked, answered or not.
async function askRest($: EngineInterface, rates: Rates, at: number, until: number): Promise<Rates> {
  const { active, others } = await logins($)
  let seen = rates
  for (const login of [...new Set([active, ...others])]) {
    if (!isRestDue(seen, login, at)) continue
    const budget = callBudget(until, await $.clock.now(), REST_CALL_MS)
    if (budget === null) break
    const res = await gh($, ['api', 'rate_limit'], await tokenFor($, login), undefined, budget).catch(() => null)
    const read = res !== null && res.exitCode === 0 ? restOf(res.stdout, await $.clock.now()) : null
    seen = withSeen(seen, login, { ...read, askedAt: at })
  }

  return seen
}

const refFromUrl = (url: string): PrRef | null => {
  const m = url.match(/^https:\/\/github\.com\/([\w.-]+\/[\w.-]+)\/pull\/(\d+)$/)

  return m === null ? null : { url, repo: m[1] ?? '', number: Number(m[2]) }
}

// Fetches `refs` now and merges them into the shared results, which it returns. `live`, on a
// round: the PRs live sessions watch; the round drops the others once they are old.
async function publish($: EngineInterface, refs: readonly PrRef[], at: number, live?: ReadonlySet<string>): Promise<Shared> {
  const isRound = live !== undefined
  const before = await readShared($)
  // Held by a secondary limit: no round, no Refresh, no new watch asks until the wait is over.
  if ((before.heldUntil ?? 0) > at) return before
  const until = at + LEASE_MS - 1_000
  // A round asks for the newest version too, once the last check is CHECK_MS old.
  const withLatest = isRound && isNoticeOn && at - (before.latest?.checkedAt ?? 0) >= CHECK_MS
  const { found, errors, rates, needed, latest, waitMs, asked } = await fetchRefs($, refs, before.accounts, before.rates, at, until, isRound, withLatest)
  // REST's count rides a round, never a watch or a Refresh, nor a round a limit stopped.
  const seen = isRound && waitMs === null ? await askRest($, rates, at, until) : rates
  // Re-read: another session may have written while this one was asking GitHub.
  const shared = await readShared($)
  shared.accounts = { ...shared.accounts, ...before.accounts }
  found.forEach((pr, url) => (shared.prs[url] = { at, pr }))
  // A PR with data keeps it through a failure, and its card says so only once it lasts.
  errors.forEach(({ error, isMiss }, url) => (shared.prs[url] = failedEntry(shared.prs[url], error, at, isMiss)))
  // Each login's latest sighting wins, whichever session made it.
  for (const [login, rate] of Object.entries(seen)) shared.rates = withSeen(shared.rates, login, rate)
  shared.rate = legacyRate(shared.rates) ?? shared.rate
  // Asked is checked, answered or not: a check that fails waits as long as one that didn't,
  // so it never rides every round. An unanswered one keeps the version known before.
  if (asked) shared.latest = { version: latest ?? shared.latest?.version ?? null, checkedAt: at }
  if (isRound) {
    shared.prs = keptEntries(shared.prs, live, at)
    const isMoving = [...found.values()].some(pr => derive(pr).isMoving)
    shared.fetchedAt = at
    shared.nextAt = at + (isMoving ? pace.fastMs : pace.slowMs)
    // Only once every login the round needs is low does the round itself wait, for the first
    // reset; one spent login holds back its own PRs alone.
    const resetAt = roundWaitsUntil(shared.rates, needed, at)
    if (resetAt !== null) shared.nextAt = Math.max(shared.nextAt, resetAt)
  }
  // A secondary limit: no round, and no Refresh, until GitHub's wait is over. A hold another
  // session set while this one asked stands too: this round's nextAt never cuts it short.
  if (waitMs !== null) shared.heldUntil = Math.max(shared.heldUntil ?? 0, (await $.clock.now()) + waitMs)
  shared.nextAt = Math.max(shared.nextAt, shared.heldUntil ?? 0)
  await writeJson($, `${await root($)}/results.json`, shared)

  return shared
}

// A session's tick: say what it watches, then either run the round (lease held) or read it.
async function tick($: EngineInterface, own: readonly PrRef[], at: number): Promise<Shared> {
  await announce($, own.map(ref => ref.url))
  const shared = await readShared($)
  if (at < shared.nextAt || !(await takeLease($, at))) return shared
  try {
    const urls = new Set([...(await liveUrls($, at)), ...own.map(ref => ref.url)])
    // Longest unasked first, so a batch the last round had no time for goes first in this one.
    const askedAt = (url: string) => shared.prs[url]?.failedSince ?? shared.prs[url]?.at ?? 0
    const due = [...urls]
      .flatMap(url => {
        const entry = shared.prs[url]
        const isFinished = entry?.pr !== undefined && entry.pr.state !== 'OPEN'
        const ref = refFromUrl(url)

        return ref === null || isUnreachable(entry, at) || isFinished ? [] : [ref]
      })
      .sort((a, b) => askedAt(a.url) - askedAt(b.url))

    return await publish($, due, at, urls)
  } finally {
    await releaseLease($)
  }
}

// ---- This session's cards -------------------------------------------------------------

const refOf = (pr: TrackedPr): PrRef => ({ url: pr.url, repo: pr.repo, number: pr.number })

// Folds the shared results into this session's cards: newer entries only, and a toast when
// a card's state changes or a review lands. A new review's toast says it all: the state change
// it caused (a review or ready headline) doesn't toast as well; CI, conflicts and a merge still do.
async function apply($: EngineInterface, shared: Shared) {
  const at = await $.clock.now()
  const list = (await read($, prs)).map(full)
  // The person's own reviews never toast, whichever of their gh accounts wrote them.
  const { active, others } = await logins($)
  const moved: string[] = []
  const next = list.map(current => {
    const entry = shared.prs[current.url]
    if (entry === undefined) return current
    const error = entry.error ?? null
    // No newer data, yet the error may have come or gone: a failure that lasted keeps the
    // entry's time, so "checked 2m ago" stays the age of what the card shows.
    if (current.checkedAt !== null && entry.at <= current.checkedAt) return error === current.error ? current : { ...current, error }
    if (entry.pr === undefined) return { ...current, error, checkedAt: current.checkedAt ?? entry.at }
    const { fresh, seen } = newReviews(entry.pr, current.reviewSeen, [active, ...others], entry.pr.author?.login)
    const pr: TrackedPr = {
      ...current,
      title: entry.pr.title,
      state: entry.pr.state,
      ...derive(entry.pr, current.reviews),
      ...metaOf(entry.pr),
      merge: mergeOf(entry.pr, current.merge),
      reviews: reviewsOf(entry.pr) ?? current.reviews,
      reviewSeen: seen,
      checkedAt: entry.at,
      error,
    }
    const said = reviewToasts(fresh, pr.number)
    for (const text of said) $.ui.toast(text)
    if (current.checkedAt !== null && current.pill !== pr.pill && pr.error === null) {
      const isReviewCaused = said.length > 0 && (pr.stage === 'review' || (pr.stage === 'ready' && pr.tone !== 'error'))
      if (!isReviewCaused) $.ui.toast(`${TOAST_ICON[pr.tone] ?? '•'} PR #${pr.number} · ${pr.headline}`)
      moved.push(pr.url)
    }

    return pr
  })
  await update($, prs, () => next)
  await update($, now, () => at)
  await update($, limits, current => {
    const shown = limitsOf(shared, [active, ...others])

    return JSON.stringify(current) === JSON.stringify(shown) ? current : shown
  })
  const heldUntil = shared.heldUntil ?? 0
  if (heldUntil > at && heldUntil !== pauseEndsAt) {
    pauseEndsAt = heldUntil
    $.clock.after(heldUntil - at, async () => {
      const ended = await $.clock.now()
      await update($, now, current => Math.max(current, ended))
    })
  }
  // What a hand-off press said belongs to the failure, or the threads, it was about.
  if (moved.length > 0) {
    for (const handoff of HANDOFFS) {
      await updateRows($, handoff.id, all => Object.fromEntries(Object.entries(all).filter(([url, ask]) => !moved.includes(url) || isBusy(ask.busy, at))))
    }
  }
  await refreshStatus($)
  await syncAnimation($)
  // Last, and never thrown: the update notice must not cost a PR result.
  await sayNewer($, shared).catch(() => undefined)
}

// What the gh meter shows: the logged-in logins' buckets, the active one first (every login the
// results know of, where gh named none), and the hold.
function limitsOf(shared: Shared, names: readonly string[]): Limits {
  const { rates } = shared
  const known = names.filter(name => name !== '')
  const order = known.length > 0 ? known : Object.keys(rates)

  return {
    logins: order.flatMap(login => {
      const { graphql, core } = rates[login] ?? {}

      return graphql !== undefined || core !== undefined ? [{ login, ...(graphql && { graphql }), ...(core && { core }) }] : []
    }),
    heldUntil: shared.heldUntil ?? 0,
  }
}

// This copy's own version, from its plugin.json (the API names the plugin, not its version).
// Kept once read (null: no plain version to compare); unreadable, it is read again next time,
// and until then nothing is said.
let ownVersion: string | null | undefined
// The version this load toasted, should notice.json not take the write.
let toldVersion: string | null = null

async function readOwnVersion($: EngineInterface): Promise<string | null> {
  if (ownVersion === undefined) {
    try {
      ownVersion = versionIn(String(await $.fs.read(`${$.plugin.root}/.claude-plugin/plugin.json`)))
    } catch {
      return null
    }
  }

  return ownVersion
}

// Whether the last check found a newer pr-watch: the footer says so, and one toast per new
// version across every session on the machine (notice.json keeps the one toasted, so a new
// session or a reload doesn't toast it again).
async function sayNewer($: EngineInterface, shared: Shared) {
  const own = await readOwnVersion($)
  const latest = shared.latest?.version ?? null
  const next =
    isNoticeOn && own !== null && latest !== null && isNewer(latest, own)
      ? { version: latest, how: howToUpdate($.plugin.root, await home($), $.plugin.name) }
      : null
  await update($, newer, current => (current?.version === next?.version && current?.how === next?.how ? current : next))
  if (next === null || toldVersion === next.version) return
  toldVersion = next.version
  const path = `${await root($)}/notice.json`
  if ((await readJson<{ version?: string }>($, path))?.version === next.version) return
  // Claim, wait, re-read (as takeLease does): sessions that read notice.json at once would
  // otherwise all toast. Only the claim that stuck toasts.
  await writeJson($, path, { version: next.version, owner: INSTANCE })
  await $.clock.sleep(150)
  const claim = await readJson<{ version?: string; owner?: string }>($, path)
  if (claim?.version === next.version && claim.owner === INSTANCE) $.ui.toast(noticeToast(next.version, next.how))
}

// Fetches these PRs now (a new watch, the Refresh button) rather than waiting for the round.
async function fetchNow($: EngineInterface, refs: PrRef[]) {
  if (refs.length === 0) return
  // Held by a secondary limit, publish asks nothing: the next round does, once the wait is over.
  await apply($, await publish($, refs, await $.clock.now()))
}

async function setAsk($: EngineInterface, url: string, ask: MergeAsk | null) {
  await update($, merging, all => {
    const { [url]: _, ...rest } = all

    return ask === null ? rest : { ...rest, [url]: ask }
  })
}

// One question at a time across the pane, so y and n answer the one on screen.
async function oneQuestion($: EngineInterface) {
  await update($, merging, all => Object.fromEntries(Object.entries(all).map(([url, a]) => [url, { ...a, asking: undefined }])))
  for (const handoff of HANDOFFS) {
    await updateRows($, handoff.id, all => Object.fromEntries(Object.entries(all).map(([url, a]) => [url, { ...a, asking: undefined }])))
  }
}

// Marks a card's merge row busy for a run, unless a run already holds it: false then, so a
// second press (a double click, `y` twice) starts nothing. One write, so two can't both win.
async function claimMerge($: EngineInterface, url: string, ask: MergeAsk & Required<Pick<MergeAsk, 'busy'>>) {
  let isClaimed = false
  await update($, merging, all => {
    isClaimed = !isBusy(all[url]?.busy, ask.busy.at)

    return isClaimed ? { ...all, [url]: ask } : all
  })

  return isClaimed
}

const BUSY: Record<MergeRun['action'], string> = {
  merge: 'Merging…',
  auto: 'Turning on auto-merge…',
  'cancel-auto': 'Cancelling auto-merge…',
}

// Runs gh pr merge as the account the poller found sees the repo (its token, never kept),
// then asks GitHub again at once so the card shows what happened.
async function runMerge($: EngineInterface, pr: TrackedPr, run: MergeRun) {
  const method = run.action === 'cancel-auto' ? undefined : run.method
  const busy = { action: run.action, at: await $.clock.now() }
  if (!(await claimMerge($, pr.url, { method, busy }))) return
  let ask: MergeAsk = { method }
  try {
    const login = (await readShared($)).accounts[ownerOf(pr)] ?? ''
    const res = await gh($, mergeArgv(pr.url, run), await tokenFor($, login))
    if (res.exitCode !== 0) {
      const { message, isMethodRefused } = mergeError(res.stderr || res.stdout)
      ask = { method: isMethodRefused ? undefined : method, error: message }
      if (isMethodRefused && method !== undefined) {
        await update($, refused, all => ({ ...all, [pr.repo]: [...new Set([...(all[pr.repo] ?? []), method])] }))
      }
    }
  } catch {
    ask = { method, error: 'gh pr merge did not finish' }
  }
  // Only while the row is still this run's: not on a dismissed card, nor over a later press.
  await update($, merging, all => (all[pr.url]?.busy?.at === busy.at ? { ...all, [pr.url]: ask } : all))
  await fetchNow($, [refOf(pr)])
}

// Which hand-off a row is: Fix with Claude's (`fixing`), Address with Claude's (`addressing`)
// or Resolve with Claude's (`resolving`). They run apart, so a card can collect logs while its
// threads come.
type Kind = 'fix' | 'address' | 'resolve'
type Rows = Record<string, HandoffAsk>

// A hand-off row's state, each atom named where it is read or written, so the mod's state can
// be listed.
async function readRows($: EngineInterface, kind: Kind): Promise<Rows> {
  if (kind === 'fix') return read($, fixing)

  return kind === 'address' ? read($, addressing) : read($, resolving)
}

async function updateRows($: EngineInterface, kind: Kind, change: (all: Rows) => Rows) {
  if (kind === 'fix') await update($, fixing, change)
  else if (kind === 'address') await update($, addressing, change)
  else await update($, resolving, change)
}

async function setHandoff($: EngineInterface, kind: Kind, url: string, ask: HandoffAsk | null) {
  await updateRows($, kind, all => {
    const { [url]: _, ...rest } = all

    return ask === null ? rest : { ...rest, [url]: ask }
  })
}

// A collection's outcome, only while the row is still its own (see runMerge).
async function settleHandoff($: EngineInterface, kind: Kind, url: string, at: number, ask: HandoffAsk) {
  await updateRows($, kind, all => (all[url]?.busy?.at === at ? { ...all, [url]: ask } : all))
}

// claimMerge for a hand-off row: a second press while the logs or threads come starts nothing.
async function claimHandoff($: EngineInterface, kind: Kind, url: string, busy: Busy) {
  let isClaimed = false
  await updateRows($, kind, all => {
    isClaimed = !isBusy(all[url]?.busy, busy.at)

    return isClaimed ? { ...all, [url]: { busy } } : all
  })

  return isClaimed
}

// Collects what failed on a card, as the gh account the poller found sees the repo: the
// failing checks as GitHub has them now, and the failed steps' logs of the first few Actions
// jobs. A part that won't come is said on the card; the rest still goes, names and links.
async function collectFailure($: EngineInterface, pr: TrackedPr) {
  const busy = { at: await $.clock.now() }
  if (!(await claimHandoff($, 'fix', pr.url, busy))) return
  try {
    const token = await tokenFor($, (await readShared($)).accounts[ownerOf(pr)] ?? '')
    let failures: Failure[]
    let problem: string | null = null
    try {
      const res = await gh($, ['api', 'graphql', '-f', `query=${checksQuery(refOf(pr))}`], token)
      const reply = parseChecks(res.stdout, res.stderr, refOf(pr))
      if ('error' in reply) throw new Error(reply.error)
      failures = failingChecks(reply.checks, pr.repo)
    } catch (err) {
      // The names the card shows, and the PR's checks page, are still worth handing over.
      problem = `Couldn't read the checks: ${err instanceof Error ? err.message : 'gh api graphql failed'}`
      failures = pr.checks.failing.map(name => ({ name, workflow: null, conclusion: 'failed', url: null, summary: null, jobId: null }))
    }
    if (failures.length === 0) {
      await settleHandoff($, 'fix', pr.url, busy.at, { error: 'No check is failing on the latest runs now' })
      await fetchNow($, [refOf(pr)])

      return
    }
    const wanted = new Set(toFetch(failures))
    const handoffs = await Promise.all(
      failures.map(async (failure): Promise<Handoff> => {
        if (!wanted.has(failure) || failure.jobId === null) return { failure }
        try {
          const argv = ['run', 'view', '--job', String(failure.jobId), '--log-failed', '-R', pr.repo]
          const res = await gh($, argv, token, undefined, 60_000)

          return res.exitCode === 0 ? { failure, log: res.stdout } : { failure, error: logError(res.stderr || res.stdout) }
        } catch {
          return { failure, error: 'gh run view did not finish' }
        }
      }),
    )
    // Dismissed while the logs came (its card, and so its draft, are gone), or pressed again
    // once this run looked stale: the later press drafts.
    if ((await readRows($, 'fix'))[pr.url]?.busy?.at !== busy.at) return
    const error = [problem, logProblems(handoffs)].filter(Boolean).join('; ') || undefined
    await handOver($, 'fix', pr, fixHeading(pr), fixPrompt(pr, handoffs), error)
  } catch {
    await settleHandoff($, 'fix', pr.url, busy.at, { error: "Couldn't hand the failure over" })
  }
}

// Reads a card's review threads, what they say too, as the gh account the poller found sees
// the repo: one query for this PR alone, on the press, so the poll never carries a comment.
async function collectThreads($: EngineInterface, pr: TrackedPr) {
  const busy = { at: await $.clock.now() }
  if (!(await claimHandoff($, 'address', pr.url, busy))) return
  try {
    const token = await tokenFor($, (await readShared($)).accounts[ownerOf(pr)] ?? '')
    const res = await gh($, ['api', 'graphql', '-f', `query=${threadsQuery(refOf(pr))}`], token)
    const reply = parseThreads(res.stdout, res.stderr)
    if ('error' in reply) {
      await settleHandoff($, 'address', pr.url, busy.at, { error: `Couldn't read the review threads: ${reply.error}` })

      return
    }
    if (openThreads(reply.threads).length === 0) {
      await settleHandoff($, 'address', pr.url, busy.at, { error: 'No review thread is unresolved now' })
      await fetchNow($, [refOf(pr)])

      return
    }
    // Dismissed, or pressed again once stale, while the threads came (see collectFailure).
    if ((await readRows($, 'address'))[pr.url]?.busy?.at !== busy.at) return
    await handOver($, 'address', pr, addressHeading(pr), addressPrompt(pr, reply.threads, reply.total), undefined)
  } catch {
    await settleHandoff($, 'address', pr.url, busy.at, { error: "Couldn't hand the review comments over" })
  }
}

// Drafts the ask to resolve a card's conflicts. GitHub doesn't list a conflict's files, so
// there is nothing to collect: the card's latest round says whether a conflict is still there.
// The row is claimed all the same, so a second press drafts nothing more.
async function draftResolve($: EngineInterface, pr: TrackedPr) {
  const busy = { at: await $.clock.now() }
  if (!(await claimHandoff($, 'resolve', pr.url, busy))) return
  try {
    // Dismissed since the press: no card, no draft.
    const card = (await read($, prs)).map(full).find(p => p.url === pr.url)
    if (card === undefined) {
      await setHandoff($, 'resolve', pr.url, null)

      return
    }
    if (!card.isConflicted) {
      await settleHandoff($, 'resolve', pr.url, busy.at, { error: 'No conflict with base now' })
      await fetchNow($, [refOf(pr)])

      return
    }
    await handOver($, 'resolve', card, resolveHeading(card), resolvePrompt(card), undefined)
  } catch {
    await settleHandoff($, 'resolve', pr.url, busy.at, { error: "Couldn't hand the conflict over" })
  }
}

// A draft in the prompt box, to read and send with Enter: nothing reaches Claude unseen. Where
// the box won't take one (none on this surface, or a hook kept it out), the card asks instead,
// and only Confirm sends it. A dialog holding the keys just goes away, so that one waits.
async function handOver($: EngineInterface, kind: Kind, pr: TrackedPr, heading: string, text: string, error: string | undefined) {
  // Over an earlier draft of this hand-off (the others' kept), or an empty box; after
  // anything the person typed.
  const filled = await $.prompt.fill(redraft((await $.prompt.read()).text, heading, text))
  if (filled.isFilled) {
    await setHandoff($, kind, pr.url, { done: 'Drafted in the prompt box: read it, then press Enter', error })
  } else if (filled.refusal === 'dialog') {
    await setHandoff($, kind, pr.url, { error: 'A dialog holds the prompt box; close it and press again' })
  } else {
    await oneQuestion($)
    await setHandoff($, kind, pr.url, { asking: text, error })
  }
}

async function sendHandoff($: EngineInterface, kind: Kind, pr: TrackedPr, text: string) {
  await setHandoff($, kind, pr.url, { done: 'Sent to Claude' })
  // Not awaited: the prompt runs as a turn of its own once the session is idle. A hook that
  // keeps it out, or a call that fails, takes back the "sent".
  void $.prompt.submit({ text, asUser: true }).then(
    async res => {
      if (res.drop !== undefined) await setHandoff($, kind, pr.url, { error: `Not sent: ${res.drop}` })
    },
    async () => setHandoff($, kind, pr.url, { error: "Couldn't send it to Claude" }).catch(() => undefined),
  ).catch(() => undefined)
}

// The hand-offs a card can offer, the card's order: Resolve with Claude where the branch
// conflicts with its base (a conflict outranks CI and review, as on the card), Fix with Claude
// where CI failed, Address with Claude where review threads are open. `what` names the hand-off
// in the question asked instead of a draft; `verb` and `key`, its button in the band.
const HANDOFFS = [
  {
    id: 'resolve',
    label: 'Resolve with Claude',
    verb: 'Resolve',
    // `r` is Refresh's: `c`, for the conflict.
    key: 'c',
    busy: 'Drafting…',
    what: 'merge conflicts',
    isOffered: (pr: TrackedPr) => pr.isConflicted,
  },
  {
    id: 'fix',
    label: 'Fix with Claude',
    verb: 'Fix',
    key: 'f',
    busy: 'Collecting logs…',
    what: 'CI failure',
    isOffered: (pr: TrackedPr) => pr.checks.failed > 0,
  },
  {
    id: 'address',
    label: 'Address with Claude',
    verb: 'Address',
    key: 'a',
    busy: 'Reading review threads…',
    what: 'review comments',
    isOffered: (pr: TrackedPr) => (pr.reviews?.unresolved ?? 0) > 0,
  },
] as const

// A hand-off pressed in the band: the draft lands in the prompt box as from a card. Where it
// asks first, or goes wrong, instead, the pane opens, with the keys, where that card says so:
// the band has no room for the question.
async function bandHandoff($: EngineInterface, kind: Kind, pr: TrackedPr) {
  // A question already waits on the card: the pane answers it; a second collection would
  // only replace it.
  if ((await readRows($, kind))[pr.url]?.asking !== undefined) {
    await $.ui.open({ ...OPEN, focus: true })

    return
  }
  if (kind === 'resolve') await draftResolve($, pr)
  else if (kind === 'fix') await collectFailure($, pr)
  else await collectThreads($, pr)
  const ask = (await readRows($, kind))[pr.url]
  // A draft that landed stays where it is (a part that wouldn't come is said on the card).
  const hasFailed = ask?.error !== undefined && ask.done === undefined
  if (ask?.asking !== undefined || hasFailed) await $.ui.open({ ...OPEN, focus: true })
}

// COLORFGBG's light or dark, once read (null: it doesn't say).
let colourMode: Mode | null | undefined

// Light or dark: Claude Code's own theme row says, unless it is `auto` or a custom one; then the
// terminal's COLORFGBG, where it sets one; else dark.
async function readMode($: EngineInterface): Promise<Mode> {
  try {
    const said = modeOf((await $.config.list()).find(row => row.key === 'theme')?.value)
    if (said !== null) return said
  } catch {
    // No rows to read here: the terminal's word, or dark.
  }
  // The terminal's word doesn't change while the session runs: read it once.
  if (colourMode === undefined) {
    try {
      colourMode = modeOfColours(await $.env.get('COLORFGBG'))
    } catch {
      colourMode = null
    }
  }

  return colourMode ?? 'dark'
}

// The terminal's colours, as Claude Code decides them from its environment (theme.ts's depthOf):
// read once, since they don't change while the session runs; redrawn when they are 256.
async function readDepth($: EngineInterface) {
  try {
    const [tmux, tmuxTruecolor, colorterm, term, program] = await Promise.all([
      $.env.get('TMUX'),
      $.env.get('CLAUDE_CODE_TMUX_TRUECOLOR'),
      $.env.get('COLORTERM'),
      $.env.get('TERM'),
      $.env.get('TERM_PROGRAM'),
    ])
    const seen = depthOf({ TMUX: tmux, CLAUDE_CODE_TMUX_TRUECOLOR: tmuxTruecolor, COLORTERM: colorterm, TERM: term, TERM_PROGRAM: program })
    if (seen === depth) return
    depth = seen
    $.ui.invalidate('ui.render')
  } catch {
    // Nothing to read here: the 24-bit paint stays.
  }
}

// Redraws the pane and the band when the light or dark has changed.
async function followMode($: EngineInterface, next?: Mode) {
  const seen = next ?? (await readMode($))
  if (seen === mode) return
  mode = seen
  $.ui.invalidate('ui.render')
}

// This mod's `theme` row in /config: `pr-watch.theme`, or with the marketplace after the name.
const THEME_ROW = /^pr-watch(?:@[\w.-]+)?\.theme$/

// `/pr-watch theme <name>`: the look changes at once; then it is written to the `theme` option
// where /config has the row and lets a plugin set it, so every session follows. Where it
// doesn't, the pick holds for this session.
async function pickTheme($: EngineInterface, name: ThemeName): Promise<string> {
  themeName = name
  $.ui.invalidate('ui.render')
  let why = ''
  try {
    const row = (await $.config.list()).find(r => THEME_ROW.test(r.key))
    if (row !== undefined) {
      const done = await $.config.set({ key: row.key, value: name })
      if (done.deny === undefined) return `pr-watch theme: ${name}. Saved in /config, so every session follows.`
      why = ` (/config said: ${done.deny})`
    }
  } catch {
    // No row to write here.
  }

  return `pr-watch theme: ${name}, for this session${why}. Pick it in /config (pr-watch, theme) to keep it for every session.`
}

async function refreshStatus($: EngineInterface) {
  const list = await read($, prs)
  const focus = list.find(isActive) ?? list[list.length - 1]
  $.ui.status(focus === undefined ? undefined : `PR #${focus.number}: ${focus.headline}`)
}

// `isExplicit`: the person named the PR, so it is watched whatever its state. Otherwise (a PR
// Claude acted on) only an open PR the person hasn't stopped watching is: GitHub is asked
// first, so a merged or closed one never gets a card; the person can still watch it by name.
async function track($: EngineInterface, refs: PrRef[], isExplicit = false) {
  const known = new Set((await read($, prs)).map(pr => pr.url))
  if (isExplicit) await update($, dropped, list => list.filter(url => !refs.some(ref => ref.url === url)))
  else for (const url of await read($, dropped)) known.add(url)
  const fresh = refs.filter(ref => !known.has(ref.url))
  if (fresh.length === 0) return
  if (isExplicit) {
    await show($, fresh)
    void fetchNow($, fresh).catch(() => undefined)

    return
  }
  void (async () => {
    const shared = await publish($, fresh, await $.clock.now())
    const open = fresh.filter(ref => shared.prs[ref.url]?.pr?.state === 'OPEN')
    if (open.length === 0) return
    await show($, open)
    await apply($, shared)
  })().catch(() => undefined)
}

async function show($: EngineInterface, fresh: PrRef[]) {
  await update($, prs, list => [...list, ...fresh.filter(r => !list.some(pr => pr.url === r.url)).map(placeholder)])
  // The band shows a new PR by itself; the pane waits to be asked for.
  if (layout === 'band') {
    await syncAnimation($)

    return
  }
  const opened = await $.ui.open(OPEN)
  if (!opened.isPlaced && !hasHinted) {
    hasHinted = true
    $.ui.toast(`pr-watch: tracking PR #${fresh[0]?.number} — run /pr-watch to open the panel`)
  }
  await syncAnimation($)
}

// Runs the spinner and shimmer only while some PR is moving; blits skip the render pass.
async function syncAnimation($: EngineInterface) {
  const isMoving = (await read($, prs)).some(pr => full(pr).isMoving)
  if (isMoving && anim === null) {
    anim = $.clock.every(FRAME_MS, async () => {
      frame += 1
      const moving = (await read($, prs)).map(full).filter(pr => pr.isMoving)
      if (moving.length === 0) {
        anim?.cancel()
        anim = null

        return
      }
      const paint = look()
      const { spin } = paint.theme
      const { spinner } = paint.variant.raster
      for (const pr of moving) {
        const art = artCards.get(pr.url)
        if (art !== undefined) {
          // A banner card: the whole card is one Raster, its wave, glint and spinner in it. Its
          // size is the one drawn, so a refusal means the pane isn't mounted.
          if (isPaneDrawn && frame % ART_EVERY === 0) {
            void $.ui.blit({ requestId: PANE, key: `art:${pr.url}`, cells: artCells(art, frame) }).then(done => {
              if ('deny' in done) isPaneDrawn = false
            })
          }
        } else if (isPaneDrawn) {
          // The spinner is one cell, so a refusal means the pane isn't mounted, never another size.
          const cells = spinnerCell(frame + pr.number, spinner, spin, rasterBg(paint, pr.tone))
          void $.ui.blit({ requestId: PANE, key: `spin:${pr.url}`, cells }).then(done => {
            if ('deny' in done) isPaneDrawn = false
          })
        }
        if (layout === 'band' && bandId !== null && bandSpins.has(pr.url)) {
          void $.ui.blit({ requestId: bandId, key: `band-spin:${pr.url}`, cells: spinnerCell(frame + pr.number, spinner, spin) })
        }
        const width = barWidth.get(pr.url)
        if (isPaneDrawn && art === undefined && width !== undefined && pr.checks.pending > 0) {
          void $.ui.blit({ requestId: PANE, key: `bar:${pr.url}`, cells: barCells(pr.checks, width, frame, barPaint(paint, pr.tone)) })
        }
      }
    })
  } else if (!isMoving && anim !== null) {
    anim.cancel()
    anim = null
  }
}

async function dismiss($: EngineInterface, url: string) {
  await update($, prs, list => list.filter(pr => pr.url !== url))
  await update($, dropped, list => (list.includes(url) ? list : [...list, url]))
  await setAsk($, url, null)
  await setHandoff($, 'fix', url, null)
  await setHandoff($, 'address', url, null)
  await setHandoff($, 'resolve', url, null)
  barWidth.delete(url)
  artCards.delete(url)
  await syncAnimation($)
  if ((await read($, prs)).length === 0) await $.ui.close({ id: PANE })
  await refreshStatus($)
}

async function dismissFinished($: EngineInterface) {
  for (const pr of await read($, prs)) {
    if (!isActive(pr)) await dismiss($, pr.url)
  }
}

// After a /clear: put the watched PRs back if the session's state started over, and seat the
// pane again, since no session.start follows to do it.
async function restore($: EngineInterface, carried: TrackedPr[]) {
  const list = await read($, prs)
  if (list.length === 0) await update($, prs, () => carried)
  if (layout === 'pane' && !(await $.ui.panes()).some(pane => pane.id === PANE)) await $.ui.open(OPEN)
  await refreshStatus($)
  await syncAnimation($)
}

export const register: Register = (on, options) => {
  pace.fastMs = seconds(options.poll_active_seconds, 20_000)
  pace.slowMs = seconds(options.poll_idle_seconds, 60_000)
  isNoticeOn = options.update_notice !== false
  layout = layoutOf(options.layout)
  themeName = optionTheme(options.theme)
  ownVersion = undefined
  isPaneDrawn = false
  toldVersion = null

  // A change in /config applies from the next round, no reload needed.
  on('config.set', async ($, e, next) => {
    const done = await next(e)
    if (done.deny !== undefined) return done
    // Claude Code's own theme: the looks follow it to light or dark.
    if (e.key === 'theme') {
      await followMode($, modeOf(done.value) ?? undefined)

      return done
    }
    const field = e.key.match(/^pr-watch(?:@[\w.-]+)?\.(poll_active_seconds|poll_idle_seconds|update_notice|layout|theme)$/)?.[1]
    if (field === undefined) return done
    if (field === 'poll_active_seconds') pace.fastMs = seconds(done.value, pace.fastMs)
    else if (field === 'poll_idle_seconds') pace.slowMs = seconds(done.value, pace.slowMs)
    else if (field === 'layout') {
      // The band draws, or stops, at once. On the band the pane closes (the room was wanted
      // back); back on the pane, it opens as a session start would open it.
      layout = layoutOf(done.value)
      bandSpins.clear()
      $.ui.invalidate('ui.render')
      if (layout === 'band') void $.ui.close({ id: PANE })
      else if ((await read($, prs)).length > 0) void $.ui.open(OPEN)
    } else if (field === 'theme') {
      // The pane and the band take the new look at once.
      themeName = optionTheme(done.value)
      $.ui.invalidate('ui.render')
    } else {
      isNoticeOn = done.value !== false
      // Off: the footer's note goes now; on, it comes back with the next tick.
      if (!isNoticeOn) await update($, newer, () => null)
    }

    return done
  }).catch(($, e, next) => next(e))

  on('session.start', async ($, e, next) => {
    await $.command.register({
      name: 'pr-watch',
      description: 'Open the PR pane; /pr-watch <url> watches a PR, /pr-watch stop <number|url|all> stops, /pr-watch theme <name> restyles it',
      argumentHint: '[<pr url> | stop <number|url|all> | theme [<name>]]',
    })
    // Off the start path: the pane redraws if it turns out light, or painted in 256 colours.
    void followMode($)
    void readDepth($)
    await $.tool.register({
      name: 'watch',
      description:
        "Adds GitHub pull requests to the user's PR watch pane, which shows their live CI, review and merge status. " +
        'Call it only after the user agreed to watch the PR.',
      inputSchema: {
        type: 'object',
        properties: { urls: { type: 'array', items: { type: 'string' }, description: 'PR URLs: https://github.com/<owner>/<repo>/pull/<n>' } },
        required: ['urls'],
      },
    })
    // One tick for the session; the round itself is shared by every session on the machine.
    $.clock.every(10_000, async () => {
      const list = await read($, prs)
      // A light or dark picked where no config.set is raised (the theme dialog) shows by the next
      // tick, merged and closed cards too.
      if (list.length > 0) await followMode($)
      const active = list.map(full).filter(isActive)
      if (active.length === 0 || isTicking) return
      isTicking = true
      try {
        await apply($, await tick($, active.map(refOf), await $.clock.now()))
      } finally {
        isTicking = false
      }
    })
    if ((await read($, prs)).length > 0) {
      if (layout === 'pane') void $.ui.open(OPEN)
      await refreshStatus($)
      await syncAnimation($)
    }

    return next(e)
  })

  // A /clear ends the conversation with no session.start after it; the engine may close the
  // pane and start the state over, so carry the PRs across and restore once it has settled.
  on('session.end', async ($, e, next) => {
    if (e.reason !== 'clear') return next(e)
    const carried = await read($, prs)
    const ended = await next(e)
    if (carried.length > 0) $.clock.after(500, () => void restore($, carried))

    return ended
  }).catch(($, e, next) => next(e))

  on('command.run', { command: 'pr-watch' }, async ($, e) => {
    // `/pr-watch theme neon` restyles the pane and the band; `/pr-watch theme` lists the looks.
    const asked = e.args.trim().match(/^theme(?:\s+(.*))?$/i)
    if (asked !== null) {
      const wanted = asked[1]?.trim() ?? ''
      if (wanted === '') return { text: themeListing(themeName) }
      const name = themeOf(wanted)

      return { text: name === null ? unknownTheme(wanted) : await pickTheme($, name) }
    }
    // `/pr-watch stop 2344` (or a URL, or `all`) stops watching without opening the pane.
    const stop = e.args.trim().match(/^(?:stop|unwatch|rm)\s+(.+)$/i)
    if (stop !== null) {
      const wanted = stop[1]?.trim() ?? ''
      const urls = new Set(findPrUrls(wanted).map(r => r.url))
      const nums = new Set(wanted.split(/[\s,]+/).map(w => Number(w.replace(/^#/, ''))).filter(n => n > 0))
      const gone = (await read($, prs)).filter(pr => wanted === 'all' || urls.has(pr.url) || nums.has(pr.number))
      for (const pr of gone) await dismiss($, pr.url)

      return { text: gone.length > 0 ? `Stopped watching ${gone.map(pr => `#${pr.number}`).join(', ')}.` : `Not watching ${wanted}.` }
    }
    const refs = findPrUrls(e.args)
    await $.ui.open(OPEN)
    if (refs.length > 0) {
      await track($, refs, true)

      return { text: `Watching ${refs.map(r => `#${r.number}`).join(', ')}.` }
    }

    return { text: 'PR panel opened.' }
  })

  // Watching is harmless and the user asked for it, so the tool needs no permission prompt.
  on('tool.check', { tool: WATCH_TOOL }, () => ({ decision: 'allow' }))

  on('tool.call', async ($, e, next) => {
    if (String(e.tool) === WATCH_TOOL) {
      // The mod's own tool. (Its name is not in the generated tool types, hence no matcher.)
      const input = e as unknown as { urls?: unknown }
      const urls = Array.isArray(input.urls) ? input.urls.filter((u): u is string => typeof u === 'string') : []
      const refs = findPrUrls(urls.join(' '))
      if (refs.length === 0) return { result: 'No GitHub pull request URL given; expected https://github.com/<owner>/<repo>/pull/<n>.' }
      await track($, refs, true)

      return { result: `Watching ${refs.map(r => `#${r.number}`).join(', ')} in the PR pane.` }
    }
    const ran = await next(e)
    if (ran.deny !== undefined) return ran

    // A PR Claude acted on is watched without asking; one it only read is offered.
    const touch = touched(String(e.tool), e as unknown as Record<string, unknown>, ran.text ?? '')
    let { acted, read: seen } = touch
    if (touch.resolve !== null && e.tool === 'Bash' && !ran.isError) {
      // `git push`, `gh pr checks`: ask gh which PR that was, from the same directory.
      const num = shellCode(e.command).match(/\bgh\s+pr\s+[\w-]+\s+#?(\d+)\b/)?.[1]
      const res = await gh($, ['pr', 'view', ...(num ? [num] : []), '--json', 'url', '-q', '.url'], null, cwdOf(e.command))
      const refs = res.exitCode === 0 ? findPrUrls(res.stdout) : []
      if (touch.resolve === 'acted') acted = refs
      else seen = refs
    }
    if (acted.length > 0) await track($, acted)

    const watched = new Set([...(await read($, prs)).map(pr => pr.url), ...acted.map(ref => ref.url)])
    const asked = new Set([...(await read($, suggested)), ...(await read($, dropped))])
    const fresh = seen.filter(ref => !watched.has(ref.url) && !asked.has(ref.url))
    if (fresh.length === 0) return ran
    await update($, suggested, list => [...list, ...fresh.map(ref => ref.url)])
    const named = fresh.map(ref => `${ref.repo}#${ref.number} (${ref.url})`).join(', ')

    return {
      ...ran,
      context: [
        ...(ran.context ?? []),
        `pr-watch: you read ${named}, which the user's live PR pane is not watching. Only if it looks like the user's ` +
          `own work in this session (a PR they are shepherding, not background reading or research), ask in one short ` +
          `line whether to add it to the PR pane, and call ${WATCH_TOOL} with its URL if they agree. Otherwise say nothing ` +
          `about it. Either way, do not bring it up again.`,
      ],
    }
  }).catch(($, e, next) => next(e))

  // PR links the person pastes into a prompt.
  on('prompt.submit', async ($, e, next) => {
    const refs = findPrUrls(e.text)
    if (refs.length > 0) await track($, refs, true)

    return next(e)
  }).catch(($, e, next) => next(e))

  on('ui.render', { component: 'Pane', requestId: PANE }, async ($, e) => {
    isPaneDrawn = true
    const { Box, Text, Button, Link } = $.ui.resolve(e)
    const Raster = e.surface === 'terminal' ? $.ui.resolve(e).Raster : null
    const list = (await read($, prs)).map(full)
    const at = await read($, now)
    const asks = await read($, merging)
    const refusedBy = await read($, refused)
    const [fixRows, addressRows, resolveRows] = await Promise.all([readRows($, 'fix'), readRows($, 'address'), readRows($, 'resolve')])
    const handoffs = { fix: fixRows, address: addressRows, resolve: resolveRows }
    const upgrade = isNoticeOn ? await read($, newer) : null
    const budget = await read($, limits)
    const isHelpOpen = await read($, ghHelp)
    const width = Math.max(24, e.props.bodyColumns)
    const inner = width - 4 // card border + padding (or, borderless, the padding standing for it)
    const open = list.filter(isActive).length
    const done = list.length - open
    // The theme's look (theme.ts): every colour below is the theme's, by tone.
    const paint = look()
    const { theme, variant } = paint
    const ink = (tone: TrackedPr['tone'], role?: Role) => inkOf(paint, tone, role)
    // This drawing's Rasters are the ones the animation blits: what an earlier one drew is gone.
    if (Raster !== null) {
      barWidth.clear()
      artCards.clear()
    }

    const header = (
      <Box flexDirection="column" marginBottom={1}>
        <Box justifyContent="space-between">
          <Text bold>
            <Text color={ink('merged')}>⎇</Text> Pull requests
          </Text>
          <Text>
            {open > 0 && (
              <Text>
                <Text color={ink('warning')} bold>{open}</Text>
                <Text dimColor> open</Text>
              </Text>
            )}
            {open > 0 && done > 0 && <Text dimColor> · </Text>}
            {done > 0 && (
              <Text>
                <Text color={ink('merged')} bold>{done}</Text>
                <Text dimColor> done</Text>
              </Text>
            )}
          </Text>
        </Box>
        {Raster !== null ? (
          <Raster key="rule" columns={width} rows={1} cells={ruleCells(width, variant.raster.rule, theme.rule)} />
        ) : (
          <Text color={ink('merged')} dimColor>{ruleText(width, theme.rule)}</Text>
        )}
      </Box>
    )

    // The frame's close mark sits on its top row, which some terminal overlays cover. Closing
    // hides the pane only: the PRs stay watched, and a new PR or /pr-watch brings it back.
    const closeButton = <Button key="close" label="Close" hotkey="x" plain dimColor onPress={() => $.ui.close({ id: PANE })} />

    // A newer pr-watch, under the footer's buttons: "↑ pr-watch 0.9.0 available · git pull in …".
    const notice = upgrade !== null && (
      <Box key="newer" flexDirection="column">
        {noticeLines(upgrade.version, upgrade.how, width).map((line, i) => (
          <Text key={`newer:${i}`} dimColor wrap="wrap">
            {line}
          </Text>
        ))}
      </Box>
    )

    // GitHub's budget per login, a meter each: "gh  octocat ▀▀▀▀▀▀▀▀▀▀▀▀  94% ↻42m  ?", the bar's
    // top half GraphQL's points, its bottom half REST's requests, coloured healthy, low, spent;
    // the pause above while a limit holds the poll. `?` opens what it all means beneath (the
    // pointer on it shows the same over the pane).
    const meter = ghMeter(budget, at)
    const toneInk = (tone: 'warning' | 'error' | null) => (tone === null ? undefined : ink(tone))
    const ghLine = (() => {
      if (meter === null) return null
      const refillW = Math.max(0, ...meter.meters.map(m => (m.refill === null ? 0 : m.refill.length + 1)))
      // The login gives way first on a narrow pane, so the row still fits beside a 4-cell bar.
      const loginW = Math.min(14, Math.max(1, width - 3 - 1 - 4 - 5 - refillW - 3), Math.max(0, ...meter.meters.map(m => m.login.length)))
      // "gh " + login + " " + bar + " 100%" + refill + "  ?"
      const meterW = Math.max(4, Math.min(16, width - 3 - loginW - 1 - 5 - refillW - 3))
      // The bar is the lower of GraphQL's and REST's shares, the one the % is of (rate.ts).
      const textBar = (bar: Bar) => {
        const filled = Math.round(Math.min(1, bar.share) * meterW)

        return (
          <Text>
            <Text color={toneInk(bar.tone) ?? ink('success')}>{'━'.repeat(filled)}</Text>
            <Text dimColor>{'─'.repeat(meterW - filled)}</Text>
          </Text>
        )
      }
      const help = (
        <Button key="gh-help" label="?" plain dimColor={!isHelpOpen} hover={{ scope: HELP_SCOPE, bold: true }} onPress={() => update($, ghHelp, isOpen => !isOpen)} />
      )
      const rows = [
        ...(meter.pause === null
          ? []
          : [
              <Text key="gh-pause" color={ink('warning')} wrap="truncate-end">
                {meter.pause}
              </Text>,
            ]),
        ...meter.meters.map((m, i) => (
          <Box key={`gh:${m.login}`}>
            <Text dimColor>{i === 0 ? 'gh ' : '   '}</Text>
            <Text dimColor>{(m.login.length > loginW ? `${m.login.slice(0, loginW - 1)}…` : m.login).padEnd(loginW)} </Text>
            {Raster !== null ? (
              <Raster key={`gh-bar:${m.login}`} columns={meterW} rows={1} cells={meterCells({ share: m.bar.share, health: m.bar.tone }, meterW, variant.raster)} />
            ) : (
              textBar(m.bar)
            )}
            <Text color={toneInk(m.tone)} dimColor={m.tone === null} bold={m.tone !== null}>
              {` ${m.percent.padStart(4)}`}
            </Text>
            {m.refill !== null && <Text color={toneInk(m.tone)} dimColor>{` ${m.refill}`}</Text>}
          </Box>
        )),
      ]

      return (
        <Box key="gh" flexDirection="column">
          {rows.map((row, i) => (
            <Box key={`gh-row:${i}`} justifyContent="space-between">
              {row}
              {i === 0 && help}
            </Box>
          ))}
          {isHelpOpen ? (
            <Box key="gh-explain" flexDirection="column" marginTop={1}>
              {GH_HELP.map((line, i) => (
                <Text key={`gh-explain:${i}`} dimColor wrap="wrap">
                  {line}
                </Text>
              ))}
            </Box>
          ) : (
            // Under the pointer only: the same words in a note over the rows above the meter.
            <Box
              position="absolute"
              bottom={rows.length}
              left={0}
              width={width}
              display="none"
              hover={{ scope: HELP_SCOPE, display: 'flex' }}
              flexDirection="column"
              borderStyle="round"
              borderColor={ink('subtle')}
              backgroundColor={variant.panel}
              paddingX={1}
            >
              {GH_HELP.map((line, i) => (
                <Text key={`gh-note:${i}`} wrap="wrap">
                  {line}
                </Text>
              ))}
            </Box>
          )}
        </Box>
      )
    })()

    if (list.length === 0) {
      return (
        <Box flexDirection="column">
          {header}
          <Box flexDirection="column" alignItems="center" paddingY={1}>
            <Text color={ink('merged')}>╭─────╮</Text>
            <Text color={ink('merged')}>│ ⎇ ◌ │</Text>
            <Text color={ink('merged')}>╰─────╯</Text>
            <Text bold>Nothing to watch yet</Text>
            <Text dimColor>PRs appear when Claude runs gh pr …</Text>
            <Text dimColor>or with /pr-watch &lt;url&gt;</Text>
          </Box>
          <Box>{closeButton}</Box>
          {ghLine}
          {notice}
        </Box>
      )
    }

    // Merge, or auto-merge, behind a question; or auto-merge's state with a way to turn it off.
    const mergeRow = (pr: TrackedPr) => {
      const ask = asks[pr.url] ?? {}
      const methods = pr.merge.methods.filter(m => !(refusedBy[pr.repo] ?? []).includes(m))
      const method = pickMethod(methods, ask.method)
      const error = ask.error !== undefined && (
        <Text color={ink('error')} wrap="wrap">
          {`✗ ${ask.error}`}
        </Text>
      )
      if (isBusy(ask.busy, at)) return <Text color={ink('suggestion')}>{`◌ ${BUSY[ask.busy.action]}`}</Text>
      if (pr.merge.isAuto) {
        return (
          <Box flexDirection="column">
            {error}
            <Box justifyContent="space-between">
              <Text color={ink('suggestion')}>{`⇢ Auto-merge on${pr.merge.autoMethod === null ? '' : ` · ${pr.merge.autoMethod}`}`}</Text>
              {pr.merge.canCancelAuto && (
                <Button
                  key={`cancel-auto:${pr.url}`}
                  label="Cancel auto-merge"
                  onPress={() => runMerge($, pr, { action: 'cancel-auto' })}
                />
              )}
            </Box>
          </Box>
        )
      }
      const action = pr.merge.canMerge ? 'merge' : pr.merge.canAuto ? 'auto' : null
      if (method === null || action === null) return error || null
      if (ask.asking === action) {
        return (
          <Box flexDirection="column">
            <Text bold wrap="wrap">
              {mergeQuestion(method, pr, action === 'auto')}
            </Text>
            <Box gap={2}>
              <Button key={`confirm:${pr.url}`} label="Confirm" hotkey="y" plain onPress={() => runMerge($, pr, { action, method })} />
              <Button key={`cancel:${pr.url}`} label="Cancel" hotkey="n" plain dimColor onPress={() => setAsk($, pr.url, { method })} />
            </Box>
          </Box>
        )
      }

      return (
        <Box flexDirection="column">
          {error}
          <Box gap={2}>
            <Button
              key={`${action}:${pr.url}`}
              label={action === 'merge' ? MERGE_LABEL[method] : `Auto-merge · ${method}`}
              variant="primary"
              onPress={async () => {
                await oneQuestion($)
                await setAsk($, pr.url, { method, asking: action })
              }}
            />
            {methods.length > 1 && (
              <Button
                key={`method:${pr.url}`}
                label={`⇄ ${nextMethod(methods, method)}`}
                plain
                dimColor
                onPress={() => setAsk($, pr.url, { method: nextMethod(methods, method) })}
              />
            )}
          </Box>
        </Box>
      )
    }

    // The hand-offs a card offers, what came of each one's last press above, and their buttons
    // side by side below; a question, where no prompt box took the draft, stands for its button.
    const handoffRow = (pr: TrackedPr) => {
      const offered = HANDOFFS.filter(handoff => handoff.isOffered(pr))
      if (offered.length === 0) return null
      const notes = []
      const buttons = []
      for (const handoff of offered) {
        const ask = handoffs[handoff.id][pr.url] ?? {}
        if (isBusy(ask.busy, at)) {
          notes.push(<Text key={`${handoff.id}-busy:${pr.url}`} color={ink('suggestion')}>{`◌ ${handoff.busy}`}</Text>)
          continue
        }
        if (ask.error !== undefined) {
          notes.push(
            <Text key={`${handoff.id}-error:${pr.url}`} color={ink('error')} wrap="wrap">
              {`✗ ${ask.error}`}
            </Text>,
          )
        }
        const text = ask.asking
        if (text !== undefined) {
          notes.push(
            <Box key={`${handoff.id}-ask:${pr.url}`} flexDirection="column">
              <Text bold wrap="wrap">
                {`Send ${handoff.what} of #${pr.number} to Claude?`}
              </Text>
              <Box gap={2}>
                <Button key={`${handoff.id}-confirm:${pr.url}`} label="Confirm" hotkey="y" plain onPress={() => sendHandoff($, handoff.id, pr, text)} />
                <Button key={`${handoff.id}-cancel:${pr.url}`} label="Cancel" hotkey="n" plain dimColor onPress={() => setHandoff($, handoff.id, pr.url, null)} />
              </Box>
            </Box>,
          )
          continue
        }
        if (ask.done !== undefined) {
          notes.push(
            <Text key={`${handoff.id}-done:${pr.url}`} color={ink('success')} wrap="wrap">
              {`✓ ${ask.done}`}
            </Text>,
          )
        }
        // Not a HANDOFFS field: the loader wants $ passed only where a collector is named.
        buttons.push(
          <Button
            key={`${handoff.id}:${pr.url}`}
            label={handoff.label}
            onPress={() => (handoff.id === 'resolve' ? draftResolve($, pr) : handoff.id === 'fix' ? collectFailure($, pr) : collectThreads($, pr))}
          />,
        )
      }

      return (
        <Box flexDirection="column">
          {notes}
          {buttons.length > 0 && (
            <Box columnGap={2} flexWrap="wrap">
              {buttons}
            </Box>
          )}
        </Box>
      )
    }

    // Who reviewed, and the threads still open: "✓ alice approved · 💬 nadav commented" and
    // "3 unresolved threads (1 outdated)", each only when there is something to say.
    const reviewLines = (reviews: Reviews, threads: string) => {
      const pieces = verdictLine(reviews, inner)

      return (
        <Box flexDirection="column">
          {pieces.length > 0 && (
            <Text wrap="truncate-end">
              {pieces.map((piece, i) => {
                // A pending request, or "+2 more": dim.
                const tone = piece.verdict === null ? undefined : VERDICT_COLOUR[piece.verdict]
                const colour = tone === undefined ? undefined : ink(tone)

                return (
                  <Text>
                    {i > 0 && <Text dimColor> · </Text>}
                    <Text color={colour} dimColor={colour === undefined}>
                      {piece.text}
                    </Text>
                  </Text>
                )
              })}
            </Text>
          )}
          {threads !== '' && (
            <Text color={ink('warning')} wrap="truncate-end">
              {threads}
            </Text>
          )}
        </Box>
      )
    }

    // A card's last row: its link, how old its data is, and its dismiss.
    const linkRow = (pr: TrackedPr) => (
      <Box justifyContent="space-between">
        <Text>
          {/* Its own label: with "open ↗" on every card, a click on any of them opened the last.
              The repo too when another card shares the number (acme/app#7, acme/api#7). */}
          <Link href={pr.url} label={`open ${list.some(o => o.url !== pr.url && o.number === pr.number) ? pr.repo : ''}#${pr.number} ↗`} />
          {pr.checkedAt !== null && <Text dimColor> · checked {ago(at - pr.checkedAt)}</Text>}
        </Text>
        <Button
          key={`dismiss:${pr.url}`}
          label={isActive(pr) ? '✕' : 'Dismiss'}
          plain={isActive(pr) ? true : undefined}
          role="dismiss"
          variant={isActive(pr) ? undefined : 'primary'}
          dimColor={isActive(pr)}
          onPress={() => dismiss($, pr.url)}
        />
      </Box>
    )

    const card = (pr: TrackedPr) => {
      const step = stepIndex(pr.stage)
      const isMerged = pr.stage === 'merged'
      const doneTone = isMerged ? 'merged' : 'success'
      const doneColour = ink(doneTone, 'steps')
      const toneColour = ink(pr.tone)
      const pill = pillOf(paint, pr.tone, pr.pill)
      const icon = [...pr.pill][0] ?? '●'
      // Labels take 25 cells and the glyphs/gaps 10 (one more for a wide icon, 💬); connectors
      // share what is left.
      const link = Math.min(6, Math.max(1, Math.floor((inner - 34 - cells(icon)) / 3)))
      const label = `${pr.checks.passed}/${pr.checks.total}`
      const barW = Math.max(6, inner - label.length - 1)
      const hasBar = (pr.state === 'OPEN' && pr.checks.total > 0) || isMerged
      const threads = pr.state === 'OPEN' && pr.reviews !== null ? threadsBeside(pr.headline, pr.reviews) : { line: '', note: '' }

      if (hasBar && !isMerged) barWidth.set(pr.url, barW)

      const glyph = (i: number) => {
        if (i < step) return <Text color={doneColour}>✓</Text>
        if (i > step) return <Text dimColor>○</Text>
        if (pr.isMoving && Raster !== null) {
          const cells = spinnerCell(frame + pr.number, variant.raster.spinner, theme.spin, rasterBg(paint, pr.tone))

          return <Raster key={`spin:${pr.url}`} columns={1} rows={1} cells={cells} />
        }

        return <Text color={ink(pr.tone, 'steps')} bold>{pr.isMoving ? '◌' : icon}</Text>
      }

      return (
        <Box key={`card:${pr.url}`} flexDirection="column" {...cardBox(paint, pr.tone)}>
          <Box justifyContent="space-between">
            <Text {...pill.props}>
              {pill.open !== '' && pill.open}
              {pill.text}
              {pill.close !== '' && pill.close}
            </Text>
            <Text dimColor wrap="truncate-start">
              {pr.repo}
            </Text>
          </Box>

          <Box>
            <Text wrap="truncate-end">
              <Text color={ink(pr.tone, 'number')} bold>
                #{pr.number}
              </Text>{' '}
              <Text bold>{pr.title || 'Loading…'}</Text>
            </Text>
          </Box>
          {pr.branch !== '' && (
            <Text wrap="truncate-end">
              <Text color={ink('suggestion', 'branch')}>{pr.branch}</Text>
              <Text dimColor> → </Text>
              <Text dimColor>{pr.base}</Text>
              <Text dimColor>{'  '}</Text>
              <Text color={ink('success', 'diff')}>+{pr.additions}</Text> <Text color={ink('error', 'diff')}>−{pr.deletions}</Text>
              {pr.author !== '' && <Text dimColor>{'  '}@{pr.author}</Text>}
            </Text>
          )}

          <Box marginTop={1}>
            {STEPS.map((name, i) => (
              <Box>
                {glyph(i)}
                <Text
                  color={i < step ? doneColour : i === step ? ink(pr.tone, 'steps') : undefined}
                  dimColor={i > step}
                  bold={i === step}
                >
                  {' '}
                  {name}
                </Text>
                {i < STEPS.length - 1 && (
                  <Text color={i < step ? doneColour : undefined} dimColor={i >= step}>
                    {' '}
                    {(i < step ? '━' : '┈').repeat(link)}{' '}
                  </Text>
                )}
              </Box>
            ))}
          </Box>

          {hasBar && (
            <Box>
              {Raster !== null ? (
                <Raster
                  key={isMerged ? `done:${pr.url}` : `bar:${pr.url}`}
                  columns={barW}
                  rows={1}
                  cells={
                    isMerged
                      ? fullBarCells(barW, variant.raster.mauve, rasterBg(paint, pr.tone))
                      : barCells(pr.checks, barW, frame, barPaint(paint, pr.tone))
                  }
                />
              ) : (
                <Text>
                  {(isMerged ? [{ text: '━'.repeat(barW), colour: 'merged' as const }] : barRuns(pr.checks, barW)).map(r => (
                    <Text color={ink(r.colour)}>{r.text}</Text>
                  ))}
                </Text>
              )}
              {!isMerged && <Text dimColor> {label}</Text>}
            </Box>
          )}

          <Text color={toneColour} wrap="wrap">
            {pr.headline}
            {isMerged && pr.mergedAt !== null && <Text dimColor> · {ago(at - Date.parse(pr.mergedAt))}</Text>}
            {threads.note !== '' && <Text dimColor> · {threads.note}</Text>}
          </Text>
          {pr.checks.failing.length > 2 &&
            pr.checks.failing.slice(2, 5).map(name => (
              <Text color={ink('error')} dimColor wrap="truncate-end">
                {'  '}✗ {name}
              </Text>
            ))}
          {pr.state === 'OPEN' && pr.reviews !== null && reviewLines(pr.reviews, threads.line)}
          {pr.error !== null && (
            <Text color={ink('error')} wrap="truncate-end">
              ⚠ {pr.error}
            </Text>
          )}
          {pr.state === 'OPEN' && handoffRow(pr)}
          {pr.state === 'OPEN' && mergeRow(pr)}

          {linkRow(pr)}
        </Box>
      )
    }

    // A banner card (theme.ts's `layout`): its words in one Raster's cells over the art
    // (art.ts), the PR's progress the art itself; then its hand-offs, merge row and links on rows
    // of their own, over the card's glass, so every press works as on a stacked card.
    const bannerCard = (pr: TrackedPr) => {
      const art = variant.art
      if (art === null || theme.art === null) return card(pr)
      const glass = variant.tones[pr.tone].fill ?? undefined
      const below = (
        <Box flexDirection="column" paddingX={1} backgroundColor={glass}>
          {pr.state === 'OPEN' && handoffRow(pr)}
          {pr.state === 'OPEN' && mergeRow(pr)}
          {linkRow(pr)}
        </Box>
      )
      if (Raster !== null) {
        const model = artCard(theme.art, pr, at, width, art, theme.spin)
        artCards.set(pr.url, model)

        return (
          <Box key={`card:${pr.url}`} flexDirection="column" marginBottom={theme.card.gap}>
            <Raster key={`art:${pr.url}`} columns={width} rows={model.grid.length} cells={artCells(model, frame)} />
            {below}
          </Box>
        )
      }
      // No Raster on this surface: the same rows as text, on the glass.
      const run = (seg: Seg, i: number) => {
        if (seg.role === 'badge') {
          return (
            <Text key={`seg:${i}`} {...pillOf(paint, pr.tone, '').props}>
              {seg.text}
            </Text>
          )
        }
        const tone = roleTone(seg.role, pr.tone)

        return (
          <Text key={`seg:${i}`} color={tone === null ? undefined : ink(tone)} dimColor={seg.role === 'dim'} bold={seg.role === 'strong'}>
            {seg.text}
          </Text>
        )
      }

      return (
        <Box key={`card:${pr.url}`} flexDirection="column" marginBottom={theme.card.gap} backgroundColor={glass}>
          <Box flexDirection="column" paddingX={1}>
            {bannerLines(pr, at, width).map((line, i) => (
              <Box key={`line:${pr.url}:${i}`} justifyContent="space-between">
                <Text wrap="truncate-end">{line.left.length > 0 ? line.left.map(run) : ' '}</Text>
                {(line.right ?? []).length > 0 && <Text>{(line.right ?? []).map(run)}</Text>}
              </Box>
            ))}
          </Box>
          {below}
        </Box>
      )
    }

    return (
      <Box flexDirection="column">
        {header}
        {list.flatMap((pr, i) => {
          // The theme's layout: a stacked card, or a banner card.
          const drawn = theme.layout === 'banner' ? bannerCard(pr) : card(pr)

          // A theme without boxes draws a thin rule between the PRs instead.
          return i > 0 && theme.card.separator !== null
            ? [
                <Box key={`sep:${pr.url}`} marginBottom={theme.card.gap}>
                  <Text dimColor>{theme.card.separator.repeat(width)}</Text>
                </Box>,
                drawn,
              ]
            : [drawn]
        })}
        <Box gap={2}>
          <Button key="refresh" label="Refresh" hotkey="r" plain dimColor onPress={() => fetchNow($, list.filter(isActive).map(refOf))} />
          {closeButton}
          {done > 0 && (
            <Button key="dismiss-finished" label="Dismiss finished" hotkey="d" plain dimColor onPress={() => dismissFinished($)} />
          )}
        </Box>
        {ghLine}
        {notice}
      </Box>
    )
  })

  // The band layout: a line per watched PR above the prompt, most urgent first, and one line of
  // buttons. It yields to a survey, draws nothing with nothing watched, and keeps whatever the
  // plugins after it draw there, below its own lines (it shrinks to two lines beside them).
  on('ui.render', { component: 'AbovePrompt' }, async ($, e, next) => {
    const isTerminal = e.surface === 'terminal'
    // Nothing of ours mounted in the terminal's band: no spinner to blit there. A desktop draw
    // leaves the terminal's spinners be.
    const yieldBand = () => {
      if (isTerminal) bandSpins.clear()

      return next(e)
    }
    if (layout !== 'band' || e.props.hasSurvey) return yieldBand()
    const list = (await read($, prs)).map(full)
    if (list.length === 0) return yieldBand()
    const theirs = await next(e)
    const plan = planOf(list, e.props.maxRows, theirs.type === 'engine' ? BAND_ROWS : SHARED_ROWS)
    if (plan.shown.length === 0) {
      if (isTerminal) bandSpins.clear()

      return theirs
    }
    const { Box, Text, Button } = $.ui.resolve(e)
    // Narrowed by the surface, so the terminal's table (the one with a Raster) is resolved there.
    const Raster = e.surface === 'terminal' ? $.ui.resolve(e).Raster : null
    const [at, fixRows, addressRows, resolveRows, upgrade, budget] = await Promise.all([
      read($, now),
      readRows($, 'fix'),
      readRows($, 'address'),
      readRows($, 'resolve'),
      isNoticeOn ? read($, newer) : null,
      read($, limits),
    ])
    // Only the pause, or a bucket below 20%: a healthy budget leaves the line to the buttons.
    const ghNote = bandNote(budget, at)
    const width = Math.max(20, e.props.bodyColumns)
    // Single lines: the theme's text colours and spinner, no frames.
    const paint = look()
    const { theme, variant } = paint
    if (isTerminal) {
      bandId = e.requestId
      bandSpins.clear()
    }

    const line = (pr: TrackedPr) => {
      const row = rowOf(pr, width)
      const isSpinning = pr.isMoving && Raster !== null
      if (isSpinning) bandSpins.add(pr.url)

      return (
        <Box key={`band:${pr.url}`}>
          {isSpinning ? (
            <Raster key={`band-spin:${pr.url}`} columns={1} rows={1} cells={spinnerCell(frame + pr.number, variant.raster.spinner, theme.spin)} />
          ) : (
            <Text color={inkOf(paint, pr.tone)} bold>
              {row.icon}
            </Text>
          )}
          <Text wrap="truncate-end">
            {' '}
            <Text color={inkOf(paint, pr.tone, 'number')} bold>
              {row.number}
            </Text>
            {row.title !== '' && <Text> {row.title}</Text>}
            <Text dimColor>{row.title !== '' ? ' · ' : ' '}</Text>
            <Text color={inkOf(paint, pr.tone)}>{row.state}</Text>
          </Text>
        </Box>
      )
    }

    // The most urgent PR's hand-off, when it has one: Resolve where its branch conflicts, else
    // Fix where CI failed, else Address where threads are open. While it collects, what it does
    // stands in for its button.
    const top = plan.shown[0]
    const handoff = top !== undefined && top.state === 'OPEN' ? HANDOFFS.find(h => h.isOffered(top)) : undefined
    const rows = { fix: fixRows, address: addressRows, resolve: resolveRows }
    const isCollecting = top !== undefined && handoff !== undefined && isBusy(rows[handoff.id][top.url]?.busy, at)
    const handoffLabel = `${handoff?.verb} #${top?.number} with Claude`
    const handoffKey = handoff?.key ?? ''
    const busyText = `◌ ${handoff?.busy}`
    const fixed = buttonCells('o', 'Open pane') + 2 + buttonCells('r', 'Refresh')
    const actions = actionsOf(
      width,
      fixed,
      plan.more,
      handoff === undefined ? null : isCollecting ? cells(busyText) : buttonCells(handoffKey, handoffLabel),
      upgrade === null ? null : { long: headOf(upgrade.version), short: `↑ ${upgrade.version}` },
      ghNote,
    )

    return (
      <Box flexDirection="column">
        {plan.shown.map(line)}
        {plan.hasActions && (
          <Box key="band:actions" gap={2}>
            {actions.more !== null && <Text dimColor>{actions.more}</Text>}
            {/* A press is the person's ask: the pane is placed at any width, with the keys. */}
            <Button key="band-open" label="Open pane" hotkey="o" plain dimColor onPress={() => $.ui.open({ ...OPEN, focus: true })} />
            <Button key="band-refresh" label="Refresh" hotkey="r" plain dimColor onPress={() => fetchNow($, list.filter(isActive).map(refOf))} />
            {actions.handoff && top !== undefined && handoff !== undefined && (isCollecting ? (
              <Text color={inkOf(paint, 'suggestion')}>{busyText}</Text>
            ) : (
              <Button key="band-handoff" label={handoffLabel} hotkey={handoffKey} plain dimColor onPress={() => bandHandoff($, handoff.id, top)} />
            ))}
            {actions.notice !== null && <Text dimColor>{actions.notice}</Text>}
            {actions.gh !== null && (
              <Text key="band:gh" color={ghNote === null || ghNote.tone === null ? undefined : inkOf(paint, ghNote.tone)} dimColor={(ghNote?.tone ?? null) === null}>
                {actions.gh}
              </Text>
            )}
          </Box>
        )}
        {theirs}
      </Box>
    )
  })
}
