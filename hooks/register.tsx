import { atom, read, update } from 'claude-code'
import type { EngineInterface, Register, RenderChildren } from 'claude-code'

import type { Approvals, Busy, CommentAsk, Details, Fetched, HandoffAsk, Limits, MergeAsk, Tab, TrackedPr, Verdict } from '../types'
import { barCells, barRuns, fullBarCells, meterCells, ruleCells, ruleText, spinnerCell } from './look'
import { artCard, artCells, bannerLines, roleTone } from './art'
import type { ArtCard, Seg } from './art'
import { barPaint, cardBox, depthOf, inkOf, lookOf, modeOf, modeOfColours, optionTheme, pillOf, rasterBg, themeListing, themeOf, unknownTheme } from './theme'
import type { Depth, Look, Mode, Role, ThemeName } from './theme'
import {
  MERGE_LABEL,
  STEPS,
  afterServerError,
  ago,
  cwdOf,
  derive,
  findPrUrls,
  isActive,
  isBusy,
  isServerError,
  mergeArgv,
  mergeError,
  mergeOf,
  mergeQuestion,
  mergeReadArgv,
  mergeReadOf,
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
import { commentArgv, commentError, commentPlaceholder, commentUrl, isPostable } from './comment'
import { cells, newReviews, reviewToasts, reviewsOf, threadsBeside } from './review'
import { TABS, approvalsHeading, approvalsOf, approvalsQuery, checkRows, gateNote, parseApprovals, parseTeams, ruledTeamIds, seenOf, stillNeeded, tabLabel, teamsQuery, threadRows } from './tabs'
import { CHECK_MS, PRW_LINE, PRW_PILL, PRW_TEXT, PRW_URL, companionIn, headOf, howToUpdate, isNewer, isPrwIn, noticeLines, noticeToast, versionIn } from './update'
import type { CompanionState } from './update'
import { BAND_ROWS, SHARED_ROWS, actionsOf, buttonCells, layoutOf, planOf, rowOf } from './band'
import type { Layout } from './band'
import { failedEntry, isMiss, isUnreachable, keptEntries } from './cache'
import { NO_LOGINS, bestLogin, everyLogin, isDue, learned, loginFor, loginsOf, notFoundAs, ownersBelowWrite, rankOf, tokenError, untriedBy } from './accounts'
import type { AsLogin, Logins } from './accounts'
import type { Entry } from './cache'
import { GH_HELP, bandNote, bucketOf, ghMeter, isRestDue, legacyRate, lowUntil, restOf, roundWaitsUntil, withSeen } from './rate'
import type { Bar, Rates } from './rate'
import type { GhPr, MergeRun, PrRef } from './pr'
import { isTemp, placeOf, sessionFile, tempPath, urlsIn } from './protocol'
import type { Place } from './protocol'

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
// A card's comment: its field, the post, what came of it.
const commenting = atom({ plugin: 'pr-watch', key: 'commenting' } as const, {})
// Try again on a not-found card, while every login is asked.
const retrying = atom({ plugin: 'pr-watch', key: 'retrying' } as const, {})
// A newer pr-watch than this one, for the footer.
const newer = atom({ plugin: 'pr-watch', key: 'newer' } as const, null)
// Whether the card telling of prw shows (sayCompanion).
const companion = atom({ plugin: 'pr-watch', key: 'companion' } as const, false)
// GitHub's budget per login and a hold, for the gh meter.
const limits = atom({ plugin: 'pr-watch', key: 'limits' } as const, { logins: [], heldUntil: 0 })
// Whether the gh meter's explanation is open (its `?`): shut until asked, per session.
const ghHelp = atom({ plugin: 'pr-watch', key: 'ghHelp' } as const, false)
// Each card's tab, and what its tabs read (tabs.ts), by PR url.
const tabs = atom({ plugin: 'pr-watch', key: 'tabs' } as const, {})
const details = atom({ plugin: 'pr-watch', key: 'details' } as const, {})
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
// What each card's comment field holds, by PR url: kept here on every keystroke, so typing
// redraws nothing, and a redraw (a poll round's) draws the field with it. Lost on reload.
const drafts = new Map<string, string>()

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
// So N sessions cost one GitHub call per round, not N. Other readers and writers (the `prw`
// companion) share these files: companion/PROTOCOL.md says what each holds, and what a reader
// may meet in them.

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
// Session files and temp files untouched this long are removed, at most every PRUNE_EVERY_MS.
const PRUNE_MS = 86_400_000
const PRUNE_EVERY_MS = 3_600_000
// `mv` and `rm` on the cache's own files answer at once, or not at all.
const FILE_OP_MS = 5_000

// Survives /clear (the module stays loaded), unlike the session id.
const INSTANCE = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`
let homeDir: string | null | undefined
let dir: string | null = null
// Where this session runs, for its session file: from the session start, and again whenever
// the engine says the directory changed (classic.CwdChanged), never asked per tick.
let place: Place | null = null
let placeSeq = 0
// gh's logins as `gh auth status` last listed them, when gh was last asked and whether it
// answered (accounts.ts's isDue says when it is asked again), and the reading under way, so
// callers at once share one. A reading that lists none is never kept: the last good one stands.
let loginsKept: Logins = NO_LOGINS
let loginsAsked: { at: number; isOk: boolean } | undefined
let loginsAsk: Promise<Logins> | null = null
let prunedAt = 0

const empty = (): Shared => ({ fetchedAt: 0, nextAt: 0, prs: {}, accounts: {}, rate: null, rates: {}, latest: { version: null, checkedAt: 0 } })

// A call as `login` goes through sh, which asks gh for the login's token and hands it to that
// one gh call: the mod never holds a token, nor sets one in env or argv (a guard on mods may
// refuse a call that sets env). A login gh has no token for fails with tokenError's text,
// never runs as the active account.
const AS_LOGIN = `t=$(gh auth token --hostname github.com --user "$1" 2>/dev/null) && [ -n "$t" ] || { echo "Couldn't get $1's gh token" >&2; exit 4; }; shift; export GH_TOKEN="$t"; exec gh "$@"`

const ghArgv = (argv: readonly string[], login?: string) => (login ? ['/bin/sh', '-c', AS_LOGIN, 'sh', login, ...argv] : ['gh', ...argv])

async function gh($: EngineInterface, argv: string[], login?: string, cwd?: string, timeoutMs = GH_MS) {
  return $.process.run(ghArgv(argv, login), { cwd, timeoutMs })
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

// In place: a reader may catch the file half-written. Every tick's writes (lease.json, the
// session file) go this way, as an exec each tick costs more than a torn read does (a reader
// takes one as absent, readJson's null, and reads again next tick); results.json alone is
// written whole (writeResults).
async function writeJson($: EngineInterface, path: string, value: unknown) {
  try {
    await $.fs.write(path, JSON.stringify(value))
  } catch {
    // The cache is an optimisation: a session that cannot write it still polls for itself.
  }
}

// results.json, which every reader reads whole, is written whole: to a temp file beside it,
// then moved over it, a rename a reader sees all or nothing of. `$.fs` has no rename, so it is
// an `mv`, once a round. Where that fails, the file is written in place as before, so the round
// is shared all the same; a temp file left behind is taken over by this session's next round,
// or pruned (liveUrls).
async function writeResults($: EngineInterface, shared: Shared) {
  const path = `${await root($)}/results.json`
  const temp = tempPath(path, INSTANCE)
  try {
    await $.fs.write(temp, JSON.stringify(shared))
    const moved = await $.process.run(['mv', '-f', temp, path], { timeoutMs: FILE_OP_MS })
    if (moved.exitCode === 0) return
  } catch {
    // Written in place below.
  }
  await writeJson($, path, shared)
}

async function readShared($: EngineInterface): Promise<Shared> {
  return { ...empty(), ...(await readJson<Shared>($, `${await root($)}/results.json`)) }
}

// Where the session runs, for its session file: the directory, and its project root's folder
// as its name (a `cd` into a subfolder doesn't rename it). Where the engine won't say the root,
// the directory names it. Only the latest call lands: the session start's read, left running,
// never overwrites a later classic.CwdChanged's.
async function readPlace($: EngineInterface, cwd: string) {
  const seq = ++placeSeq
  const at = placeOf(cwd, await $.session.root().catch(() => cwd))
  if (seq === placeSeq) place = at
}

// This session's watch list, and who and where it is (protocol.ts); written every tick, so its
// mtime says the session is alive.
async function announce($: EngineInterface, urls: readonly string[]) {
  await writeJson($, `${await root($)}/sessions/${INSTANCE}.json`, sessionFile(urls, place))
}

// Every live session's PRs: an older mod's file, a newer one's and the companion's alike
// (urlsIn), one cut off mid-write listing none this time.
async function liveUrls($: EngineInterface, at: number): Promise<string[]> {
  const cache = await root($)
  const base = `${cache}/sessions`
  let entries: Awaited<ReturnType<EngineInterface['fs']['list']>> = []
  try {
    entries = await $.fs.list(base)
  } catch {
    return []
  }
  const live = entries.filter(f => f.kind === 'file' && f.name.endsWith('.json') && at - f.mtimeMs < ALIVE_MS)
  const isPruneDue = at - prunedAt > PRUNE_EVERY_MS
  // Listed beside the session reads, not after them: the round waits on neither alone.
  const [lists, temps] = await Promise.all([
    Promise.all(live.map(f => readJson<unknown>($, `${base}/${f.name}`))),
    isPruneDue ? $.fs.list(cache).catch(() => []) : Promise.resolve([]),
  ])
  if (isPruneDue) {
    prunedAt = at
    // Exited sessions' files, and temp files a write left beside results.json (writeResults).
    const dead = [
      ...entries.filter(f => f.kind === 'file' && at - f.mtimeMs > PRUNE_MS).map(f => `${base}/${f.name}`),
      ...temps.filter(f => f.kind === 'file' && isTemp(f.name) && at - f.mtimeMs > PRUNE_MS).map(f => `${cache}/${f.name}`),
    ]
    if (dead.length > 0) void $.process.run(['rm', '-f', ...dead], { timeoutMs: FILE_OP_MS }).catch(() => undefined)
  }

  return [...new Set(lists.flatMap(urlsIn))]
}

// A lease, not a lock: plain files have no atomic create, so claim, wait, and re-read. Two
// sessions racing past this costs one duplicate round, never a missed one. Written in place
// (writeJson): a lease read torn is no lease held, so at worst that costs a duplicate round too.
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

// gh's logins, read again every few minutes (a login added or switched to shows): the last
// good reading stands meanwhile, and is awaited only where there is none yet, or with
// `isFresh` (before a round calls a PR not found).
async function logins($: EngineInterface, isFresh = false, timeoutMs = GH_MS): Promise<Logins> {
  // The time first: callers at once then see each other's ask, never start one each.
  const at = await $.clock.now()
  if (loginsAsk === null && (isFresh || isDue(loginsAsked, at))) {
    loginsAsk = (async () => {
      const status = await gh($, ['auth', 'status', '--hostname', 'github.com'], undefined, undefined, timeoutMs).catch(() => null)
      const read = status === null ? null : loginsOf(`${status.stdout}\n${status.stderr}`)
      loginsAsked = { at: await $.clock.now(), isOk: read !== null }
      if (read !== null) loginsKept = read

      return loginsKept
    })().finally(() => (loginsAsk = null))
  }

  return loginsAsk !== null && (isFresh || loginsKept === NO_LOGINS) ? loginsAsk : loginsKept
}

// The login a call runs as, or what the card says where there is none. No token is read here:
// gh() has gh hand the login's token to the one call it makes (AS_LOGIN).
const asLogin = (login: string): AsLogin => (login === '' ? { error: tokenError('') } : { login })

// The login a press acts as: the one the poller found sees the owner's repos, else the active
// one, which polls the owners no other login was found for. Never another: a press whose
// login has no token says so on the card.
async function pressLogin($: EngineInterface, pr: TrackedPr): Promise<string> {
  return loginFor((await readShared($)).accounts, ownerOf(pr), await logins($))
}

const ownerOf = (ref: PrRef) => ref.repo.split('/')[0] ?? ''

// One batch's call, timed, and what GitHub said: the status and headers, and the reply.
async function askBatch($: EngineInterface, part: readonly PrRef[], argv: string[], login: string, timeoutMs = GH_MS) {
  const startedAt = await $.clock.now()
  // A call past its timeout throws: it fails as a timeout, rather than losing the round. One
  // that throws sooner was refused or couldn't start, and says why: never a timeout, so it
  // isn't asked again.
  const res = await gh($, argv, login, undefined, timeoutMs).catch(async (e: unknown) => ({
    stdout: '',
    stderr:
      (await $.clock.now()) - startedAt >= timeoutMs - 1_000
        ? `gh api graphql did not finish: timeout after ${Math.round(timeoutMs / 1_000)}s`
        : `gh api graphql didn't run: ${e instanceof Error ? e.message : String(e)}`,
  }))
  const http = parseHttp(res.stdout)

  return { http, reply: parseReply(http.body, res.stderr, part), ms: (await $.clock.now()) - startedAt }
}

