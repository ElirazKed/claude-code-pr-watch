import { atom, read, update } from 'claude-code'
import type { EngineInterface, Register } from 'claude-code'

import type { MergeAsk, TrackedPr } from '../types'
import { PALETTE, barCells, barRuns, fullBarCells, ruleCells, spinnerCell } from './look'
import {
  MERGE_LABEL,
  STEPS,
  ago,
  cwdOf,
  derive,
  findPrUrls,
  isActive,
  mergeArgv,
  mergeError,
  mergeQuestion,
  metaOf,
  nextMethod,
  pickMethod,
  placeholder,
  shellCode,
  stepIndex,
  touched,
} from './pr'
import { buildQuery, chunks, parseReply } from './batch'
import type { RateLimit } from './batch'
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
const WATCH_TOOL = 'mcp__pr-watch__watch'
const FRAME_MS = 90

// Module memory: lost on reload, which only costs one extra poll.
let isTicking = false
let hasHinted = false
// What the pane last drew, so the animation can blit frames of the same size.
const barWidth = new Map<string, number>()
let anim: { cancel: () => void } | null = null
let frame = 0

// Rows kept by an older version of this mod lack the newer fields.
function full(pr: TrackedPr): TrackedPr {
  return { ...placeholder(pr), ...pr, checks: { ...placeholder(pr).checks, ...pr.checks } }
}

const TOAST_ICON: Record<string, string> = { success: '✓', error: '✗', warning: '◷', merged: '🎉', suggestion: '⇢' }

// ---- One poller for the whole machine -------------------------------------------------
// Every session lists the PRs it watches under ~/.cache/pr-watch/sessions/ (the file's mtime
// is its heartbeat); whichever session finds the shared results due takes a short lease and
// fetches every live session's PRs in one batched query, and the rest read results.json.
// So N sessions cost one GitHub call per round, not N.

type Entry = { at: number; pr?: GhPr; error?: string }
type Shared = {
  fetchedAt: number
  nextAt: number
  prs: Record<string, Entry>
  // Which gh login sees an owner's private repos (a login, never a token).
  accounts: Record<string, string>
  rate: RateLimit | null
}

// How often a round runs while some PR moves, and otherwise: the `poll_active_seconds` and
// `poll_idle_seconds` options, kept current by `config.set`.
const MIN_POLL_MS = 10_000
const pace = { fastMs: 20_000, slowMs: 60_000 }
const seconds = (value: unknown, fallback: number) =>
  typeof value === 'number' && Number.isFinite(value) ? Math.max(MIN_POLL_MS, value * 1_000) : fallback
// A session whose file has not been touched for this long has exited.
const ALIVE_MS = 60_000
const LEASE_MS = 30_000
// A PR no account can see is asked about again only this often.
const UNREACHABLE_MS = 300_000
const LOW_RATE = 300
const PRUNE_MS = 86_400_000

// Survives /clear (the module stays loaded), unlike the session id.
const INSTANCE = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`
let dir: string | null = null
let loginsCache: Promise<{ active: string; others: string[] }> | null = null
const tokens = new Map<string, Promise<string | null>>()
let prunedAt = 0

const empty = (): Shared => ({ fetchedAt: 0, nextAt: 0, prs: {}, accounts: {}, rate: null })

async function gh($: EngineInterface, argv: string[], token?: string | null, cwd?: string) {
  return $.process.run(['gh', ...argv], {
    cwd,
    timeoutMs: 20_000,
    env: token ? { GH_TOKEN: token } : undefined,
  })
}

async function root($: EngineInterface): Promise<string> {
  dir ??= `${(await $.env.get('HOME')) ?? '/tmp'}/.cache/pr-watch`

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

async function query($: EngineInterface, refs: readonly PrRef[], token: string | null) {
  const out = { found: new Map<string, GhPr>(), missing: [] as PrRef[], failed: new Map<string, string>(), rate: null as RateLimit | null }
  for (const part of chunks(refs)) {
    const res = await gh($, ['api', 'graphql', '-f', `query=${buildQuery(part)}`], token)
    const reply = parseReply(res.stdout, res.stderr, part)
    if (reply.error !== null) for (const ref of part) out.failed.set(ref.url, reply.error)
    for (const [url, pr] of reply.found) out.found.set(url, pr)
    out.missing.push(...reply.missing)
    out.rate = reply.rate ?? out.rate
  }

  return out
}

// One batched query per gh account; PRs the expected account cannot see are retried with
// each other logged-in account, and the one that sees them is remembered per owner.
async function fetchRefs($: EngineInterface, refs: readonly PrRef[], accounts: Record<string, string>) {
  const found = new Map<string, GhPr>()
  const errors = new Map<string, string>()
  let rate: RateLimit | null = null
  const byLogin = new Map<string, PrRef[]>()
  for (const ref of refs) byLogin.set(accounts[ownerOf(ref)] ?? '', [...(byLogin.get(accounts[ownerOf(ref)] ?? '') ?? []), ref])

  let missing: PrRef[] = []
  for (const [login, group] of byLogin) {
    const r = await query($, group, await tokenFor($, login))
    r.found.forEach((pr, url) => found.set(url, pr))
    r.failed.forEach((err, url) => errors.set(url, err))
    missing.push(...r.missing)
    rate = r.rate ?? rate
  }
  if (missing.length > 0) {
    const { active, others } = await logins($)
    for (const login of [active, ...others]) {
      const untried = missing.filter(ref => (accounts[ownerOf(ref)] ?? '') !== login && (login !== active || ownerOf(ref) in accounts))
      if (untried.length === 0) continue
      const r = await query($, untried, await tokenFor($, login))
      r.found.forEach((pr, url) => {
        found.set(url, pr)
        const ref = untried.find(x => x.url === url)
        if (ref !== undefined) accounts[ownerOf(ref)] = login
      })
      missing = missing.filter(ref => !r.found.has(ref.url))
      if (missing.length === 0) break
    }
    for (const ref of missing) errors.set(ref.url, 'Not found, or no gh account can see it')
  }

  return { found, errors, rate }
}

const refFromUrl = (url: string): PrRef | null => {
  const m = url.match(/^https:\/\/github\.com\/([\w.-]+\/[\w.-]+)\/pull\/(\d+)$/)

  return m === null ? null : { url, repo: m[1] ?? '', number: Number(m[2]) }
}

// Fetches `refs` now and merges them into the shared results, which it returns.
async function publish($: EngineInterface, refs: readonly PrRef[], at: number, isRound = false): Promise<Shared> {
  const before = await readShared($)
  const { found, errors, rate } = await fetchRefs($, refs, before.accounts)
  // Re-read: another session may have written while this one was asking GitHub.
  const shared = await readShared($)
  shared.accounts = { ...shared.accounts, ...before.accounts }
  found.forEach((pr, url) => (shared.prs[url] = { at, pr }))
  errors.forEach((error, url) => (shared.prs[url] = { ...shared.prs[url], at, error }))
  shared.rate = rate ?? shared.rate
  if (isRound) {
    const isMoving = [...found.values()].some(pr => derive(pr).isMoving)
    shared.fetchedAt = at
    shared.nextAt = at + (isMoving ? pace.fastMs : pace.slowMs)
    if (shared.rate !== null && shared.rate.remaining < LOW_RATE) {
      shared.nextAt = Math.max(shared.nextAt, Date.parse(shared.rate.resetAt) || 0)
    }
  }
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
    const due = [...urls].flatMap(url => {
      const entry = shared.prs[url]
      const isUnreachable = entry?.error !== undefined && entry.pr === undefined && at - entry.at < UNREACHABLE_MS
      const isFinished = entry?.pr !== undefined && entry.pr.state !== 'OPEN'
      const ref = refFromUrl(url)

      return ref === null || isUnreachable || isFinished ? [] : [ref]
    })

    return await publish($, due, at, true)
  } finally {
    await releaseLease($)
  }
}

// ---- This session's cards -------------------------------------------------------------

const refOf = (pr: TrackedPr): PrRef => ({ url: pr.url, repo: pr.repo, number: pr.number })

// Folds the shared results into this session's cards: newer entries only, and a toast when
// a card's state changes.
async function apply($: EngineInterface, shared: Shared) {
  const at = await $.clock.now()
  const list = (await read($, prs)).map(full)
  const next = list.map(current => {
    const entry = shared.prs[current.url]
    if (entry === undefined || (current.checkedAt !== null && entry.at <= current.checkedAt)) return current
    if (entry.pr === undefined) return { ...current, error: entry.error ?? null, checkedAt: current.checkedAt ?? entry.at }
    const pr: TrackedPr = {
      ...current,
      title: entry.pr.title,
      state: entry.pr.state,
      ...derive(entry.pr),
      ...metaOf(entry.pr),
      checkedAt: entry.at,
      error: entry.error ?? null,
    }
    if (current.checkedAt !== null && current.pill !== pr.pill && pr.error === null) {
      $.ui.toast(`${TOAST_ICON[pr.tone] ?? '•'} PR #${pr.number} · ${pr.headline}`)
    }

    return pr
  })
  await update($, prs, () => next)
  await update($, now, () => at)
  await refreshStatus($)
  await syncAnimation($)
}