// Why a failed call failed (null: it didn't), logged to errors.log on the way.
async function failureIn($: EngineInterface, login: string, size: number, call: { http: Http; reply: { error: string | null }; ms: number }) {
  if (call.reply.error === null) return null
  const failure: CallFailure = failureOf(call.reply.error, call.http)
  await logFailure($, { login, size, ms: call.ms, http: call.http, failure, error: call.reply.error })

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
// (rate.ts), and the PRs keep what they have. Every batch goes as `login` (gh hands the call its
// token); where gh won't, the call fails with tokenError's text and none is called not found.
async function query($: EngineInterface, refs: readonly PrRef[], login: string, until: number, withLatest = false) {
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
  const as = asLogin(login)
  if ('error' in as) {
    for (const ref of refs) out.failed.set(ref.url, as.error)

    return out
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
    let call = await askBatch($, part, argv, as.login, budget)
    let failure = await failureIn($, login, part.length, call)
    if (failure === 'timeout') {
      const retry = callBudget(until, (await $.clock.now()) + RETRY_MS, GH_MS)
      if (retry !== null) {
        await $.clock.sleep(RETRY_MS)
        call = await askBatch($, part, argv, as.login, retry)
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
  const known = await logins($)
  const isLow = (login: string) => isRound && lowUntil(seen, login, at) !== null
  const saw = (login: string, r: { rate: RateLimit | null; rateAt: number }) => {
    const graphql = bucketOf(r.rate, r.rateAt)
    if (graphql !== null) seen = withSeen(seen, login, { graphql })
  }
  // The logins that answered "not found" for each PR, by url: the card names them.
  const missedBy = new Map<string, string[]>()
  const missedAs = (login: string, refs: readonly PrRef[]) => refs.forEach(ref => missedBy.set(ref.url, [...(missedBy.get(ref.url) ?? []), login]))
  // The active account polls the owners no other account was found for, and those whose
  // login has logged out since.
  const byLogin = new Map<string, PrRef[]>()
  for (const ref of refs) {
    const login = loginFor(accounts, ownerOf(ref), known)
    byLogin.set(login, [...(byLogin.get(login) ?? []), ref])
  }
  const needed = [...byLogin.keys()]

  let missing: PrRef[] = []
  for (const [login, group] of byLogin) {
    if (waitMs !== null) break
    if (isLow(login)) continue
    const r = await query($, group, login, until, isAsking)
    if (r.isLatestAsked) isAsking = false
    waitMs = r.waitMs
    saw(login, r)
    r.found.forEach((pr, url) => {
      found.set(url, pr)
      // Remembered as a login that has logged out, and seen as this one: this one from now on.
      const ref = group.find(x => x.url === url)
      const owner = ref === undefined ? '' : ownerOf(ref)
      if (owner !== '' && accounts[owner] !== undefined && accounts[owner] !== login) accounts[owner] = login
    })
    r.failed.forEach((error, url) => errors.set(url, { error, isMiss: false }))
    missedAs(login, r.missing)
    missing.push(...r.missing)
    latest = r.latest ?? latest
  }
  if (missing.length > 0 && waitMs === null) {
    // "Not found" only once every logged-in account was asked and answered: one that failed,
    // ran out of time, is low on points or gave no token might see them yet. Before any is
    // called that, gh's logins are read again, so one logged in since is asked too.
    let isSure = true
    let order = everyLogin(known)
    for (const isAgain of [false, true]) {
      if (isAgain) {
        const budget = callBudget(until, await $.clock.now(), GH_MS)
        if (missing.length === 0 || waitMs !== null || !isSure || budget === null) break
        order = everyLogin(await logins($, true, budget))
      }
      for (const login of order) {
        const untried = untriedBy(missing, missedBy, login)
        if (untried.length === 0) continue
        if (isLow(login)) {
          isSure = false

          continue
        }
        const r = await query($, untried, login, until)
        waitMs = r.waitMs
        saw(login, r)
        if (r.failed.size > 0 || r.left.length > 0) isSure = false
        // A PR never read says why it still isn't: as a failure, which passes.
        r.failed.forEach((error, url) => {
          if (!errors.has(url)) errors.set(url, { error, isMiss: false })
        })
        missedAs(login, r.missing)
        r.found.forEach((pr, url) => {
          found.set(url, pr)
          errors.delete(url)
          const ref = untried.find(x => x.url === url)
          if (ref !== undefined) accounts[ownerOf(ref)] = login
        })
        missing = missing.filter(ref => !r.found.has(ref.url))
        if (missing.length === 0 || waitMs !== null) break
      }
    }
    // Held by a secondary limit, the rest were not all asked: they wait, as they were.
    // With gh's logins unread, the others were never asked: no PR is called not found.
    if (waitMs === null && isSure && order.length > 0) for (const ref of missing) errors.set(ref.url, { error: notFoundAs(missedBy.get(ref.url) ?? []), isMiss: true })
  }

  // An owner nobody is remembered for, whose PRs came back with less than write (a public repo
  // of the other account reads as anyone's): its PRs asked once as each other login, and the
  // owner kept as one that sees them all with more rights, so a merge goes as the right one.
  // After the PRs with no data: an owner one of them was found for is remembered, so not asked.
  const sightings = refs.flatMap(ref => {
    const pr = found.get(ref.url)

    return pr === undefined ? [] : [{ owner: ownerOf(ref), permission: pr.repository?.viewerPermission }]
  })
  for (const owner of waitMs === null ? ownersBelowWrite(sightings, accounts, known) : []) {
    const asked = loginFor(accounts, owner, known)
    const theirs = refs.filter(ref => ownerOf(ref) === owner && found.has(ref.url))
    const rank = (prs: Iterable<GhPr>) => Math.max(0, ...[...prs].map(pr => rankOf(pr.repository?.viewerPermission)))
    const answers = [{ login: asked, seen: theirs.length, rank: rank(theirs.flatMap(ref => found.get(ref.url) ?? [])) }]
    const got = new Map<string, Map<string, GhPr>>()
    let isSure = true
    for (const login of everyLogin(known).filter(login => login !== asked)) {
      if (isLow(login)) {
        isSure = false

        continue
      }
      const r = await query($, theirs, login, until)
      waitMs = r.waitMs
      saw(login, r)
      if (r.failed.size > 0 || r.left.length > 0) isSure = false
      got.set(login, r.found)
      answers.push({ login, seen: r.found.size, rank: rank(r.found.values()) })
      if (waitMs !== null) break
    }
    const best = bestLogin(answers, theirs.length)
    // Asked as every other login and none does more: the first stands, and isn't asked again.
    if (best !== asked || (isSure && waitMs === null)) accounts[owner] = best
    got.get(best)?.forEach((pr, url) => found.set(url, pr))
    if (waitMs !== null) break
  }

  return { found, errors, rates: seen, needed, latest, waitMs, asked: withLatest && !isAsking }
}

// GitHub's /rate_limit for each logged-in login once REST_MS is up (rate.ts): REST's requests,
// and the points again, for the gh meter. It counts against no limit. Only a round asks, in
// what is left of its lease, never every session; asked counts as checked, answered or not.
async function askRest($: EngineInterface, rates: Rates, at: number, until: number): Promise<Rates> {
  let seen = rates
  for (const login of everyLogin(await logins($))) {
    if (!isRestDue(seen, login, at)) continue
    const budget = callBudget(until, await $.clock.now(), REST_CALL_MS)
    if (budget === null) break
    // A login gh gives no token for is not asked as another: it counts as asked, unanswered.
    const as = asLogin(login)
    const res = 'login' in as ? await gh($, ['api', 'rate_limit'], as.login, undefined, budget).catch(() => null) : null
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
  const accountsWere = { ...before.accounts }
  // A round asks for the newest version too, once the last check is CHECK_MS old.
  const withLatest = isRound && isNoticeOn && at - (before.latest?.checkedAt ?? 0) >= CHECK_MS
  const { found, errors, rates, needed, latest, waitMs, asked } = await fetchRefs($, refs, before.accounts, before.rates, at, until, isRound, withLatest)
  // REST's count rides a round, never a watch or a Refresh, nor a round a limit stopped.
  const seen = isRound && waitMs === null ? await askRest($, rates, at, until) : rates
  // Re-read: another session may have written while this one was asking GitHub.
  const shared = await readShared($)
  shared.accounts = { ...shared.accounts, ...learned(accountsWere, before.accounts) }
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
  await writeResults($, shared)

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
  const mine = everyLogin(await logins($))
  const moved: string[] = []
  const next = list.map(current => {
    const entry = shared.prs[current.url]
    if (entry === undefined) return current
    const error = entry.error ?? null
    // Never read, as no login could see it: the card offers Try again.
    const isNotFound = isMiss(entry)
    // No newer data, yet the error may have come or gone: a failure that lasted keeps the
    // entry's time, so "checked 2m ago" stays the age of what the card shows.
    if (current.checkedAt !== null && entry.at <= current.checkedAt) {
      return error === current.error && isNotFound === current.isNotFound ? current : { ...current, error, isNotFound }
    }
    if (entry.pr === undefined) return { ...current, error, isNotFound, checkedAt: current.checkedAt ?? entry.at }
    const { fresh, seen } = newReviews(entry.pr, current.reviewSeen, mine, entry.pr.author?.login)
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
      isNotFound: false,
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
    const shown = limitsOf(shared, mine)

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
  // What a hand-off press said belongs to the failure, the threads or the conflict it was
  // about: gone once the card moves on, or no longer offers that hand-off (a draft's pill
  // stays put as its conflict comes and goes).
  const cards = new Map(next.map(pr => [pr.url, pr]))
  for (const handoff of HANDOFFS) {
    const isStale = (url: string) => {
      const card = cards.get(url)

      return moved.includes(url) || (card !== undefined && !handoff.isOffered(card))
    }
    await updateRows($, handoff.id, all => {
      const kept = Object.entries(all).filter(([url, ask]) => !isStale(url) || isBusy(ask.busy, at))

      return kept.length === Object.keys(all).length ? all : Object.fromEntries(kept)
    })
  }
  await refreshStatus($)
  await syncAnimation($)
  // Last, and never thrown: the update notice, and a tab read again, must not cost a PR result.
  await sayNewer($, shared).catch(() => undefined)
  await sayCompanion($).catch(() => undefined)
  await refreshTabs($, next).catch(() => undefined)
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

// Why the card telling of prw is gone for good, once known: read from companion.json, which
// a dismissal in any session writes, or written here on finding prw has run.
let companionState: CompanionState | null = null

// The card telling of prw: shown until someone dismisses it (in any session) or prw is seen
// running, and only while the update notice is on.
async function sayCompanion($: EngineInterface) {
  const isNews = isNoticeOn && companionState === null && (await isCompanionNews($))
  await update($, companion, current => {
    // Asked again after the reads: a Dismiss pressed, or the notice turned off, while they ran wins.
    const isShown = isNews && isNoticeOn && companionState === null

    return current === isShown ? current : isShown
  })
}

async function isCompanionNews($: EngineInterface): Promise<boolean> {
  const cache = await root($)
  const said = companionIn(await readJson<unknown>($, `${cache}/companion.json`))
  // Read first, then `??=`: a Dismiss pressed during the read isn't undone by the file it hasn't
  // written yet.
  companionState ??= said
  if (companionState !== null) return false
  const names = (await $.fs.list(`${cache}/sessions`).catch(() => [])).map(f => f.name)
  if (!isPrwIn(names)) return true
  if (companionState !== null) return false
  companionState = 'seen'
  await writeJson($, `${cache}/companion.json`, { state: companionState })

  return false
}

async function dismissCompanion($: EngineInterface) {
  companionState = 'dismissed'
  await update($, companion, () => false)
  await writeJson($, `${await root($)}/companion.json`, { state: companionState, at: await $.clock.now() })
}

// Fetches these PRs now (a new watch, the Refresh button) rather than waiting for the round.
async function fetchNow($: EngineInterface, refs: PrRef[]) {
  if (refs.length === 0) return
  // Held by a secondary limit, publish asks nothing: the next round does, once the wait is over.
  await apply($, await publish($, refs, await $.clock.now()))
}

// Try again on a card no login could see: asked now as every login, the five-minute skip
// (cache.ts) notwithstanding. Found, the card fills and the mark goes; not, the card says again
// who was asked, gh's logins read afresh first (fetchRefs). A second press while it asks does
// nothing.
async function tryAgain($: EngineInterface, pr: TrackedPr) {
  const busy = { at: await $.clock.now() }
  let isClaimed = false
  await update($, retrying, all => {
    isClaimed = !isBusy(all[pr.url], busy.at)

    return isClaimed ? { ...all, [pr.url]: busy } : all
  })
  if (!isClaimed) return
  try {
    await fetchNow($, [refOf(pr)])
  } catch {
    // The card says what it said: the button comes back to press again.
  } finally {
    await update($, retrying, all => {
      const { [pr.url]: _, ...rest } = all

      return all[pr.url]?.at === busy.at ? rest : all
    })
  }
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

// Runs gh pr merge as the account the poller found sees the repo (its token, never written
// anywhere), and only as that one, then asks GitHub again at once so the card shows what
// happened. A server error (a 502, a gateway timeout) says nothing of whether it went through:
// RETRY_MS on, the PR is read again as the same login, and the run counts as done, runs once
// more, or shows the error it had, as afterServerError says (pr.ts).
async function runMerge($: EngineInterface, pr: TrackedPr, run: MergeRun) {
  const method = run.action === 'cancel-auto' ? undefined : run.method
  const busy = { action: run.action, at: await $.clock.now() }
  if (!(await claimMerge($, pr.url, { method, busy }))) return
  let ask: MergeAsk = { method }
  try {
    const got = asLogin(await pressLogin($, pr))
    if ('error' in got) ask = { method, error: got.error }
    else {
      let res = await gh($, mergeArgv(pr.url, run), got.login)
      if (res.exitCode !== 0 && isServerError(res.stderr || res.stdout)) {
        await $.clock.sleep(RETRY_MS)
        const view = await gh($, mergeReadArgv(pr.url), got.login).catch(() => null)
        const next = afterServerError(run, view !== null && view.exitCode === 0 ? mergeReadOf(view.stdout) : null)
        if (next === 'done') res = { ...res, exitCode: 0 }
        else if (next === 'retry') res = await gh($, mergeArgv(pr.url, run), got.login)
      }
      if (res.exitCode !== 0) {
        const { message, isMethodRefused } = mergeError(res.stderr || res.stdout)
        ask = { method: isMethodRefused ? undefined : method, error: message }
        if (isMethodRefused && method !== undefined) {
          await update($, refused, all => ({ ...all, [pr.repo]: [...new Set([...(all[pr.repo] ?? []), method])] }))
        }
      }
    }
  } catch {
    ask = { method, error: 'gh pr merge did not finish' }
  }
  // Only while the row is still this run's: not on a dismissed card, nor over a later press.
  await update($, merging, all => (all[pr.url]?.busy?.at === busy.at ? { ...all, [pr.url]: ask } : all))
  await fetchNow($, [refOf(pr)])
}

// ---- Comment ------------------------------------------------------------------------------
// A card's Comment opens a field; Enter posts what it holds to the PR's conversation (an issue
// comment, not a review). Nothing is preset: the person writes every comment.

// The field's address: what `$.ui.focus` and a test type into.
const commentKey = (url: string) => `comment-text:${url}`

async function setComment($: EngineInterface, url: string, ask: CommentAsk | null) {
  await update($, commenting, all => {
    const { [url]: _, ...rest } = all

    return ask === null ? rest : { ...rest, [url]: ask }
  })
}

// Open or Cancel from a frame drawn before a post began (a quick click, another surface) waits
// on it: overwriting its row would drop what it came to, and let a second Enter post again.
// False: a post holds the row, nothing changed.
async function setIdleComment($: EngineInterface, url: string, ask: CommentAsk | null) {
  const at = await $.clock.now()
  let isSet = false
  await update($, commenting, all => {
    isSet = !isBusy(all[url]?.busy, at)
    if (!isSet) return all
    const { [url]: _, ...rest } = all

    return ask === null ? rest : { ...rest, [url]: ask }
  })

  return isSet
}

// Comment opens the field, the last post's word gone, and moves the keys onto it where the pane
// holds them (a click, ctrl+x tab); where it doesn't, the field takes them once it does.
async function openComment($: EngineInterface, pr: TrackedPr) {
  if (!(await setIdleComment($, pr.url, { isOpen: true }))) return
  void $.ui.focus({ requestId: PANE, key: commentKey(pr.url) }).catch(() => undefined)
}

// Cancel: the field closes and its text goes, nothing posted.
async function closeComment($: EngineInterface, url: string) {
  if (await setIdleComment($, url, null)) drafts.delete(url)
}

// Enter in the field posts its text as the login the poller found sees the repo, by that login's
// own token, as a merge runs (runMerge): never as another account. Blank text does nothing, and
// neither does a second Enter while it posts. Posted, the field closes, the card links the
// comment and GitHub is asked again (a comment can start checks or change a review); refused,
// the text stays in the field and the card says why, to try again.
async function postComment($: EngineInterface, pr: TrackedPr, text: string) {
  if (!isPostable(text)) return
  const busy = { at: await $.clock.now() }
  let isClaimed = false
  await update($, commenting, all => {
    const ask = all[pr.url]
    isClaimed = ask?.isOpen === true && !isBusy(ask.busy, busy.at)

    return isClaimed ? { ...all, [pr.url]: { isOpen: true, busy } } : all
  })
  if (!isClaimed) return
  drafts.set(pr.url, text)
  let ask: CommentAsk = { isOpen: true }
  try {
    const login = await pressLogin($, pr)
    const got = asLogin(login)
    if ('error' in got) ask = { isOpen: true, error: got.error }
    else {
      const res = await gh($, commentArgv(pr, text), got.login)
      ask = res.exitCode === 0 ? { done: { url: commentUrl(res.stdout) ?? pr.url, login } } : { isOpen: true, error: commentError(res.stderr || res.stdout, login) }
    }
  } catch {
    // Cut off, not refused: GitHub may have taken it, so a retry could post it twice.
    ask = { isOpen: true, error: "Couldn't tell whether it posted: gh api did not finish. Check the PR before trying again" }
  }
  if (ask.done !== undefined) drafts.delete(pr.url)
  // Only while the row is still this post's: not on a dismissed card.
  await update($, commenting, all => (all[pr.url]?.busy?.at === busy.at ? { ...all, [pr.url]: ask } : all))
  if (ask.done !== undefined) await fetchNow($, [refOf(pr)])
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

// Every hand-off's rows, by kind, for the pane and the band.
async function readHandoffs($: EngineInterface): Promise<Record<Kind, Rows>> {
  const [fix, address, resolve] = await Promise.all([readRows($, 'fix'), readRows($, 'address'), readRows($, 'resolve')])

  return { fix, address, resolve }
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
    const got = asLogin(await pressLogin($, pr))
    // No token for that login: nothing is read as another one.
    if ('error' in got) {
      await settleHandoff($, 'fix', pr.url, busy.at, { error: got.error })

      return
    }
    const { login } = got
    let failures: Failure[]
    let problem: string | null = null
    try {
      const res = await gh($, ['api', 'graphql', '-f', `query=${checksQuery(refOf(pr))}`], login)
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
          const res = await gh($, argv, login, undefined, 60_000)

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
    const got = asLogin(await pressLogin($, pr))
    if ('error' in got) {
      await settleHandoff($, 'address', pr.url, busy.at, { error: got.error })

      return
    }
    const res = await gh($, ['api', 'graphql', '-f', `query=${threadsQuery(refOf(pr))}`], got.login)
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
    // Pressed again once stale while the cards were read (see collectFailure).
    if ((await readRows($, 'resolve'))[pr.url]?.busy?.at !== busy.at) return
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

// A hand-off's press, from a card or the band: what it collects, then its draft.
async function startHandoff($: EngineInterface, kind: Kind, pr: TrackedPr) {
  if (kind === 'resolve') await draftResolve($, pr)
  else if (kind === 'fix') await collectFailure($, pr)
  else await collectThreads($, pr)
}

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
  await startHandoff($, kind, pr)
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

// ---- A card's tabs (tabs.ts) --------------------------------------------------------------

type Reads = Exclude<Tab, 'overview'>
type Got = { data: NonNullable<Fetched<unknown>['data']> } | { error: string }

// One query for this PR alone, of what the tab shows, as the gh account the poller found sees
// the repo; a ruleset's teams take a second, for their names.
async function fetchTab($: EngineInterface, pr: TrackedPr, tab: Reads, login: string): Promise<Got> {
  const ref = refOf(pr)
  const ask = async (query: string) => gh($, ['api', 'graphql', '-f', `query=${query}`], login)
  if (tab === 'checks') {
    const res = await ask(checksQuery(ref))
    const reply = parseChecks(res.stdout, res.stderr, ref)

    return 'error' in reply ? { error: `Couldn't read the checks: ${reply.error}` } : { data: checkRows(reply.checks) }
  }
  if (tab === 'threads') {
    const res = await ask(threadsQuery(ref))
    const reply = parseThreads(res.stdout, res.stderr)

    return 'error' in reply ? { error: `Couldn't read the review threads: ${reply.error}` } : { data: threadRows(reply.threads) }
  }
  const res = await ask(approvalsQuery(ref))
  const reply = parseApprovals(res.stdout, res.stderr)
  if ('error' in reply) return { error: `Couldn't read the review rules: ${reply.error}` }
  const ids = ruledTeamIds(reply.pr)
  // Teams it can't name are left out, as for a login that can't see them.
  const teams = ids.length === 0 ? new Map<string, string>() : parseTeams((await ask(teamsQuery(ids)).catch(() => ({ stdout: '' }))).stdout)

  return { data: approvalsOf(reply.pr, ownerOf(pr), teams) }
}

// Reads a card's tab, once at a time: a read under way is not started again. What it came to
// replaces what the tab held; a failure keeps what was there, with why beside it. Dismissed
// meanwhile, the card's row is gone, and what came is dropped.
async function readTab($: EngineInterface, pr: TrackedPr, tab: Reads) {
  const at = await $.clock.now()
  let isClaimed = false
  await update($, details, all => {
    const held: Fetched<unknown> | undefined = all[pr.url]?.[tab]
    isClaimed = !isBusy(held?.busy, at)

    return isClaimed ? ({ ...all, [pr.url]: { ...all[pr.url], [tab]: { ...held, busy: { at } } } } as Record<string, Details>) : all
  })
  if (!isClaimed) return
  const seen = seenOf(tab, pr)
  const settle = (got: Got) =>
    update($, details, all => {
      const held: Fetched<unknown> | undefined = all[pr.url]?.[tab]
      if (held?.busy?.at !== at || !(pr.url in all)) return all
      const fetched = 'error' in got ? { data: held.data, error: got.error, seen } : { data: got.data, seen }

      return { ...all, [pr.url]: { ...all[pr.url], [tab]: fetched } } as Record<string, Details>
    })
  try {
    const got = asLogin(await pressLogin($, pr))
    await settle('error' in got ? { error: got.error } : await fetchTab($, pr, tab, got.login))
  } catch {
    await settle({ error: "Couldn't read GitHub" })
  }
}

// A tab pressed: the card shows it from then on, and a tab that reads, reads now (pressed
// again, again: it is also how to read it afresh).
async function openTab($: EngineInterface, pr: TrackedPr, tab: Tab) {
  await update($, tabs, all => {
    const { [pr.url]: _, ...rest } = all

    return tab === 'overview' ? rest : { ...rest, [pr.url]: tab }
  })
  if (tab !== 'overview') await readTab($, pr, tab)
}

// After a round: a card on a tab that reads reads it again where the round changed what it
// shows; one that didn't costs nothing.
async function refreshTabs($: EngineInterface, list: readonly TrackedPr[]) {
  const shown = await read($, tabs)
  const held = await read($, details)
  const stale = list.flatMap(pr => {
    const tab = shown[pr.url]
    if (tab === undefined || tab === 'overview') return []
    const fetched: Fetched<unknown> | undefined = held[pr.url]?.[tab]

    return fetched !== undefined && fetched.seen !== seenOf(tab, pr) ? [{ pr, tab }] : []
  })
  await Promise.all(stale.map(({ pr, tab }) => readTab($, pr, tab)))
}

async function dismiss($: EngineInterface, url: string) {
  await update($, prs, list => list.filter(pr => pr.url !== url))
  await update($, dropped, list => (list.includes(url) ? list : [...list, url]))
  await setAsk($, url, null)
  await setHandoff($, 'fix', url, null)
  await setHandoff($, 'address', url, null)
  await setHandoff($, 'resolve', url, null)
  await setComment($, url, null)
  await update($, tabs, ({ [url]: _, ...rest }) => rest)
  await update($, details, ({ [url]: _, ...rest }) => rest)
  drafts.delete(url)
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
  // gh's logins are read again: one may have been added or logged out.
  loginsKept = NO_LOGINS
  loginsAsked = undefined

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
      if (!isNoticeOn) {
        await update($, newer, () => null)
        await update($, companion, () => false)
      }
    }

    return done
  }).catch(($, e, next) => next(e))

  on('session.start', async ($, e, next) => {
    // Off the start path: until the root is read, the directory names the session.
    place = placeOf(e.cwd)
    void readPlace($, e.cwd)
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

  // A `cd`, a /cd or a worktree move: the session file says where the session runs from its
  // next tick. A subagent running in a directory of its own (a worktree, a spawn `cwd`) fires
  // none, and one sharing the session's directory moves the session's: no `agent_id` to check.
  on('classic.CwdChanged', async ($, e, next) => {
    await readPlace($, e.new_cwd)

    return next(e)
  }).catch(($, e, next) => next(e))

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
      // `git push`, `gh pr checks`: ask gh which PR that was, from the same directory, as
      // Claude's own gh call ran (no token of ours). It only names the PR: rounds read it as
      // the login that sees it.
      const num = shellCode(e.command).match(/\bgh\s+pr\s+[\w-]+\s+#?(\d+)\b/)?.[1]
      const res = await gh($, ['pr', 'view', ...(num ? [num] : []), '--json', 'url', '-q', '.url'], undefined, cwdOf(e.command))
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
    // The mobile app draws no field yet: no Comment there.
    const Input = e.surface === 'mobile' ? null : $.ui.resolve(e).Input
    const list = (await read($, prs)).map(full)
    const at = await read($, now)
    const asks = await read($, merging)
    const refusedBy = await read($, refused)
    const handoffs = await readHandoffs($)
    const retries = await read($, retrying)
    const comments = await read($, commenting)
    const shownTabs = await read($, tabs)
    const read_ = await read($, details)
    // The login an open field will post as, for its placeholder.
    // Only for a field that draws (an open card, a surface with Input), and results.json and
    // gh's logins read once for all of them, not once per card on every redraw.
    const commentAs = new Map<string, string>()
    const fields = Input === null ? [] : list.filter(pr => pr.state === 'OPEN' && comments[pr.url]?.isOpen === true)
    if (fields.length > 0) {
      const { accounts } = await readShared($)
      const known = await logins($)
      for (const pr of fields) commentAs.set(pr.url, loginFor(accounts, ownerOf(pr), known))
    }
    const upgrade = isNoticeOn ? await read($, newer) : null
    const isCompanionShown = isNoticeOn && (await read($, companion))
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

    // prw, the companion app: a card of its own under the PRs, until it is dismissed.
    const companionPill = pillOf(paint, 'merged', PRW_PILL)
    const companionCard = (
      <Box key="companion" flexDirection="column" {...cardBox(paint, 'merged')}>
        <Text {...companionPill.props}>
          {companionPill.open !== '' && companionPill.open}
          {companionPill.text}
          {companionPill.close !== '' && companionPill.close}
        </Text>
        <Text wrap="wrap">
          <Text bold>prw</Text>: {PRW_TEXT}
        </Text>
        <Box gap={2} marginTop={1}>
          <Link href={PRW_URL} label="Learn more ↗" />
          <Button key="companion-dismiss" label="Dismiss" plain dimColor onPress={() => dismissCompanion($)} />
        </Box>
      </Box>
    )

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

    // The repo too where another card shares the number (acme/app#7, acme/api#7): a Link has no
    // key, so its label is what tells one card's from another's.
    const twinRepo = (pr: TrackedPr) => (list.some(o => o.url !== pr.url && o.number === pr.number) ? pr.repo : '')

    // Comment, last of a card's buttons and dim, as secondary actions are; pressed, a field below
    // them in its place, with Cancel. What the last post came to sits over the field: posting, a
    // link to the comment, or why GitHub said no (the text still in the field).
    const commentParts = (pr: TrackedPr) => {
      if (Input === null) return { button: null, rows: [] }
      const ask = comments[pr.url] ?? {}
      if (isBusy(ask.busy, at)) {
        return { button: null, rows: [<Text key={`comment-busy:${pr.url}`} color={ink('suggestion')}>◌ Posting the comment…</Text>] }
      }
      const rows = []
      if (ask.error !== undefined) {
        rows.push(
          <Text key={`comment-error:${pr.url}`} color={ink('error')} wrap="wrap">
            {`✗ ${ask.error}`}
          </Text>,
        )
      }
      if (ask.done !== undefined) {
        rows.push(
          <Text key={`comment-done:${pr.url}`} wrap="wrap">
            <Text color={ink('success')}>{`✓ Commented as ${ask.done.login} · `}</Text>
            <Link href={ask.done.url} label={`comment on ${twinRepo(pr)}#${pr.number} ↗`} />
          </Text>,
        )
      }
      if (ask.isOpen !== true) {
        return { button: <Button key={`comment:${pr.url}`} label="Comment" plain dimColor onPress={() => openComment($, pr)} />, rows }
      }
      // One line: at two, in the slim dock, the terminal cut the placeholder's login short and
      // wrapped the key hint onto the second row.
      rows.push(
        <Box key={`comment-field:${pr.url}`} flexDirection="column">
          <Input
            key={commentKey(pr.url)}
            placeholder={commentPlaceholder(pr, commentAs.get(pr.url) ?? '')}
            value={drafts.get(pr.url) ?? ''}
            submitLabel="post"
            autoFocus
            onInput={text => {
              drafts.set(pr.url, text)
            }}
            onSubmit={text => postComment($, pr, text)}
          />
          <Box>
            <Button key={`comment-cancel:${pr.url}`} label="Cancel" plain dimColor onPress={() => closeComment($, pr.url)} />
          </Box>
        </Box>,
      )

      return { button: null, rows }
    }

    // The hand-offs a card offers, what came of each one's last press above, and their buttons
    // side by side below, Comment last; a question, where no prompt box took the draft, stands
    // for its button. Comment's field and word go under the buttons.
    const actionRow = (pr: TrackedPr) => {
      const offered = HANDOFFS.filter(handoff => handoff.isOffered(pr))
      const comment = commentParts(pr)
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
        buttons.push(<Button key={`${handoff.id}:${pr.url}`} label={handoff.label} onPress={() => startHandoff($, handoff.id, pr)} />)
      }
      if (comment.button !== null) buttons.push(comment.button)
      if (notes.length === 0 && buttons.length === 0 && comment.rows.length === 0) return null

      return (
        <Box flexDirection="column">
          {notes}
          {buttons.length > 0 && (
            <Box columnGap={2} flexWrap="wrap">
              {buttons}
            </Box>
          )}
          {comment.rows}
        </Box>
      )
    }

    // A card no login could see: its error names the logins asked; Try again asks them all now.
    const notFoundRow = (pr: TrackedPr) => {
      if (!pr.isNotFound) return null
      if (isBusy(retries[pr.url], at)) return <Text color={ink('suggestion')}>◌ Asking every gh login…</Text>

      return (
        <Box>
          <Button key={`retry:${pr.url}`} label="Try again" onPress={() => tryAgain($, pr)} />
        </Box>
      )
    }

    // ---- The tabs (tabs.ts): a strip of them under the stepper, and the one shown under it.
    const tabOf = (pr: TrackedPr): Tab => shownTabs[pr.url] ?? 'overview'

    // "Overview  Checks 2/3  Reviews 1/2  Threads 3": the one shown bold in the card's colour.
    const tabStrip = (pr: TrackedPr) => {
      const shown = tabOf(pr)

      return (
        <Box columnGap={2} flexWrap="wrap">
          {TABS.map(tab => {
            const { name, count } = tabLabel(tab, pr, read_[pr.url]?.reviews?.data)
            const isShown = tab === shown

            return (
              <Button key={`tab:${tab}:${pr.url}`} label={count === '' ? name : `${name} ${count}`} plain dimColor={!isShown} onPress={() => openTab($, pr, tab)}>
                <Text bold={isShown} underline={isShown} color={isShown ? ink(pr.tone, 'steps') : undefined}>
                  {name}
                </Text>
                {count !== '' && <Text dimColor> {count}</Text>}
              </Button>
            )
          })}
        </Box>
      )
    }

    // What a reading tab says while it reads with nothing yet to show, or why it couldn't.
    const readNote = (fetched: Fetched<unknown> | undefined, what: string) => {
      if (fetched === undefined || (fetched.data === undefined && isBusy(fetched.busy, at))) {
        return <Text color={ink('suggestion')}>{`◌ Reading ${what}…`}</Text>
      }

      return fetched.error === undefined ? null : (
        <Text color={ink('error')} wrap="wrap">
          {`✗ ${fetched.error}`}
        </Text>
      )
    }

    // At most `max` rows, then "+3 more".
    const capped = <T,>(rows: readonly T[], max: number) => ({ shown: rows.slice(0, rows.length > max ? max - 1 : max), more: rows.length > max ? rows.length - max + 1 : 0 })
    const moreRow = (more: number, what = '') => more > 0 && <Text dimColor>{`  +${more} more${what}`}</Text>

    const CHECK_ICON = { failed: '✗', running: '◌', queued: '○', passed: '✓', skipped: '⊘' } as const
    const CHECK_TONE = { failed: 'error', running: 'warning', queued: undefined, passed: 'success', skipped: undefined } as const

    // Every check on the head commit, failing first: "✗ unit · CI". Before the first read, what
    // the poll knows: the failing and running ones by name, and how many passed or wait.
    const checksPanel = (pr: TrackedPr) => {
      const fetched = read_[pr.url]?.checks
      const rows = fetched?.data
      if (rows === undefined) {
        const { failing, running, queued, passed } = pr.checks

        return (
          <Box flexDirection="column">
            {readNote(fetched, 'the checks')}
            {failing.map(name => (
              <Text color={ink('error')} wrap="truncate-end">{`✗ ${name}`}</Text>
            ))}
            {running.map(name => (
              <Text color={ink('warning')} wrap="truncate-end">{`◌ ${name}`}</Text>
            ))}
            {queued + passed > 0 && <Text dimColor>{[queued > 0 ? `${queued} queued` : '', passed > 0 ? `${passed} passed` : ''].filter(Boolean).join(' · ')}</Text>}
          </Box>
        )
      }
      if (rows.length === 0) return <Text dimColor>No checks have reported on the head commit.</Text>
      const { shown, more } = capped(rows, 10)

      return (
        <Box flexDirection="column">
          {readNote(fetched, 'the checks')}
          {shown.map(row => {
            const tone = CHECK_TONE[row.state]

            return (
              <Text wrap="truncate-end">
                <Text color={tone === undefined ? undefined : ink(tone)} dimColor={tone === undefined}>{`${CHECK_ICON[row.state]} ${row.name}`}</Text>
                {row.workflow !== null && <Text dimColor>{` · ${row.workflow}`}</Text>}
                {row.state === 'skipped' && <Text dimColor> · skipped</Text>}
              </Text>
            )
          })}
          {moreRow(more)}
        </Box>
      )
    }

    const GATE_ICON = { approved: '✓', changes: '✗', waiting: '◷', maybe: '○' } as const
    const GATE_TONE = { approved: 'success', changes: 'error', waiting: 'warning', maybe: undefined } as const
    const VERDICT_SAYS: Record<Verdict, string> = { approved: 'approved', changes: 'requested changes', commented: 'commented', requested: 'asked' }
    const VERDICT_ICON: Record<Verdict, string> = { approved: '✓', changes: '✗', commented: '💬', requested: '◷' }

    // What the branch's rules ask for and where each part stands: "Required · 1 of 2
    // approvals", a row per code owner or required team (✓ met, by whom; ◷ waiting; ✗ changes
    // asked), the approvals still owed by anyone; then every reviewer, a row each.
    const reviewsPanel = (pr: TrackedPr) => {
      const fetched = read_[pr.url]?.reviews
      const approvals: Approvals | undefined = fetched?.data
      // A code owner still asked has its gate's row: not a second one here.
      const gated = new Set((approvals?.gates ?? []).map(gate => gate.name.replace(/^@(?:[^/]+\/)?/, '')))
      const reviewers = (pr.reviews?.reviewers ?? []).filter(reviewer => reviewer.verdict !== 'requested' || !gated.has(reviewer.login))
      const nameW = Math.min(22, Math.max(0, ...(approvals?.gates ?? []).map(gate => gate.name.length)))
      const owed = approvals === undefined ? '' : stillNeeded(approvals)

      return (
        <Box flexDirection="column">
          {readNote(fetched, 'the review rules')}
          {approvals !== undefined && (
            <Box flexDirection="column">
              <Text bold wrap="truncate-end">
                {approvalsHeading(approvals)}
              </Text>
              {approvals.gates.map(gate => {
                const tone = GATE_TONE[gate.state]

                return (
                  <Text wrap="truncate-end">
                    <Text color={tone === undefined ? undefined : ink(tone)} dimColor={tone === undefined}>
                      {` ${GATE_ICON[gate.state]} ${gate.name.length > nameW ? `${gate.name.slice(0, nameW - 1)}…` : gate.name.padEnd(nameW)}`}
                    </Text>
                    <Text dimColor>{`  ${gateNote(gate)}`}</Text>
                  </Text>
                )
              })}
              {owed !== '' && (
                <Text wrap="truncate-end">
                  <Text color={ink('warning')}>{` ○ ${owed}`}</Text>
                  <Text dimColor> · anyone with write access</Text>
                </Text>
              )}
            </Box>
          )}
          {reviewers.length > 0 && (
            <Box flexDirection="column" marginTop={approvals === undefined ? 0 : 1}>
              <Text bold>Reviewers</Text>
              {reviewers.map(reviewer => {
                const tone = VERDICT_COLOUR[reviewer.verdict]

                return (
                  <Text wrap="truncate-end">
                    <Text color={tone === undefined ? undefined : ink(tone)} dimColor={tone === undefined}>{` ${VERDICT_ICON[reviewer.verdict]} ${reviewer.login}`}</Text>
                    <Text dimColor>{` ${VERDICT_SAYS[reviewer.verdict]}`}</Text>
                  </Text>
                )
              })}
              {moreRow(pr.reviews?.more ?? 0)}
            </Box>
          )}
          {reviewers.length === 0 && approvals !== undefined && <Text dimColor>No reviews yet.</Text>}
        </Box>
      )
    }

    // The threads still open, by file and line: where, then who opened it and what they said.
    const threadsPanel = (pr: TrackedPr) => {
      const fetched = read_[pr.url]?.threads
      const rows = fetched?.data
      const note = readNote(fetched, 'the threads')
      if (rows === undefined) return note
      if (rows.length === 0) return note ?? <Text dimColor>No unresolved threads.</Text>
      const { shown, more } = capped(rows, 5)

      return (
        <Box flexDirection="column">
          {note}
          {shown.map(row => (
            <Box flexDirection="column">
              <Text wrap="truncate-start">
                <Text color={ink(row.isOutdated ? 'subtle' : 'warning')}>{row.spot}</Text>
                {row.replies > 0 && <Text dimColor>{` · ${row.replies} repl${row.replies === 1 ? 'y' : 'ies'}`}</Text>}
              </Text>
              <Text wrap="truncate-end">
                <Text bold>{`  ${row.author || 'ghost'}`}</Text>
                <Text dimColor>{` ${row.text}`}</Text>
              </Text>
            </Box>
          ))}
          {moreRow(more)}
        </Box>
      )
    }

    // A card's tab strip, a rule, and the panel of a reading tab: nothing for Overview, whose
    // rows are the card's own. None for a card never read, which has nothing to tab through.
    const tabsBlock = (pr: TrackedPr, overview: RenderChildren) => {
      if (pr.checkedAt === null || pr.isNotFound) return overview
      const tab = tabOf(pr)

      return (
        <Box flexDirection="column">
          <Box marginTop={1}>{tabStrip(pr)}</Box>
          <Text dimColor>{'─'.repeat(inner)}</Text>
          {tab === 'overview' ? overview : tab === 'checks' ? checksPanel(pr) : tab === 'reviews' ? reviewsPanel(pr) : threadsPanel(pr)}
        </Box>
      )
    }

    // A card's last row: its link, how old its data is, and its dismiss.
    const linkRow = (pr: TrackedPr) => (
      <Box justifyContent="space-between">
        <Text>
          {/* Its own label: with "open ↗" on every card, a click on any of them opened the last. */}
          <Link href={pr.url} label={`open ${twinRepo(pr)}#${pr.number} ↗`} />
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
      // The headline's own note on the threads it counts ("1 outdated"); the Threads tab has the rest.
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

          <Box>
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

          {tabsBlock(
            pr,
            <Text color={toneColour} wrap="wrap">
              {pr.headline}
              {isMerged && pr.mergedAt !== null && <Text dimColor> · {ago(at - Date.parse(pr.mergedAt))}</Text>}
              {threads.note !== '' && <Text dimColor> · {threads.note}</Text>}
            </Text>,
          )}
          {pr.error !== null && (
            <Text color={ink('error')} wrap="truncate-end">
              ⚠ {pr.error}
            </Text>
          )}
          {notFoundRow(pr)}
          {pr.state === 'OPEN' && actionRow(pr)}
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
          {/* The art is the banner's Overview: the strip, and another tab's panel, go under it. */}
          {tabsBlock(pr, null)}
          {notFoundRow(pr)}
          {pr.state === 'OPEN' && actionRow(pr)}
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
        {isCompanionShown && companionCard}
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
    const cap = theirs.type === 'engine' ? BAND_ROWS : SHARED_ROWS
    // The line telling of prw takes a PR's row, where a PR and the actions still fit beside it.
    const hasCompanion = isNoticeOn && (await read($, companion)) && Math.min(cap, Math.floor(e.props.maxRows)) >= 3
    const plan = planOf(list, e.props.maxRows - (hasCompanion ? 1 : 0), cap - (hasCompanion ? 1 : 0))
    if (plan.shown.length === 0) {
      if (isTerminal) bandSpins.clear()

      return theirs
    }
    const { Box, Text, Button, Link } = $.ui.resolve(e)
    // Narrowed by the surface, so the terminal's table (the one with a Raster) is resolved there.
    const Raster = e.surface === 'terminal' ? $.ui.resolve(e).Raster : null
    const [at, rows, upgrade, budget] = await Promise.all([
      read($, now),
      readHandoffs($),
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
        {hasCompanion && (
          <Box key="band:companion" gap={2}>
            <Text wrap="truncate-end">
              <Text color={inkOf(paint, 'merged')} bold>
                ✦
              </Text>{' '}
              {PRW_LINE}
            </Text>
            <Link href={PRW_URL} label="Learn more ↗" />
            <Button key="band-companion-dismiss" label="✕" hotkey="p" plain dimColor onPress={() => dismissCompanion($)} />
          </Box>
        )}
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