// Fetches these PRs now (a new watch, the Refresh button) rather than waiting for the round.
async function fetchNow($: EngineInterface, refs: PrRef[]) {
  if (refs.length === 0) return
  await apply($, await publish($, refs, await $.clock.now()))
}

async function setAsk($: EngineInterface, url: string, ask: MergeAsk | null) {
  await update($, merging, all => {
    const { [url]: _, ...rest } = all

    return ask === null ? rest : { ...rest, [url]: ask }
  })
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
  await setAsk($, pr.url, { method, busy: BUSY[run.action] })
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
  await setAsk($, pr.url, ask)
  await fetchNow($, [refOf(pr)])
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
      for (const pr of moving) {
        void $.ui.blit({ requestId: PANE, key: `spin:${pr.url}`, cells: spinnerCell(frame + pr.number) })
        const width = barWidth.get(pr.url)
        if (width !== undefined && pr.checks.pending > 0) {
          void $.ui.blit({ requestId: PANE, key: `bar:${pr.url}`, cells: barCells(pr.checks, width, frame) })
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
  barWidth.delete(url)
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
  if (!(await $.ui.panes()).some(pane => pane.id === PANE)) await $.ui.open(OPEN)
  await refreshStatus($)
  await syncAnimation($)
}

export const register: Register = (on, options) => {
  pace.fastMs = seconds(options.poll_active_seconds, 20_000)
  pace.slowMs = seconds(options.poll_idle_seconds, 60_000)

  // A change in /config applies from the next round, no reload needed.
  on('config.set', async ($, e, next) => {
    const done = await next(e)
    const field = e.key.match(/^pr-watch(?:@[\w.-]+)?\.(poll_active_seconds|poll_idle_seconds)$/)?.[1]
    if (field === undefined || done.deny !== undefined) return done
    if (field === 'poll_active_seconds') pace.fastMs = seconds(done.value, pace.fastMs)
    else pace.slowMs = seconds(done.value, pace.slowMs)

    return done
  }).catch(($, e, next) => next(e))

  on('session.start', async ($, e, next) => {
    await $.command.register({
      name: 'pr-watch',
      description: 'Open the PR pane; /pr-watch <url> watches a PR, /pr-watch stop <number|url|all> stops',
      argumentHint: '[<pr url> | stop <number|url|all>]',
    })
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
      const active = (await read($, prs)).map(full).filter(isActive)
      if (active.length === 0 || isTicking) return
      isTicking = true
      try {
        await apply($, await tick($, active.map(refOf), await $.clock.now()))
      } finally {
        isTicking = false
      }
    })
    if ((await read($, prs)).length > 0) {
      void $.ui.open(OPEN)
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
    const { Box, Text, Button, Link } = $.ui.resolve(e)
    const Raster = e.surface === 'terminal' ? $.ui.resolve(e).Raster : null
    const list = (await read($, prs)).map(full)
    const at = await read($, now)
    const asks = await read($, merging)
    const refusedBy = await read($, refused)
    const width = Math.max(24, e.props.bodyColumns)
    const inner = width - 4 // card border + padding
    const open = list.filter(isActive).length
    const done = list.length - open

    const header = (
      <Box flexDirection="column" marginBottom={1}>
        <Box justifyContent="space-between">
          <Text bold>
            <Text color="merged">⎇</Text> Pull requests
          </Text>
          <Text>
            {open > 0 && (
              <Text>
                <Text color="warning" bold>{open}</Text>
                <Text dimColor> open</Text>
              </Text>
            )}
            {open > 0 && done > 0 && <Text dimColor> · </Text>}
            {done > 0 && (
              <Text>
                <Text color="merged" bold>{done}</Text>
                <Text dimColor> done</Text>
              </Text>
            )}
          </Text>
        </Box>
        {Raster !== null ? (
          <Raster key="rule" columns={width} rows={1} cells={ruleCells(width)} />
        ) : (
          <Text color="merged" dimColor>{'─'.repeat(width)}</Text>
        )}
      </Box>
    )

    if (list.length === 0) {
      return (
        <Box flexDirection="column">
          {header}
          <Box flexDirection="column" alignItems="center" paddingY={1}>
            <Text color="merged">╭─────╮</Text>
            <Text color="merged">│ ⎇ ◌ │</Text>
            <Text color="merged">╰─────╯</Text>
            <Text bold>Nothing to watch yet</Text>
            <Text dimColor>PRs appear when Claude runs gh pr …</Text>
            <Text dimColor>or with /pr-watch &lt;url&gt;</Text>
          </Box>
        </Box>
      )
    }

    // Merge, or auto-merge, behind a question; or auto-merge's state with a way to turn it off.
    const mergeRow = (pr: TrackedPr) => {
      const ask = asks[pr.url] ?? {}
      const methods = pr.merge.methods.filter(m => !(refusedBy[pr.repo] ?? []).includes(m))
      const method = pickMethod(methods, ask.method)
      const error = ask.error !== undefined && (
        <Text color="error" wrap="wrap">
          {`✗ ${ask.error}`}
        </Text>
      )
      if (ask.busy !== undefined) return <Text color="suggestion">{`◌ ${ask.busy}`}</Text>
      if (pr.merge.isAuto) {
        return (
          <Box flexDirection="column">
            {error}
            <Box justifyContent="space-between">
              <Text color="suggestion">{`⇢ Auto-merge on${pr.merge.autoMethod === null ? '' : ` · ${pr.merge.autoMethod}`}`}</Text>
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
                // One question at a time, so y and n answer the one on screen.
                await update($, merging, all => Object.fromEntries(Object.entries(all).map(([url, a]) => [url, { ...a, asking: undefined }])))
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

    const card = (pr: TrackedPr) => {
      const step = stepIndex(pr.stage)
      const isMerged = pr.stage === 'merged'
      const doneColour = isMerged ? 'merged' : 'success'
      const icon = [...pr.pill][0] ?? '●'
      // Labels take 25 cells and the glyphs/gaps 10; connectors share what is left.
      const link = Math.min(6, Math.max(1, Math.floor((inner - 35) / 3)))
      const label = `${pr.checks.passed}/${pr.checks.total}`
      const barW = Math.max(6, inner - label.length - 1)
      const hasBar = (pr.state === 'OPEN' && pr.checks.total > 0) || isMerged

      if (hasBar && !isMerged) barWidth.set(pr.url, barW)

      const glyph = (i: number) => {
        if (i < step) return <Text color={doneColour}>✓</Text>
        if (i > step) return <Text dimColor>○</Text>
        if (pr.isMoving && Raster !== null) {
          return <Raster key={`spin:${pr.url}`} columns={1} rows={1} cells={spinnerCell(frame + pr.number)} />
        }

        return <Text color={pr.tone} bold>{pr.isMoving ? '◌' : icon}</Text>
      }

      return (
        <Box
          key={`card:${pr.url}`}
          flexDirection="column"
          borderStyle="round"
          borderColor={pr.tone}
          borderDimColor
          hover={{ borderDimColor: false }}
          paddingX={1}
          marginBottom={1}
        >
          <Box justifyContent="space-between">
            <Text backgroundColor={pr.tone} color="inverseText" bold>
              {' '}
              {pr.pill}{' '}
            </Text>
            <Text dimColor wrap="truncate-start">
              {pr.repo}
            </Text>
          </Box>

          <Box>
            <Text wrap="truncate-end">
              <Text color={pr.tone} bold>
                #{pr.number}
              </Text>{' '}
              <Text bold>{pr.title || 'Loading…'}</Text>
            </Text>
          </Box>
          {pr.branch !== '' && (
            <Text wrap="truncate-end">
              <Text color="suggestion">{pr.branch}</Text>
              <Text dimColor> → </Text>
              <Text dimColor>{pr.base}</Text>
              <Text dimColor>{'  '}</Text>
              <Text color="success">+{pr.additions}</Text> <Text color="error">−{pr.deletions}</Text>
              {pr.author !== '' && <Text dimColor>{'  '}@{pr.author}</Text>}
            </Text>
          )}

          <Box marginTop={1}>
            {STEPS.map((name, i) => (
              <Box>
                {glyph(i)}
                <Text
                  color={i < step ? doneColour : i === step ? pr.tone : undefined}
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
                  cells={isMerged ? fullBarCells(barW, PALETTE.mauve) : barCells(pr.checks, barW, frame)}
                />
              ) : (
                <Text>
                  {(isMerged ? [{ text: '━'.repeat(barW), colour: 'merged' as const }] : barRuns(pr.checks, barW)).map(r => (
                    <Text color={r.colour}>{r.text}</Text>
                  ))}
                </Text>
              )}
              {!isMerged && <Text dimColor> {label}</Text>}
            </Box>
          )}

          <Text color={pr.tone} wrap="wrap">
            {pr.headline}
            {isMerged && pr.mergedAt !== null && <Text dimColor> · {ago(at - Date.parse(pr.mergedAt))}</Text>}
          </Text>
          {pr.checks.failing.length > 2 &&
            pr.checks.failing.slice(2, 5).map(name => (
              <Text color="error" dimColor wrap="truncate-end">
                {'  '}✗ {name}
              </Text>
            ))}
          {pr.error !== null && (
            <Text color="error" wrap="truncate-end">
              ⚠ {pr.error}
            </Text>
          )}
          {pr.state === 'OPEN' && mergeRow(pr)}

          <Box justifyContent="space-between">
            <Text>
              <Link href={pr.url} label="open ↗" />
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
        </Box>
      )
    }

    return (
      <Box flexDirection="column">
        {header}
        {list.map(card)}
        <Box gap={2}>
          <Button key="refresh" label="Refresh" hotkey="r" plain dimColor onPress={() => fetchNow($, list.filter(isActive).map(refOf))} />
          {done > 0 && (
            <Button key="dismiss-finished" label="Dismiss finished" hotkey="d" plain dimColor onPress={() => dismissFinished($)} />
          )}
        </Box>
      </Box>
    )
  })
}
