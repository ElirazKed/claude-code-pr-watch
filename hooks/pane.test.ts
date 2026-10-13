import { expect, mock, test } from 'claude-code/testing'
import type { TestBody } from 'claude-code/testing'
import type { RenderElement } from 'claude-code'

const RUNNING = 'https://github.com/acme/app/pull/12'
const FAILING = 'https://github.com/acme/app/pull/13'
const MERGED = 'https://github.com/acme/api/pull/7'

const check = (name: string, status: string, conclusion = '') => ({ __typename: 'CheckRun', name, status, conclusion })
const pr = (url: string, extra: Record<string, unknown>) => ({
  number: Number(url.split('/').pop()),
  title: 'Retry uploads with exponential backoff',
  url,
  state: 'OPEN',
  isDraft: false,
  reviewDecision: 'REVIEW_REQUIRED',
  mergeStateStatus: 'BLOCKED',
  autoMergeRequest: null,
  statusCheckRollup: [],
  mergedAt: null,
  additions: 120,
  deletions: 14,
  headRefName: 'feat/retry',
  baseRefName: 'main',
  author: { login: 'octocat' },
  ...extra,
})

const GH: Record<string, unknown> = {
  [RUNNING]: pr(RUNNING, {
    statusCheckRollup: [check('lint', 'COMPLETED', 'SUCCESS'), check('build', 'IN_PROGRESS'), check('e2e', 'QUEUED')],
  }),
  [FAILING]: pr(FAILING, { statusCheckRollup: [check('build', 'COMPLETED', 'FAILURE'), check('lint', 'COMPLETED', 'SUCCESS')] }),
  [MERGED]: pr(MERGED, { state: 'MERGED', mergedAt: '2026-10-07T10:00:00Z', reviewDecision: 'APPROVED' }),
}

const PANE = {
  plugin: 'pr-watch',
  component: 'Pane',
  requestId: 'pr-watch',
  props: {
    title: 'Pull requests',
    isFocused: false,
    bodyColumns: 60,
    placement: 'dock',
    scroll: { offset: 0, bodyRows: 60 },
    view: {},
  },
  viewport: { columns: 180, rows: 60, isFullscreen: true },
} as const

type On = Parameters<TestBody>[1]

type Clock = ReturnType<typeof mock.clock>

const FAILED = new Set(['ACTION_REQUIRED', 'CANCELLED', 'FAILURE', 'STALE', 'STARTUP_FAILURE', 'TIMED_OUT'])

type Said = { login: string; state: string; at: string; isBot?: boolean }
type Thread = { isResolved: boolean; isOutdated: boolean }

// The review connections GitHub answers the poller with, from a fixture's `reviews` (every
// review, oldest first), `requested` (logins with a request pending) and `threads`. As GitHub
// does, the latest reviews leave out a reviewer with a request pending.
function reviewsGql(fixture: Record<string, unknown>) {
  const said = (fixture.reviews ?? []) as Said[]
  const requested = (fixture.requested ?? []) as string[]
  const threads = (fixture.threads ?? []) as Thread[]
  const node = (r: Said) => ({ author: { __typename: r.isBot ? 'Bot' : 'User', login: r.login }, state: r.state, submittedAt: r.at })
  const latest = [...new Map(said.map(r => [r.login, r])).values()].filter(r => !requested.includes(r.login)).map(node)
  const opinionated = [...new Map(said.filter(r => r.state !== 'COMMENTED').map(r => [r.login, r])).values()].map(node)

  return {
    latestReviews: { totalCount: latest.length, nodes: latest },
    latestOpinionatedReviews: { nodes: opinionated },
    reviewRequests: { totalCount: requested.length, nodes: requested.map(login => ({ requestedReviewer: { __typename: 'User', login } })) },
    reviewThreads: { totalCount: threads.length, nodes: threads },
  }
}

// gh's GraphQL shape for a fixture: the repository alias with its merge settings, and the PR
// in it, checks nested under the head commit. A check run's title and summary come only by
// the commit's check suites, for failed runs, and only when `query` asks for them there; the
// reviews only when it asks for them (the poller's query).
const asGql = (fixture: Record<string, unknown>, query = '') => {
  const { statusCheckRollup, repository, reviews: _r, requested: _q, threads: _t, ...rest } = fixture
  const all = statusCheckRollup as Record<string, unknown>[]
  const nodes = all.map(({ title: _t, summary: _s, ...item }) => item)
  const failed = all
    .filter(item => FAILED.has(String(item.conclusion)) && typeof item.databaseId === 'number')
    .map(item => ({ databaseId: item.databaseId, title: item.title ?? null, summary: item.summary ?? null }))
  const suites = query.includes('checkSuites') ? { checkSuites: { nodes: [{ checkRuns: { nodes: failed } }] } } : {}
  const commits = { nodes: [{ commit: { statusCheckRollup: { contexts: { nodes } }, ...suites } }] }

  const reviews = query.includes('reviewThreads') ? reviewsGql(fixture) : {}

  return { ...(repository as object | undefined), pullRequest: { ...rest, closedAt: null, commits, ...reviews } }
}

type MergeReply = { exitCode: number; stderr?: string }
type RunReply = { exitCode: number; stdout?: string; stderr?: string }

// A job's failed step as `gh run view --job <id> --log-failed` prints it.
const FAILED_LOG = [
  'build\tRun tests\t2026-10-07T11:58:01.1000000Z ##[group]Run npm test',
  'build\tRun tests\t2026-10-07T11:58:02.2000000Z \u001b[31mFAIL\u001b[0m src/upload.test.ts',
  'build\tRun tests\t2026-10-07T11:58:02.3000000Z Error: expected 3 retries, got 1',
].join('\n')

// What GitHub does when a `gh pr merge` goes through: the PR merges, or auto-merge turns on or off.
function merged(gh: Record<string, unknown>, argv: readonly string[]) {
  const url = String(argv[3])
  const fixture = gh[url] as Record<string, unknown>
  const method = argv.find(arg => /^--(squash|rebase|merge)$/.test(arg))?.slice(2).toUpperCase()
  if (argv.includes('--disable-auto')) gh[url] = { ...fixture, autoMergeRequest: null }
  else if (argv.includes('--auto')) gh[url] = { ...fixture, autoMergeRequest: { enabledAt: '2026-10-07T12:00:00Z', mergeMethod: method } }
  else gh[url] = { ...fixture, state: 'MERGED', mergedAt: '2026-10-07T12:00:00Z' }
}

type Blip = { stderr: string; afterMs?: number; status?: number; headers?: string[]; body?: string }
// What `gh api -i` prints before the body: the status line ("\n"), each header ("\r\n"), a blank line.
const httpHead = (status: string, headers: readonly string[]) => `HTTP/2.0 ${status}\n${headers.map(line => `${line}\r\n`).join('')}\r\n`
// When the hour's points and requests reset, in every fixture.
const POINTS_RESET = Date.parse('2026-10-07T13:00:00Z')
const REQUESTS_RESET = Date.parse('2026-10-07T13:30:00Z')
// The rate headers of a reply that got through, as the body's rateLimit field says it.
const rateHeaders = (remaining: number) => [
  'X-Ratelimit-Limit: 5000',
  `X-Ratelimit-Remaining: ${remaining}`,
  `X-Ratelimit-Reset: ${POINTS_RESET / 1_000}`,
  'X-Ratelimit-Resource: graphql',
  `X-Ratelimit-Used: ${5_000 - remaining}`,
]

const ALIAS = /p(\d+): repository\(owner: "([^"]+)", name: "([^"]+)"\) \{ pullRequest\(number: (\d+)\)/g

// Stands for gh, $HOME and the file system: answers batched queries from `gh`, keeps files in
// memory (stamped with the mocked time, so mtimes are heartbeats), and records each gh argv.
// `branchPr`: what `gh pr view` answers for the current branch. `merge`: how `gh pr merge`
// ends (now, or once a promise settles); by default it goes through, and `merges` records each
// one's argv. `runLog`: what
// `gh run view` prints, a failed job's log by default; `runs` records each argv. `asked` records
// each GraphQL query's text, and a query for what threads say waits on `hold`, when set. The
// mod's own plugin.json, wherever its folder is, reads as the file kept under 'plugin.json';
// `gh.pw`, when set, is GitHub's answer to a round's pw field (pr-watch's manifest there), and
// without it the repo can't be read, as when it is renamed, gone or private. While `blips`
// holds a failure, each GraphQL query takes the first, `afterMs` (mocked time) on: gh's
// stderr for it, and on stdout its `body` (a gateway's HTML page by default), after its
// status (the one stderr names) and `headers` where gh is run with -i, as `gh api -i` prints
// them; a blip with no status is a cut connection, with nothing on stdout. A reply that gets
// through prints its status and rate headers too, under -i. `budget.logins` are gh's logins,
// the first active (another's token is "<login>-token"), and `budget.points` and
// `budget.requests` what each has left (4000 and 4900 of 5000 by default), as the replies and
// `gh api rate_limit` say; `restAsks` records each login /rate_limit was asked for. The time
// zone (`date +%z`) is +0300.
function fakeHost(
  on: On,
  clock: Clock,
  gh: Record<string, unknown>,
  files = new Map<string, { text: string; mtimeMs: number }>(),
  branchPr?: string,
  merge: (argv: readonly string[]) => MergeReply | Promise<MergeReply> = () => ({ exitCode: 0 }),
  runLog: (argv: readonly string[]) => RunReply | Promise<RunReply> = () => ({ exitCode: 0, stdout: FAILED_LOG }),
) {
  const queries: string[][] = []
  const merges: string[][] = []
  const runs: string[][] = []
  const asked: string[] = []
  const gate: { hold: Promise<unknown> | null } = { hold: null }
  const blips: Blip[] = []
  const budget = { logins: ['me'], points: {} as Record<string, number>, requests: {} as Record<string, number> }
  const restAsks: string[] = []
  // Each GraphQL call's timeout.
  const timeouts: number[] = []
  on('env.get', () => ({ value: '/home/me' }))
  const own = (path: string) => (path.endsWith('/.claude-plugin/plugin.json') ? files.get('plugin.json') : undefined)
  on('fs.read', ($, e) => ({ value: (files.get(e.path) ?? own(e.path))?.text ?? '' }))
  on('fs.write', ($, e) => (files.set(e.path, { text: e.text, mtimeMs: clock.now() }), { value: undefined }))
  on('fs.list', ($, e) => ({
    value: [...files]
      .filter(([path]) => path.startsWith(`${e.path}/`) && !path.slice(e.path.length + 1).includes('/'))
      .map(([path, f]) => ({ name: path.slice(e.path.length + 1), kind: 'file' as const, size: f.text.length, mtimeMs: f.mtimeMs, isLink: false })),
  }))
  on('process.run', ($, e) => {
    const out = (exitCode: number, stdout: string, stderr = '') => ({
      value: { exitCode, stdout, stderr, isStdoutTruncated: false, isStderrTruncated: false },
    })
    if (e.argv[1] === 'pr' && e.argv[2] === 'merge') {
      merges.push([...e.argv])

      return Promise.resolve(merge(e.argv)).then(reply => {
        if (reply.exitCode === 0) merged(gh, e.argv)

        return out(reply.exitCode, '', reply.stderr)
      })
    }
    if (e.argv[1] === 'run' && e.argv[2] === 'view') {
      runs.push([...e.argv])

      return Promise.resolve(runLog(e.argv)).then(reply => out(reply.exitCode, reply.stdout ?? '', reply.stderr))
    }
    if (e.argv[0] === 'date') return out(0, '+0300\n')
    if (e.argv[1] === 'auth' && e.argv[2] === 'token') return out(0, `${e.argv[e.argv.indexOf('--user') + 1]}-token\n`)
    if (e.argv[1] === 'auth') {
      return out(0, budget.logins.map((login, i) => `  ✓ Logged in to github.com account ${login} (keyring)\n  - Active account: ${i === 0}\n`).join(''))
    }
    // Who asks: another login's token, or the active one.
    const login = e.init?.env?.GH_TOKEN?.replace(/-token$/, '') ?? budget.logins[0] ?? ''
    if (e.argv[1] === 'api' && e.argv[2] === 'rate_limit') {
      restAsks.push(login)
      const one = (remaining: number, reset: number) => ({ limit: 5_000, used: 5_000 - remaining, remaining, reset: reset / 1_000 })
      const resources = { core: one(budget.requests[login] ?? 4_900, REQUESTS_RESET), graphql: one(budget.points[login] ?? 4_000, POINTS_RESET) }

      return out(0, JSON.stringify({ resources, rate: resources.core }))
    }
    if (e.argv[1] === 'pr' && e.argv[2] === 'view') return branchPr ? out(0, `${branchPr}\n`) : out(1, '')
    if (e.argv[1] !== 'api') return out(1, '')
    const query = String(e.argv[4] ?? '').slice('query='.length)
    asked.push(query)
    timeouts.push(e.init?.timeoutMs ?? 0)
    const urls: string[] = []
    const data: Record<string, unknown> = { rateLimit: { limit: 5000, remaining: budget.points[login] ?? 4000, resetAt: '2026-10-07T13:00:00Z' } }
    for (const [, i, owner, name, num] of query.matchAll(ALIAS)) {
      const url = `https://github.com/${owner}/${name}/pull/${num}`
      urls.push(url)
      const fixture = gh[url] as Record<string, unknown> | undefined
      data[`p${i}`] = fixture === undefined ? null : asGql(fixture, query)
    }
    const isHead = e.argv.includes('-i')
    const blip = blips.shift()
    if (blip !== undefined) {
      queries.push(urls)
      const status = blip.status ?? Number(blip.stderr.match(/HTTP (\d{3})/)?.[1] ?? 0)
      const body = blip.body ?? '<html><body><h1>Bad Gateway</h1></body></html>'
      const stdout = !isHead ? body : status === 0 ? '' : httpHead(`${status} Bad Gateway`, blip.headers ?? []) + body

      return clock.sleep(blip.afterMs ?? 0).then(() => out(1, stdout, `${blip.stderr}\n`))
    }
    const errors: unknown[] = []
    if (query.includes('pw: repository(')) {
      data.pw = gh.pw ?? null
      if (gh.pw === undefined) errors.push({ type: 'NOT_FOUND', path: ['pw'], message: 'Could not resolve to a Repository' })
    }
    queries.push(urls)
    const body = JSON.stringify(errors.length === 0 ? { data } : { data, errors })
    const reply = out(urls.every(url => url in gh) && errors.length === 0 ? 0 : 1, isHead ? httpHead('200 OK', rateHeaders(budget.points[login] ?? 4_000)) + body : body)

    return gate.hold !== null && query.includes('body') ? gate.hold.then(() => reply) : reply
  })

  return { queries, files, merges, runs, asked, gate, blips, budget, restAsks, timeouts }
}

test('cards show every lifecycle state, animate CI, and dismiss when merged', async ($, on) => {
  const clock = mock.clock(on)
  fakeHost(on, clock, GH)
  on('ui.open', () => ({ value: { isPlaced: true } }))
  on('ui.close', () => ({ value: undefined }))
  on('ui.toast', () => ({ value: undefined }))
  on('ui.status', () => ({ value: undefined }))
  on('ui.blit', () => ({ value: {} }))
  await $.command.run({
    command: 'pr-watch',
    args: `${RUNNING} ${FAILING} ${MERGED}`,
    origin: { kind: 'composer' },
    presentation: { isFullscreen: true, columns: 180 },
  })

  for (const surface of ['terminal', 'desktop'] as const) {
    const ui = await $.ui.mount({ ...PANE, surface })
    await ui.press({ key: 'refresh' })
    // Each card links to its own PR, under its own label (a Link carries no key).
    const links = await ui.findAll({ type: 'Link' })
    expect(new Set(links.map(link => link.props.href)).size).toBe(3)
    expect(new Set(links.map(link => link.props.label)).size).toBe(3)

    expect(await ui.find({ text: /CI RUNNING/ })).toBeDefined()
    expect(await ui.find({ text: /CI FAILED/ })).toBeDefined()
    expect(await ui.find({ text: /MERGED/ })).toBeDefined()
    expect(await ui.find({ text: /Failing: build/ })).toBeDefined()
    expect(await ui.find({ text: /Running build · 1 queued · 1\/3 passed/ })).toBeDefined()
    expect(await ui.find({ text: /feat\/retry/ })).toBeDefined()
    if (surface === 'terminal') {
      expect(await ui.find({ key: `spin:${RUNNING}` })).toBeDefined()
      expect(await ui.find({ key: `bar:${RUNNING}` })).toBeDefined()
    }
    await ui.unmount()
  }

  const stopped = await $.command.run({
    command: 'pr-watch',
    args: 'stop #13',
    origin: { kind: 'composer' },
    presentation: { isFullscreen: true, columns: 180 },
  })
  expect(stopped.text).toBe('Stopped watching #13.')

  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await ui.find({ text: /CI FAILED/ })).toBeUndefined()
  await ui.press({ key: `dismiss:${MERGED}` })
  expect(await ui.find({ text: /✓ MERGED/ })).toBeUndefined()
  expect(await ui.find({ text: /CI RUNNING/ })).toBeDefined()
  await ui.unmount()
})

test('cards for the same number in two repos keep their links apart', async ($, on) => {
  const TWIN = 'https://github.com/acme/api/pull/12'
  const clock = mock.clock(on)
  fakeHost(on, clock, { [RUNNING]: GH[RUNNING], [TWIN]: pr(TWIN, {}) })
  on('ui.open', () => ({ value: { isPlaced: true } }))
  on('ui.toast', () => ({ value: undefined }))
  on('ui.status', () => ({ value: undefined }))
  on('ui.blit', () => ({ value: {} }))
  await $.command.run({ command: 'pr-watch', args: `${RUNNING} ${TWIN}`, origin: { kind: 'composer' }, presentation: { isFullscreen: true, columns: 180 } })

  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  await ui.press({ key: 'refresh' })
  const labels = (await ui.findAll({ type: 'Link' })).map(link => link.props.label).sort()
  expect(labels).toEqual(['open acme/api#12 ↗', 'open acme/app#12 ↗'])
  await ui.unmount()
})

test('a PR touched outside gh pr is offered once, and the watch tool adds it', async ($, on) => {
  const clock = mock.clock(on)
  fakeHost(on, clock, GH)
  on('ui.open', () => ({ value: { isPlaced: true } }))
  on('ui.toast', () => ({ value: undefined }))
  on('ui.status', () => ({ value: undefined }))
  on('ui.blit', () => ({ value: {} }))
  on('tool.call', { tool: 'WebFetch' }, () => ({ result: { ok: true }, text: 'PR page' }))

  const fetch = { tool: 'WebFetch', url: RUNNING, prompt: 'what is its status?' } as const
  const first = await $.tool.call(fetch)
  expect(first.context?.join(' ')).toContain('acme/app#12')
  const again = await $.tool.call(fetch)
  expect(again.context ?? []).toEqual([])

  const watched = await $.tool.call({ tool: 'mcp__pr-watch__watch', urls: [RUNNING] } as never)
  expect(watched.result).toBe('Watching #12 in the PR pane.')
  const check = await $.tool.check({ tool: 'mcp__pr-watch__watch', input: { urls: [RUNNING] } })
  expect(check.decision).toBe('allow')
})

test('a /clear keeps the watched PRs and seats the pane again', async ($, on) => {
  const clock = mock.clock(on)
  fakeHost(on, clock, GH)
  let opens = 0
  on('ui.open', () => ((opens += 1), { value: { isPlaced: true } }))
  on('ui.panes', () => ({ value: [] }))
  on('ui.toast', () => ({ value: undefined }))
  on('ui.status', () => ({ value: undefined }))
  on('ui.blit', () => ({ value: {} }))
  // The engine's end step; after it the pane is gone (ui.panes answers none).
  on('session.end', ($, e) => ({ sessionId: e.sessionId }))

  await $.tool.call({ tool: 'mcp__pr-watch__watch', urls: [RUNNING] } as never)
  opens = 0
  await $.session.end({ reason: 'clear', sessionId: 's1', resume: { id: 's1' } } as never)
  await clock.advance(1_000)

  expect(opens).toBe(1)
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await ui.find({ text: /#12/ })).toBeDefined()
  await ui.unmount()
})

test('Close beside Refresh closes the pane, and the PRs stay watched', async ($, on) => {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  const { queries } = fakeHost(on, clock, GH)
  let opens = 0
  const closes: { id: string; origin: string }[] = []
  on('ui.open', () => ((opens += 1), { value: { isPlaced: true } }))
  on('ui.close', ($, e) => (closes.push({ id: e.id, origin: e.origin.kind }), { value: undefined }))
  on('ui.toast', () => ({ value: undefined }))
  on('ui.status', () => ({ value: undefined }))
  on('ui.blit', () => ({ value: {} }))
  on('command.register', () => ({ value: { command: 'pr-watch' } }))
  on('tool.register', () => ({ value: { tool: 'mcp__pr-watch__watch' } }))
  on('session.start', () => ({ cwd: '/work' }))
  await $.session.start({ source: 'startup', cwd: '/work' } as never)
  await $.command.run({ command: 'pr-watch', args: `${RUNNING} ${MERGED}`, origin: { kind: 'composer' }, presentation: { isFullscreen: true, columns: 180 } })

  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  await ui.press({ key: 'refresh' })
  const footer = (await ui.findAll({ type: 'Button' })).map(b => b.key).filter(key => ['refresh', 'close', 'dismiss-finished'].includes(key ?? ''))
  expect(footer).toEqual(['refresh', 'close', 'dismiss-finished'])
  expect((await ui.find({ key: 'close' }))?.props).toMatchObject({ label: 'Close', hotkey: 'x' })
  await ui.press({ key: 'close' })
  await ui.unmount()
  expect(closes).toEqual([{ id: 'pr-watch', origin: 'plugin' }])

  // Still polled, still on the cards, and not stopped: closing is not dismissing.
  queries.length = 0
  await clock.advance(10_500)
  expect(queries.flat()).toContain(RUNNING)
  const after = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await after.find({ text: /#12/ })).toBeDefined()
  expect(await after.find({ text: /#7/ })).toBeDefined()
  await after.unmount()

  // A new PR brings the pane back, as after the frame's own close.
  opens = 0
  await $.tool.call({ tool: 'mcp__pr-watch__watch', urls: [FAILING] } as never)
  expect(opens).toBe(1)
})

test('the empty pane has Close too', async ($, on) => {
  const closes: string[] = []
  on('ui.close', ($, e) => (closes.push(e.id), { value: undefined }))
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await ui.find({ text: /Nothing to watch yet/ })).toBeDefined()
  expect((await ui.find({ key: 'close' }))?.props).toMatchObject({ label: 'Close', hotkey: 'x' })
  await ui.press({ key: 'close' })
  await ui.unmount()
  expect(closes).toEqual(['pr-watch'])
})

const quietUi = (on: On) => {
  on('ui.open', () => ({ value: { isPlaced: true } }))
  on('ui.close', () => ({ value: undefined }))
  on('ui.toast', () => ({ value: undefined }))
  on('ui.status', () => ({ value: undefined }))
  on('ui.blit', () => ({ value: {} }))
}

test("one session's round fetches every live session's PRs in a single query", async ($, on) => {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  quietUi(on)
  const files = new Map<string, { text: string; mtimeMs: number }>()
  // Another session, alive (touched just now), watching #13.
  files.set('/home/me/.cache/pr-watch/sessions/other.json', { text: JSON.stringify({ urls: [FAILING] }), mtimeMs: clock.now() })
  // And one that exited an hour ago: its PR is not polled.
  files.set('/home/me/.cache/pr-watch/sessions/gone.json', { text: JSON.stringify({ urls: [MERGED] }), mtimeMs: clock.now() - 3_600_000 })
  const { queries } = fakeHost(on, clock, GH, files)
  on('command.register', () => ({ value: { command: 'pr-watch' } }))
  on('tool.register', () => ({ value: { tool: 'mcp__pr-watch__watch' } }))
  on('session.start', () => ({ cwd: '/work' }))
  await $.session.start({ source: 'startup', cwd: '/work' } as never)

  await $.command.run({ command: 'pr-watch', args: RUNNING, origin: { kind: 'composer' }, presentation: { isFullscreen: true, columns: 180 } })
  await clock.advance(100)
  queries.length = 0
  await clock.advance(10_500)

  expect(queries.map(q => [...q].sort())).toEqual([[RUNNING, FAILING].sort()])
  const shared = JSON.parse(files.get('/home/me/.cache/pr-watch/results.json')?.text ?? '{}')
  expect(Object.keys(shared.prs).sort()).toEqual([FAILING, RUNNING].sort())
})

test('a session reads a fresh round another session ran, and asks GitHub nothing', async ($, on) => {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  quietUi(on)
  const files = new Map<string, { text: string; mtimeMs: number }>()
  const { queries } = fakeHost(on, clock, GH, files)
  on('command.register', () => ({ value: { command: 'pr-watch' } }))
  on('tool.register', () => ({ value: { tool: 'mcp__pr-watch__watch' } }))
  on('session.start', () => ({ cwd: '/work' }))
  await $.session.start({ source: 'startup', cwd: '/work' } as never)

  await $.command.run({ command: 'pr-watch', args: RUNNING, origin: { kind: 'composer' }, presentation: { isFullscreen: true, columns: 180 } })
  await clock.advance(100)
  // Another session ran the round a moment ago: CI on #12 has since gone green.
  const green = { ...(GH[RUNNING] as object), statusCheckRollup: [check('build', 'COMPLETED', 'SUCCESS')] }
  const at = clock.now()
  files.set('/home/me/.cache/pr-watch/results.json', {
    text: JSON.stringify({ fetchedAt: at, nextAt: at + 60_000, prs: { [RUNNING]: { at, pr: green } }, accounts: {}, rate: null }),
    mtimeMs: at,
  })
  queries.length = 0
  await clock.advance(10_500)

  expect(queries).toEqual([])
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await ui.find({ text: /CI RUNNING/ })).toBeUndefined()
  expect(await ui.find({ text: /IN REVIEW|BLOCKED/ })).toBeDefined()
  await ui.unmount()
})

test('a PR Claude only read through gh is offered, not added', async ($, on) => {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  let opens = 0
  on('ui.open', () => ((opens += 1), { value: { isPlaced: true } }))
  on('ui.close', () => ({ value: undefined }))
  on('ui.toast', () => ({ value: undefined }))
  on('ui.status', () => ({ value: undefined }))
  on('ui.blit', () => ({ value: {} }))
  fakeHost(on, clock, GH)
  on('tool.call', { tool: 'Bash' }, () => ({ result: { stdout: RUNNING }, text: `View this pull request on GitHub: ${RUNNING}` }))

  const viewed = await $.tool.call({ tool: 'Bash', command: `gh pr view ${RUNNING}`, description: 'view' } as never)
  await clock.advance(100)

  expect(viewed.context?.join(' ')).toContain('acme/app#12')
  expect(opens).toBe(0)
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await ui.find({ text: /#12/ })).toBeUndefined()
  await ui.unmount()
})

test('a PR Claude opened with gh pr create is watched without asking', async ($, on) => {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  let opens = 0
  on('ui.open', () => ((opens += 1), { value: { isPlaced: true } }))
  on('ui.close', () => ({ value: undefined }))
  on('ui.toast', () => ({ value: undefined }))
  on('ui.status', () => ({ value: undefined }))
  on('ui.blit', () => ({ value: {} }))
  fakeHost(on, clock, GH)
  on('tool.call', { tool: 'Bash' }, () => ({ result: { stdout: RUNNING }, text: `${RUNNING}\n` }))

  const made = await $.tool.call({ tool: 'Bash', command: 'gh pr create --fill', description: 'open PR' } as never)
  await clock.advance(100)

  expect(made.context ?? []).toEqual([])
  expect(opens).toBe(1)
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await ui.find({ text: /#12/ })).toBeDefined()
  await ui.unmount()
})

test('a merged PR Claude opened is never added, however recently it merged', async ($, on) => {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  let opens = 0
  on('ui.open', () => ((opens += 1), { value: { isPlaced: true } }))
  on('ui.close', () => ({ value: undefined }))
  on('ui.toast', () => ({ value: undefined }))
  on('ui.status', () => ({ value: undefined }))
  on('ui.blit', () => ({ value: {} }))
  const OLD = 'https://github.com/acme/app/pull/3'
  fakeHost(on, clock, { ...GH, [OLD]: pr(OLD, { state: 'MERGED', mergedAt: '2026-10-07T11:59:00Z' }) })
  on('tool.call', { tool: 'Bash' }, () => ({ result: { stdout: OLD }, text: OLD }))

  await $.tool.call({ tool: 'Bash', command: 'gh pr create --fill', description: 'open PR' } as never)
  await clock.advance(100)

  expect(opens).toBe(0)
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await ui.find({ text: /#3/ })).toBeUndefined()
  await ui.unmount()
})

test('a git push to a branch with an open PR watches that PR', async ($, on) => {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  quietUi(on)
  fakeHost(on, clock, GH, undefined, RUNNING)
  on('tool.call', { tool: 'Bash' }, () => ({ result: { stdout: '' }, text: 'To github.com:acme/app.git\n   a1..b2  feat/retry -> feat/retry' }))

  const pushed = await $.tool.call({ tool: 'Bash', command: 'cd ~/src/app && git push', description: 'push' } as never)
  await clock.advance(100)

  expect(pushed.context ?? []).toEqual([])
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await ui.find({ text: /#12/ })).toBeDefined()
  await ui.unmount()
})

test('a PR the person stopped watching stays stopped when Claude acts on it again', async ($, on) => {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  quietUi(on)
  fakeHost(on, clock, GH)
  on('tool.call', { tool: 'Bash' }, () => ({ result: { stdout: '' }, text: 'All checks were successful' }))
  const checks = { tool: 'Bash', command: 'gh pr checks 12 -R acme/app', description: 'checks' } as never

  await $.tool.call(checks)
  await clock.advance(100)
  const before = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await before.find({ text: /#12/ })).toBeDefined()
  await before.unmount()
  await $.command.run({ command: 'pr-watch', args: 'stop 12', origin: { kind: 'composer' }, presentation: { isFullscreen: true, columns: 180 } })
  await $.tool.call(checks)
  await clock.advance(100)

  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await ui.find({ text: /#12/ })).toBeUndefined()
  await ui.unmount()
})

// Starts a session watching `url` and runs one round; what the round wrote to results.json.
async function oneRound($: Parameters<TestBody>[0], on: On, url: string) {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  quietUi(on)
  const files = new Map<string, { text: string; mtimeMs: number }>()
  fakeHost(on, clock, GH, files)
  on('command.register', () => ({ value: { command: 'pr-watch' } }))
  on('tool.register', () => ({ value: { tool: 'mcp__pr-watch__watch' } }))
  on('session.start', () => ({ cwd: '/work' }))
  await $.session.start({ source: 'startup', cwd: '/work' } as never)
  await $.command.run({ command: 'pr-watch', args: url, origin: { kind: 'composer' }, presentation: { isFullscreen: true, columns: 180 } })
  await clock.advance(10_500)
  const shared = JSON.parse(files.get('/home/me/.cache/pr-watch/results.json')?.text ?? '{}')

  return { gap: shared.nextAt - shared.fetchedAt, clock, files }
}

test('the poll intervals come from the options', { options: { poll_active_seconds: 15, poll_idle_seconds: 45 } }, async ($, on) => {
  expect((await oneRound($, on, RUNNING)).gap).toBe(15_000)
})

test('the idle interval from the options', { options: { poll_active_seconds: 15, poll_idle_seconds: 45 } }, async ($, on) => {
  expect((await oneRound($, on, FAILING)).gap).toBe(45_000)
})

test('a /config change to the interval applies from the next round', async ($, on) => {
  on('config.set', ($, e) => ({ value: e.value }))
  const { clock, files } = await oneRound($, on, FAILING)
  await $.config.set({ key: 'pr-watch.poll_idle_seconds', value: 120 } as never)
  await clock.advance(60_000)
  const shared = JSON.parse(files.get('/home/me/.cache/pr-watch/results.json')?.text ?? '{}')

  expect(shared.nextAt - shared.fetchedAt).toBe(120_000)
})

// ---- A newer pr-watch -------------------------------------------------------------------

const RESULTS = '/home/me/.cache/pr-watch/results.json'
const NOTICE = '/home/me/.cache/pr-watch/notice.json'
// This copy's manifest, and pr-watch's on GitHub as a round's pw field answers it.
const OWN = { text: JSON.stringify({ name: 'pr-watch', version: '0.8.0' }), mtimeMs: 0 }
const onGitHub = (version: string) => ({ object: { text: JSON.stringify({ name: 'pr-watch', version }) } })
const asksLatest = (query: string) => query.includes('pw: repository(owner: "ElirazKed", name: "claude-code-pr-watch")')
const told = (toasts: readonly string[]) => toasts.filter(text => text.startsWith('↑'))

// A session of pr-watch 0.8.0 watching `urls` on `gh`; `toasts` collects what it toasts. The
// tests watch PRs that don't move, so a round runs once a minute and no spinner ticks.
async function newerSession($: Parameters<TestBody>[0], on: On, gh: Record<string, unknown>, urls: readonly string[], toasts: string[], files = new Map<string, { text: string; mtimeMs: number }>()) {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  on('ui.open', () => ({ value: { isPlaced: true } }))
  on('ui.close', () => ({ value: undefined }))
  on('ui.toast', ($, e) => (toasts.push(e.text), { value: undefined }))
  on('ui.status', () => ({ value: undefined }))
  on('ui.blit', () => ({ value: {} }))
  files.set('plugin.json', OWN)
  const host = fakeHost(on, clock, gh, files)
  on('command.register', () => ({ value: { command: 'pr-watch' } }))
  on('tool.register', () => ({ value: { tool: 'mcp__pr-watch__watch' } }))
  on('session.start', () => ({ cwd: '/work' }))
  await $.session.start({ source: 'startup', cwd: '/work' } as never)
  await $.command.run({ command: 'pr-watch', args: urls.join(' '), origin: { kind: 'composer' }, presentation: { isFullscreen: true, columns: 180 } })
  await clock.advance(100)

  return { ...host, clock }
}

// Makes the last check two hours old, so the next round asks again.
function ageCheck(files: Map<string, { text: string; mtimeMs: number }>, at: number) {
  const shared = JSON.parse(files.get(RESULTS)?.text ?? '{}')
  files.set(RESULTS, { text: JSON.stringify({ ...shared, latest: { ...shared.latest, checkedAt: at - 2 * 3_600_000 } }), mtimeMs: at })
}

test('a round asks for the newest pr-watch every two hours at most, in its first batch only', async ($, on) => {
  const many = Array.from({ length: 21 }, (_, i) => `https://github.com/acme/app/pull/${100 + i}`)
  const { asked, files, clock } = await newerSession($, on, Object.fromEntries(many.map(url => [url, pr(url, {})])), many, [])
  // A new watch asks GitHub at once, without it.
  expect(asked.length).toBeGreaterThan(0)
  expect(asked.some(asksLatest)).toBe(false)

  asked.length = 0
  await clock.advance(10_500)
  // 21 PRs make two queries: the first carries it.
  expect(asked.map(asksLatest)).toEqual([true, false])
  const checkedAt = JSON.parse(files.get(RESULTS)?.text ?? '{}').latest?.checkedAt
  expect(checkedAt).toBeGreaterThan(0)

  asked.length = 0
  await clock.advance(60_000)
  expect(asked.length).toBeGreaterThan(0)
  expect(asked.some(asksLatest)).toBe(false)

  ageCheck(files, clock.now())
  asked.length = 0
  await clock.advance(60_000)
  expect(asked.filter(asksLatest)).toHaveLength(1)
  expect(asksLatest(asked[0] ?? '')).toBe(true)
})

test('a newer pr-watch shows under the footer and toasts once per version', async ($, on) => {
  const toasts: string[] = []
  const gh = { ...GH, pw: onGitHub('0.9.0') }
  const { files, clock } = await newerSession($, on, gh, [FAILING], toasts)
  await clock.advance(10_500)

  expect(told(toasts)).toHaveLength(1)
  // Loaded from a folder (the test's), so the way to update is a git pull there.
  expect(told(toasts)[0]).toMatch(/^↑ pr-watch 0\.9\.0 available · git pull in \S/)
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await ui.find({ text: /↑ pr-watch 0\.9\.0 available/ })).toBeDefined()
  expect(await ui.find({ text: /git pull in / })).toBeDefined()
  expect(await ui.find({ text: /CI FAILED/ })).toBeDefined()
  await ui.unmount()

  // Round after round, and for every other session (notice.json keeps the one toasted): once.
  await clock.advance(60_000)
  expect(told(toasts)).toHaveLength(1)
  expect(JSON.parse(files.get(NOTICE)?.text ?? '{}')).toMatchObject({ version: '0.9.0' })

  // A later version toasts again, once.
  gh.pw = onGitHub('0.9.1')
  ageCheck(files, clock.now())
  await clock.advance(60_000)
  expect(told(toasts).slice(1)).toHaveLength(1)
  expect(told(toasts)[1]).toMatch(/^↑ pr-watch 0\.9\.1 available/)
})

test('a version another session already toasted shows in the footer, with no toast', async ($, on) => {
  const toasts: string[] = []
  const files = new Map([[NOTICE, { text: JSON.stringify({ version: '0.9.0' }), mtimeMs: 0 }]])
  const { clock } = await newerSession($, on, { ...GH, pw: onGitHub('0.9.0') }, [FAILING], toasts, files)
  await clock.advance(10_500)

  expect(told(toasts)).toEqual([])
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await ui.find({ text: /↑ pr-watch 0\.9\.0 available/ })).toBeDefined()
  await ui.unmount()
})

test("a check GitHub can't answer, or the same version, says nothing and leaves the cards be", async ($, on) => {
  const toasts: string[] = []
  // No pw answer: the repo can't be read.
  const gh: Record<string, unknown> = { ...GH }
  const { files, clock } = await newerSession($, on, gh, [FAILING], toasts)
  await clock.advance(10_500)

  const latest = JSON.parse(files.get(RESULTS)?.text ?? '{}').latest
  expect(latest.version).toBe(null)
  expect(latest.checkedAt).toBeGreaterThan(0)
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await ui.find({ text: /CI FAILED/ })).toBeDefined()
  expect(await ui.find({ text: /Failing: build/ })).toBeDefined()
  expect(await ui.find({ text: /available/ })).toBeUndefined()
  await ui.unmount()

  gh.pw = onGitHub('0.8.0')
  ageCheck(files, clock.now())
  await clock.advance(60_000)
  expect(JSON.parse(files.get(RESULTS)?.text ?? '{}').latest.version).toBe('0.8.0')
  const after = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await after.find({ text: /available/ })).toBeUndefined()
  await after.unmount()
  expect(told(toasts)).toEqual([])
})

test('with the notice off, no round asks and nothing is said', { options: { update_notice: false } }, async ($, on) => {
  const toasts: string[] = []
  // A newer version is already known from a round another session ran.
  const files = new Map([[RESULTS, { text: JSON.stringify({ fetchedAt: 0, nextAt: 0, prs: {}, accounts: {}, rate: null, latest: { version: '0.9.0', checkedAt: 0 } }), mtimeMs: 0 }]])
  const { asked, clock } = await newerSession($, on, { ...GH, pw: onGitHub('0.9.0') }, [FAILING], toasts, files)
  await clock.advance(60_000)

  expect(asked.length).toBeGreaterThan(0)
  expect(asked.some(asksLatest)).toBe(false)
  expect(told(toasts)).toEqual([])
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await ui.find({ text: /available/ })).toBeUndefined()
  await ui.unmount()
})

test('turning the notice off in /config takes the note away at once, and stops the asking', async ($, on) => {
  on('config.set', ($, e) => ({ value: e.value }))
  const toasts: string[] = []
  const { asked, files, clock } = await newerSession($, on, { ...GH, pw: onGitHub('0.9.0') }, [FAILING], toasts)
  await clock.advance(10_500)
  expect(told(toasts)).toHaveLength(1)

  await $.config.set({ key: 'pr-watch.update_notice', value: false } as never)
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await ui.find({ text: /available/ })).toBeUndefined()
  await ui.unmount()

  ageCheck(files, clock.now())
  asked.length = 0
  await clock.advance(60_000)
  expect(asked.length).toBeGreaterThan(0)
  expect(asked.some(asksLatest)).toBe(false)
})

const READY = 'https://github.com/acme/app/pull/21'
const repo = (extra: Record<string, unknown> = {}) => ({
  squashMergeAllowed: true,
  rebaseMergeAllowed: true,
  mergeCommitAllowed: false,
  viewerDefaultMergeMethod: 'SQUASH',
  viewerPermission: 'WRITE',
  ...extra,
})
const mergeable = (extra: Record<string, unknown> = {}) =>
  pr(READY, {
    reviewDecision: 'APPROVED',
    mergeStateStatus: 'CLEAN',
    mergeable: 'MERGEABLE',
    viewerCanEnableAutoMerge: true,
    viewerCanDisableAutoMerge: true,
    statusCheckRollup: [check('build', 'COMPLETED', 'SUCCESS')],
    repository: repo(),
    ...extra,
  })

// Watches #21 as `fixture` and draws the pane; `merges` is every `gh pr merge` argv since.
async function mergeCard(
  $: Parameters<TestBody>[0],
  on: On,
  fixture: Record<string, unknown>,
  merge?: (argv: readonly string[]) => MergeReply | Promise<MergeReply>,
) {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  quietUi(on)
  const { merges } = fakeHost(on, clock, { [READY]: fixture }, undefined, undefined, merge)
  await $.command.run({ command: 'pr-watch', args: READY, origin: { kind: 'composer' }, presentation: { isFullscreen: true, columns: 180 } })
  await clock.advance(100)

  return { ui: await $.ui.mount({ ...PANE, surface: 'terminal' }), merges, clock }
}

// A gh reply that waits for `release()`, so a test can act while gh runs.
function held<T>(reply: T) {
  let release = () => {}
  const promise = new Promise<T>(resolve => (release = () => resolve(reply)))

  return { promise, release: () => release() }
}

test('a clean PR offers Squash & merge, asks first, and Confirm merges it', async ($, on) => {
  const { ui, merges } = await mergeCard($, on, mergeable())
  expect((await ui.find({ key: `merge:${READY}` }))?.props.label).toBe('Squash & merge')

  await ui.press({ key: `merge:${READY}` })
  expect(await ui.find({ text: 'Squash-merge #21 into main?' })).toBeDefined()
  expect(merges).toEqual([])
  await ui.press({ key: `confirm:${READY}` })

  expect(merges).toEqual([['gh', 'pr', 'merge', READY, '--squash']])
  expect(await ui.find({ text: /✓ MERGED/ })).toBeDefined()
  expect(await ui.find({ key: `merge:${READY}` })).toBeUndefined()
  await ui.unmount()
})

test('Cancel at the question runs nothing and puts the button back', async ($, on) => {
  const { ui, merges } = await mergeCard($, on, mergeable())
  await ui.press({ key: `merge:${READY}` })
  await ui.press({ key: `cancel:${READY}` })

  expect(merges).toEqual([])
  expect(await ui.find({ text: /Squash-merge #21/ })).toBeUndefined()
  expect(await ui.find({ key: `merge:${READY}` })).toBeDefined()
  await ui.unmount()
})

test('the method button switches what the merge uses', async ($, on) => {
  const { ui, merges } = await mergeCard($, on, mergeable())
  await ui.press({ key: `method:${READY}` })
  expect((await ui.find({ key: `merge:${READY}` }))?.props.label).toBe('Rebase & merge')
  await ui.press({ key: `merge:${READY}` })
  await ui.press({ key: `confirm:${READY}` })

  expect(merges).toEqual([['gh', 'pr', 'merge', READY, '--rebase']])
  await ui.unmount()
})

test('a PR blocked on running checks offers auto-merge, which asks and then runs --auto', async ($, on) => {
  const blocked = mergeable({ mergeStateStatus: 'BLOCKED', statusCheckRollup: [check('build', 'IN_PROGRESS')] })
  const { ui, merges } = await mergeCard($, on, blocked)
  expect(await ui.find({ key: `merge:${READY}` })).toBeUndefined()

  await ui.press({ key: `auto:${READY}` })
  expect(await ui.find({ text: /Squash-merge #21 into main once/ })).toBeDefined()
  await ui.press({ key: `confirm:${READY}` })

  expect(merges).toEqual([['gh', 'pr', 'merge', READY, '--auto', '--squash']])
  expect(await ui.find({ text: '⇢ Auto-merge on · squash' })).toBeDefined()
  await ui.unmount()
})

test('auto-merge already on says so, and Cancel auto-merge turns it off at once', async ($, on) => {
  const auto = mergeable({ mergeStateStatus: 'BLOCKED', autoMergeRequest: { enabledAt: '2026-10-07T11:00:00Z', mergeMethod: 'SQUASH' } })
  const { ui, merges } = await mergeCard($, on, auto)
  expect(await ui.find({ text: '⇢ Auto-merge on · squash' })).toBeDefined()

  await ui.press({ key: `cancel-auto:${READY}` })

  expect(merges).toEqual([['gh', 'pr', 'merge', READY, '--disable-auto']])
  expect(await ui.find({ text: /Auto-merge on/ })).toBeUndefined()
  expect(await ui.find({ key: `auto:${READY}` })).toBeDefined()
  await ui.unmount()
})

test('a conflicting PR offers no merge', async ($, on) => {
  const { ui } = await mergeCard($, on, mergeable({ mergeStateStatus: 'DIRTY', mergeable: 'CONFLICTING' }))
  expect(await ui.find({ text: /CONFLICTS/ })).toBeDefined()
  expect(await ui.find({ key: `merge:${READY}` })).toBeUndefined()
  expect(await ui.find({ key: `auto:${READY}` })).toBeUndefined()
  await ui.unmount()
})

test('a draft offers no merge', async ($, on) => {
  const { ui } = await mergeCard($, on, mergeable({ isDraft: true, mergeStateStatus: 'DRAFT' }))
  expect(await ui.find({ text: /DRAFT/ })).toBeDefined()
  expect(await ui.find({ key: `merge:${READY}` })).toBeUndefined()
  expect(await ui.find({ key: `auto:${READY}` })).toBeUndefined()
  await ui.unmount()
})

test("a method GitHub refuses shows its message and isn't offered again", async ($, on) => {
  const refuse = (argv: readonly string[]) =>
    argv.includes('--squash')
      ? { exitCode: 1, stderr: 'GraphQL: Squash merges are not allowed on this repository. (mergePullRequest)\n' }
      : { exitCode: 0 }
  const { ui, merges } = await mergeCard($, on, mergeable(), refuse)
  await ui.press({ key: `merge:${READY}` })
  await ui.press({ key: `confirm:${READY}` })

  expect(merges).toEqual([['gh', 'pr', 'merge', READY, '--squash']])
  expect(await ui.find({ text: '✗ Squash merges are not allowed on this repository.' })).toBeDefined()
  expect((await ui.find({ key: `merge:${READY}` }))?.props.label).toBe('Rebase & merge')
  // Rebase is all that is left, so there is nothing to switch to.
  expect(await ui.find({ key: `method:${READY}` })).toBeUndefined()
  await ui.unmount()
})

test('a double Confirm runs one merge, and its success stands', async ($, on) => {
  const gh = held<MergeReply>({ exitCode: 0 })
  const { ui, merges, clock } = await mergeCard($, on, mergeable(), () => gh.promise)
  await ui.press({ key: `merge:${READY}` })
  // `y` twice before the pane redraws: both presses reach the same Confirm.
  const first = ui.press({ key: `confirm:${READY}` })
  const second = ui.press({ key: `confirm:${READY}` })
  await clock.advance(10)
  expect(await ui.find({ text: '◌ Merging…' })).toBeDefined()
  gh.release()
  await Promise.all([first, second])

  expect(merges).toEqual([['gh', 'pr', 'merge', READY, '--squash']])
  expect(await ui.find({ text: /✓ MERGED/ })).toBeDefined()
  expect(await ui.find({ text: /^✗/ })).toBeUndefined()
  await ui.unmount()
})

test('a double press of Cancel auto-merge runs it once', async ($, on) => {
  const auto = mergeable({ mergeStateStatus: 'BLOCKED', autoMergeRequest: { enabledAt: '2026-10-07T11:00:00Z', mergeMethod: 'SQUASH' } })
  const gh = held<MergeReply>({ exitCode: 0 })
  const { ui, merges, clock } = await mergeCard($, on, auto, () => gh.promise)
  const first = ui.press({ key: `cancel-auto:${READY}` })
  const second = ui.press({ key: `cancel-auto:${READY}` })
  await clock.advance(10)
  expect(await ui.find({ text: '◌ Cancelling auto-merge…' })).toBeDefined()
  gh.release()
  await Promise.all([first, second])

  expect(merges).toEqual([['gh', 'pr', 'merge', READY, '--disable-auto']])
  await ui.unmount()
})

test('a merge left busy (a reload mid-run, a gh that never ends) gives its buttons back once stale', async ($, on) => {
  // The first run never comes back, as one a reload cut off never does.
  const hung = held<MergeReply>({ exitCode: 0 })
  let calls = 0
  const { ui, merges, clock } = await mergeCard($, on, mergeable(), () => ((calls += 1), calls === 1 ? hung.promise : { exitCode: 0 }))
  await ui.press({ key: `merge:${READY}` })
  void ui.press({ key: `confirm:${READY}` })
  await clock.advance(10)
  expect(await ui.find({ text: '◌ Merging…' })).toBeDefined()
  expect(await ui.find({ key: `merge:${READY}` })).toBeUndefined()

  await clock.advance(121_000)
  await ui.press({ key: 'refresh' })
  expect(await ui.find({ text: /Merging…/ })).toBeUndefined()
  await clock.advance(10)
  await ui.press({ key: `merge:${READY}` })
  await ui.press({ key: `confirm:${READY}` })
  expect(merges).toHaveLength(2)
  expect(await ui.find({ text: /✓ MERGED/ })).toBeDefined()
  hung.release()
  await ui.unmount()
})

test("an error that only mentions a merge method shows, and hides no method", async ($, on) => {
  const fail = () => ({ exitCode: 1, stderr: 'GraphQL: Pull request is in clean status, but the merge method squash could not be applied (mergePullRequest)\n' })
  const { ui } = await mergeCard($, on, mergeable(), fail)
  await ui.press({ key: `merge:${READY}` })
  await ui.press({ key: `confirm:${READY}` })

  expect(await ui.find({ text: '✗ Pull request is in clean status, but the merge method squash could not be applied' })).toBeDefined()
  expect((await ui.find({ key: `merge:${READY}` }))?.props.label).toBe('Squash & merge')
  expect((await ui.find({ key: `method:${READY}` }))?.props.label).toBe('⇄ rebase')
  await ui.unmount()
})

test('once every method is refused, the card keeps the error and offers no button', async ($, on) => {
  const refuse = () => ({ exitCode: 1, stderr: 'GraphQL: Squash merges are not allowed on this repository. (mergePullRequest)\n' })
  const { ui } = await mergeCard($, on, mergeable({ repository: repo({ rebaseMergeAllowed: false }) }), refuse)
  await ui.press({ key: `merge:${READY}` })
  await ui.press({ key: `confirm:${READY}` })

  expect(await ui.find({ text: '✗ Squash merges are not allowed on this repository.' })).toBeDefined()
  expect(await ui.find({ key: `merge:${READY}` })).toBeUndefined()
  await ui.unmount()
})

test("a round an older version of the mod ran keeps the card's merge row", async ($, on) => {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  quietUi(on)
  const files = new Map<string, { text: string; mtimeMs: number }>()
  fakeHost(on, clock, { [READY]: mergeable() }, files)
  on('command.register', () => ({ value: { command: 'pr-watch' } }))
  on('tool.register', () => ({ value: { tool: 'mcp__pr-watch__watch' } }))
  on('session.start', () => ({ cwd: '/work' }))
  await $.session.start({ source: 'startup', cwd: '/work' } as never)
  await $.command.run({ command: 'pr-watch', args: READY, origin: { kind: 'composer' }, presentation: { isFullscreen: true, columns: 180 } })
  await clock.advance(100)
  // A session on a version from before the merge button won the lease: its entry has no
  // merge facts (and a newer title, so the round is seen to land).
  const fresh: Record<string, unknown> = mergeable({ title: 'Retry uploads, take two' })
  const { repository: _r, viewerCanEnableAutoMerge: _e, viewerCanDisableAutoMerge: _d, ...older } = fresh
  const at = clock.now()
  files.set('/home/me/.cache/pr-watch/results.json', {
    text: JSON.stringify({ fetchedAt: at, nextAt: at + 60_000, prs: { [READY]: { at, pr: older } }, accounts: {}, rate: null }),
    mtimeMs: at,
  })
  await clock.advance(10_500)

  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await ui.find({ text: /Retry uploads, take two/ })).toBeDefined()
  expect((await ui.find({ key: `merge:${READY}` }))?.props.label).toBe('Squash & merge')
  await ui.unmount()
})

const BROKEN = 'https://github.com/acme/app/pull/31'
// A GitHub Actions job (its databaseId is the job's id) and another CI's commit status.
const actionsJob = (name: string, id: number, conclusion: string) => ({
  ...check(name, 'COMPLETED', conclusion),
  startedAt: '2026-10-07T11:50:00Z',
  databaseId: id,
  detailsUrl: `https://github.com/acme/app/actions/runs/5/job/${id}`,
  checkSuite: { workflowRun: { workflow: { name: 'CI' } } },
})
const jenkins = { __typename: 'StatusContext', context: 'ci/jenkins', state: 'FAILURE', description: 'tests failed', targetUrl: 'https://ci.example.com/job/7' }
const broken = pr(BROKEN, { statusCheckRollup: [actionsJob('build', 901, 'FAILURE'), actionsJob('lint', 902, 'SUCCESS'), jenkins] })

type Submitted = { text: string } | { drop: string }

// Watches `fixtures` and draws the pane, with a prompt box that takes drafts unless `fill`
// says otherwise, and a session that takes prompts unless `submit` drops them; `fills` and
// `sent` are what reached the box and the session, `box` what the box holds, and `type` puts
// the person's own text in it. `asked` and `gate` are the fake host's (fakeHost).
async function fixCard(
  $: Parameters<TestBody>[0],
  on: On,
  fixtures: Record<string, unknown>,
  runLog?: (argv: readonly string[]) => RunReply | Promise<RunReply>,
  fill: () => { isFilled: boolean } = () => ({ isFilled: true }),
  submit: (text: string) => Submitted = text => ({ text }),
) {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  quietUi(on)
  const fills: string[] = []
  const sent: string[] = []
  let box = ''
  const { runs, asked, gate } = fakeHost(on, clock, fixtures, undefined, undefined, undefined, runLog)
  on('prompt.read', () => ({ value: { text: box, cursor: box.length } }))
  on('prompt.fill', ($, e) => {
    fills.push(e.text)
    const reply = fill()
    if (reply.isFilled) box = e.mode === 'append' ? `${box}${e.text}` : e.text

    return reply
  })
  on('prompt.submit', ($, e) => (sent.push(e.text), submit(e.text)))
  await $.command.run({
    command: 'pr-watch',
    args: Object.keys(fixtures).join(' '),
    origin: { kind: 'composer' },
    presentation: { isFullscreen: true, columns: 180 },
  })
  await clock.advance(100)

  return {
    ui: await $.ui.mount({ ...PANE, surface: 'terminal' }),
    clock,
    runs,
    asked,
    gate,
    fills,
    sent,
    box: () => box,
    type: (text: string) => (box = text),
  }
}

test('a card whose CI failed offers Fix with Claude; a running or green one does not', async ($, on) => {
  const { ui } = await fixCard($, on, { [BROKEN]: broken, [RUNNING]: GH[RUNNING], [READY]: mergeable() })
  expect((await ui.find({ key: `fix:${BROKEN}` }))?.props.label).toBe('Fix with Claude')
  expect(await ui.find({ key: `fix:${RUNNING}` })).toBeUndefined()
  expect(await ui.find({ key: `fix:${READY}` })).toBeUndefined()
  await ui.unmount()
})

test("Fix with Claude fetches the failed job's log and drafts the hand-off in the prompt box", async ($, on) => {
  const { ui, runs, fills, sent } = await fixCard($, on, { [BROKEN]: broken })
  await ui.press({ key: `fix:${BROKEN}` })

  expect(runs).toEqual([['gh', 'run', 'view', '--job', '901', '--log-failed', '-R', 'acme/app']])
  expect(fills).toHaveLength(1)
  const draft = fills[0] ?? ''
  expect(draft).toContain('CI failed on acme/app#31')
  expect(draft).toContain('- CI / build · failure · https://github.com/acme/app/actions/runs/5/job/901')
  expect(draft).toContain('- ci/jenkins · failure · tests failed · https://ci.example.com/job/7')
  expect(draft).toContain('Error: expected 3 retries, got 1')
  expect(draft).not.toContain('lint')
  // A draft, not a message: nothing reaches Claude until the person presses Enter.
  expect(sent).toEqual([])
  expect(await ui.find({ text: /Drafted in the prompt box/ })).toBeDefined()
  await ui.unmount()
})

test('while the logs come, the card says so', async ($, on) => {
  let release = () => {}
  const held = new Promise<RunReply>(resolve => (release = () => resolve({ exitCode: 0, stdout: FAILED_LOG })))
  const { ui, clock } = await fixCard($, on, { [BROKEN]: broken }, () => held)
  const pressed = ui.press({ key: `fix:${BROKEN}` })
  await clock.advance(10)

  expect(await ui.find({ text: '◌ Collecting logs…' })).toBeDefined()
  release()
  await pressed
  expect(await ui.find({ text: /Collecting logs/ })).toBeUndefined()
  await ui.unmount()
})

test('a log GitHub no longer keeps is said on the card, and the names and links still go', async ($, on) => {
  const gone = () => ({ exitCode: 1, stderr: 'failed to get run log: HTTP 410: Server Error (https://api.github.com/repos/acme/app/actions/runs/5/logs)\n' })
  const { ui, fills } = await fixCard($, on, { [BROKEN]: broken }, gone)
  await ui.press({ key: `fix:${BROKEN}` })

  expect(await ui.find({ text: '✗ No log for build: GitHub no longer keeps this log' })).toBeDefined()
  const draft = fills[0] ?? ''
  expect(draft).toContain('- CI / build · failure · https://github.com/acme/app/actions/runs/5/job/901')
  expect(draft).toContain('build: no log (GitHub no longer keeps this log).')
  expect(draft).toContain('https://ci.example.com/job/7')
  await ui.unmount()
})

test("where the prompt box won't take a draft, it asks first; Cancel sends nothing, Confirm sends it", async ($, on) => {
  const { ui, fills, sent } = await fixCard($, on, { [BROKEN]: broken }, undefined, () => ({ isFilled: false }))
  await ui.press({ key: `fix:${BROKEN}` })
  expect(fills).toHaveLength(1)
  expect(await ui.find({ text: 'Send CI failure of #31 to Claude?' })).toBeDefined()

  await ui.press({ key: `fix-cancel:${BROKEN}` })
  expect(sent).toEqual([])
  expect(await ui.find({ text: /Send CI failure/ })).toBeUndefined()

  await ui.press({ key: `fix:${BROKEN}` })
  await ui.press({ key: `fix-confirm:${BROKEN}` })
  expect(sent).toHaveLength(1)
  expect(sent[0]).toContain('CI failed on acme/app#31')
  expect(await ui.find({ text: '✓ Sent to Claude' })).toBeDefined()
  await ui.unmount()
})

test('a double press of Fix with Claude collects and drafts once', async ($, on) => {
  const gh = held<RunReply>({ exitCode: 0, stdout: FAILED_LOG })
  const { ui, clock, runs, fills } = await fixCard($, on, { [BROKEN]: broken }, () => gh.promise)
  const first = ui.press({ key: `fix:${BROKEN}` })
  const second = ui.press({ key: `fix:${BROKEN}` })
  await clock.advance(10)
  gh.release()
  await Promise.all([first, second])

  expect(runs).toHaveLength(1)
  expect(fills).toHaveLength(1)
  await ui.unmount()
})

test('a collection left busy gives the button back once stale', async ($, on) => {
  const hung = held<RunReply>({ exitCode: 0, stdout: FAILED_LOG })
  let calls = 0
  const { ui, clock, fills } = await fixCard($, on, { [BROKEN]: broken }, () => ((calls += 1), calls === 1 ? hung.promise : { exitCode: 0, stdout: FAILED_LOG }))
  void ui.press({ key: `fix:${BROKEN}` })
  await clock.advance(10)
  expect(await ui.find({ text: '◌ Collecting logs…' })).toBeDefined()

  await clock.advance(121_000)
  await ui.press({ key: 'refresh' })
  expect(await ui.find({ text: /Collecting logs/ })).toBeUndefined()
  await ui.press({ key: `fix:${BROKEN}` })
  expect(fills).toHaveLength(1)
  // The cut-off run, back at last, drafts nothing over the later press's.
  hung.release()
  await clock.advance(10)
  expect(fills).toHaveLength(1)
  await ui.unmount()
})

test('a stale busy mark goes with the rest of the row when the card moves', async ($, on) => {
  const hung = held<RunReply>({ exitCode: 0, stdout: FAILED_LOG })
  const fixtures: Record<string, unknown> = { [BROKEN]: broken }
  const { ui, clock, fills } = await fixCard($, on, fixtures, () => hung.promise)
  void ui.press({ key: `fix:${BROKEN}` })
  await clock.advance(121_000)
  // The failing job is re-run and passes: the card moves, and what the row held goes.
  fixtures[BROKEN] = pr(BROKEN, { statusCheckRollup: [actionsJob('build', 903, 'SUCCESS'), actionsJob('lint', 902, 'SUCCESS')] })
  await ui.press({ key: 'refresh' })
  hung.release()
  await clock.advance(10)

  expect(fills).toEqual([])
  await ui.unmount()
})

test("the draft carries a failed run's summary, asked of failed runs only", async ($, on) => {
  const summed = pr(BROKEN, {
    statusCheckRollup: [
      { ...actionsJob('build', 901, 'FAILURE'), title: '3 tests failed', summary: 'upload.test.ts: expected 3 retries' },
      { ...actionsJob('lint', 902, 'SUCCESS'), title: 'Lint passed', summary: 'x'.repeat(65_000) },
    ],
  })
  const { ui, fills } = await fixCard($, on, { [BROKEN]: summed })
  await ui.press({ key: `fix:${BROKEN}` })

  expect(fills[0]).toContain('- CI / build · failure · 3 tests failed · upload.test.ts: expected 3 retries · https://github.com/acme/app/actions/runs/5/job/901')
  expect(fills[0]).not.toContain('Lint passed')
  await ui.unmount()
})

test('a send a hook refuses takes back "Sent to Claude" and says why', async ($, on) => {
  const { ui, clock, sent } = await fixCard($, on, { [BROKEN]: broken }, undefined, () => ({ isFilled: false }), () => ({ drop: 'Blocked by a hook' }))
  await ui.press({ key: `fix:${BROKEN}` })
  await ui.press({ key: `fix-confirm:${BROKEN}` })
  await clock.advance(10)

  expect(sent.length).toBeGreaterThan(0)
  expect(await ui.find({ text: '✗ Not sent: Blocked by a hook' })).toBeDefined()
  expect(await ui.find({ text: /Sent to Claude/ })).toBeUndefined()
  await ui.unmount()
})

test('a card dismissed while its logs come drafts nothing', async ($, on) => {
  const gh = held<RunReply>({ exitCode: 0, stdout: FAILED_LOG })
  const { ui, clock, fills } = await fixCard($, on, { [BROKEN]: broken, [RUNNING]: GH[RUNNING] }, () => gh.promise)
  const pressed = ui.press({ key: `fix:${BROKEN}` })
  await clock.advance(10)
  await ui.press({ key: `dismiss:${BROKEN}` })
  gh.release()
  await pressed

  expect(fills).toEqual([])
  await ui.unmount()
})

test("a second press replaces its earlier draft and keeps the person's text before it", async ($, on) => {
  const { ui, fills, box, type } = await fixCard($, on, { [BROKEN]: broken })
  await ui.press({ key: `fix:${BROKEN}` })
  const draft = box()
  type(`Look at this one first.\n\n${draft}`)
  await ui.press({ key: `fix:${BROKEN}` })

  expect(fills).toHaveLength(2)
  expect(box().startsWith('Look at this one first.\n\nCI failed on acme/app#31')).toBe(true)
  expect(box().split('CI failed on acme/app#31')).toHaveLength(2)
  await ui.unmount()
})

const REVIEWED = 'https://github.com/acme/app/pull/41'
const unresolved = (isOutdated = false) => ({ isResolved: false, isOutdated })
// Two comment-only reviews (one by a review app), a review asked of carol, and threads: three
// open (one on code since changed), one resolved. GitHub still says REVIEW_REQUIRED.
const comments: Said[] = [
  { login: 'nadav', state: 'COMMENTED', at: '2026-10-07T10:00:00Z' },
  { login: 'codebot-review-assistant', state: 'COMMENTED', at: '2026-10-07T10:05:00Z', isBot: true },
]
const commented = pr(REVIEWED, {
  mergeStateStatus: 'BLOCKED',
  statusCheckRollup: [check('build', 'COMPLETED', 'SUCCESS')],
  reviews: comments,
  requested: ['carol'],
  threads: [unresolved(), unresolved(), unresolved(true), { isResolved: true, isOutdated: false }],
})

test('a card shows who reviewed and the threads still open, and the comments are the headline', async ($, on) => {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  quietUi(on)
  fakeHost(on, clock, { [REVIEWED]: commented })
  await $.command.run({ command: 'pr-watch', args: REVIEWED, origin: { kind: 'composer' }, presentation: { isFullscreen: true, columns: 180 } })
  await clock.advance(100)

  for (const surface of ['terminal', 'desktop'] as const) {
    const ui = await $.ui.mount({ ...PANE, surface, props: { ...PANE.props, bodyColumns: 50 } })
    expect(await ui.find({ text: /💬 COMMENTS/ })).toBeDefined()
    expect(await ui.find({ type: 'Text', text: '3 comments to address · 1 outdated' })).toBeDefined()
    expect(await ui.find({ text: /Waiting for review/ })).toBeUndefined()
    // Docked at 50 columns the verbs don't fit: icons and names.
    expect(await ui.find({ type: 'Text', text: '💬 nadav · 💬 codebot · ◷ carol' })).toBeDefined()
    // The headline counts the threads, so no second line says it again.
    expect(await ui.find({ text: /unresolved/ })).toBeUndefined()
    await ui.unmount()
  }
  const wide = await $.ui.mount({ ...PANE, surface: 'terminal', props: { ...PANE.props, bodyColumns: 80 } })
  expect(await wide.find({ type: 'Text', text: '💬 nadav commented · 💬 codebot commented · ◷ carol requested' })).toBeDefined()
  await wide.unmount()
})

test("a PR with no reviews has no reviewers line, and a merged one shows none", async ($, on) => {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  quietUi(on)
  fakeHost(on, clock, { [RUNNING]: GH[RUNNING], [MERGED]: { ...(GH[MERGED] as object), reviews: [{ login: 'alice', state: 'APPROVED', at: '2026-10-07T09:00:00Z' }] } })
  await $.command.run({ command: 'pr-watch', args: `${RUNNING} ${MERGED}`, origin: { kind: 'composer' }, presentation: { isFullscreen: true, columns: 180 } })
  await clock.advance(100)

  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await ui.find({ text: /CI RUNNING/ })).toBeDefined()
  expect(await ui.find({ text: /approved|requested|commented|unresolved/ })).toBeUndefined()
  await ui.unmount()
})

test("a review that lands between rounds toasts once; the ones there at first sight, and the person's own, don't", async ($, on) => {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  const toasts: string[] = []
  on('ui.open', () => ({ value: { isPlaced: true } }))
  on('ui.close', () => ({ value: undefined }))
  on('ui.toast', ($, e) => (toasts.push(e.text), { value: undefined }))
  on('ui.status', () => ({ value: undefined }))
  on('ui.blit', () => ({ value: {} }))
  const gh: Record<string, unknown> = { [REVIEWED]: commented }
  fakeHost(on, clock, gh)
  on('command.register', () => ({ value: { command: 'pr-watch' } }))
  on('tool.register', () => ({ value: { tool: 'mcp__pr-watch__watch' } }))
  on('session.start', () => ({ cwd: '/work' }))
  await $.session.start({ source: 'startup', cwd: '/work' } as never)
  await $.command.run({ command: 'pr-watch', args: REVIEWED, origin: { kind: 'composer' }, presentation: { isFullscreen: true, columns: 180 } })
  await clock.advance(10_500)
  expect(toasts).toEqual([])

  // The person replies (as `me`, gh's account here), then alice approves.
  gh[REVIEWED] = {
    ...commented,
    reviews: [...comments, { login: 'me', state: 'COMMENTED', at: '2026-10-07T12:00:20Z' }, { login: 'alice', state: 'APPROVED', at: '2026-10-07T12:00:30Z' }],
  }
  await clock.advance(70_000)
  expect(toasts).toEqual(['✓ alice approved #41'])

  await clock.advance(70_000)
  expect(toasts).toEqual(['✓ alice approved #41'])

  // A review that moves the card says so once: no second toast for the state it caused.
  const before = gh[REVIEWED] as Record<string, unknown>
  gh[REVIEWED] = {
    ...before,
    reviewDecision: 'CHANGES_REQUESTED',
    reviews: [...(before.reviews as Said[]), { login: 'bob', state: 'CHANGES_REQUESTED', at: '2026-10-07T12:02:00Z' }],
  }
  await clock.advance(70_000)
  expect(toasts).toEqual(['✓ alice approved #41', '✗ bob requested changes on #41'])
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await ui.find({ text: /CHANGES REQUESTED/ })).toBeDefined()
  expect(await ui.find({ type: 'Text', text: '3 unresolved threads (1 outdated)' })).toBeDefined()
  expect(await ui.find({ type: 'Text', text: '✗ bob · ✓ alice · 💬 nadav · 💬 codebot · +2 more' })).toBeDefined()
  await ui.unmount()

  // The author's own thread replies toast nothing; a merge in the same round as a review (an
  // approval auto-merged) still toasts.
  const last = gh[REVIEWED] as Record<string, unknown>
  gh[REVIEWED] = {
    ...last,
    state: 'MERGED',
    mergedAt: '2026-10-07T12:04:00Z',
    reviewDecision: 'APPROVED',
    reviews: [
      ...(last.reviews as Said[]),
      { login: 'octocat', state: 'COMMENTED', at: '2026-10-07T12:03:00Z' },
      { login: 'dana', state: 'APPROVED', at: '2026-10-07T12:03:30Z' },
    ],
  }
  await clock.advance(70_000)
  expect(toasts.slice(2)).toEqual(['✓ dana approved #41', '🎉 PR #41 · Merged'])
})

// A review thread as the press's query has it: where it is, and what each comment said.
const said = (path: string, line: number | null, comments: [string, string][], extra: Record<string, unknown> = {}) => ({
  isResolved: false,
  isOutdated: false,
  path,
  line,
  startLine: null,
  originalLine: line,
  originalStartLine: null,
  comments: {
    totalCount: comments.length,
    nodes: comments.map(([login, body], i) => ({ author: { login }, body, url: `${REVIEWED}#discussion_r${path.length}${line ?? 0}${i}` })),
  },
  ...extra,
})
// Threads with their words: two open (one outdated), one resolved; and #41 with them.
const discussion = [
  said('src/upload.ts', 42, [['nadav', 'Take the retry count from config.'], ['octocat', 'Will do.']], { startLine: 40 }),
  said('src/upload.ts', null, [['nadav', 'Is this branch still needed?']], { isOutdated: true, originalLine: 7 }),
  said('src/api.ts', 9, [['carol', 'Fixed already.']], { isResolved: true }),
]
const discussed = pr(REVIEWED, {
  mergeStateStatus: 'BLOCKED',
  statusCheckRollup: [check('build', 'COMPLETED', 'SUCCESS')],
  reviews: comments,
  threads: discussion,
})

test('a card with unresolved threads offers Address with Claude; a resolved or unreviewed one does not', async ($, on) => {
  const done = pr(READY, { statusCheckRollup: [check('build', 'COMPLETED', 'SUCCESS')], reviews: comments, threads: [{ isResolved: true, isOutdated: false }] })
  const { ui } = await fixCard($, on, { [REVIEWED]: discussed, [READY]: done, [BROKEN]: broken })
  expect((await ui.find({ key: `address:${REVIEWED}` }))?.props.label).toBe('Address with Claude')
  expect(await ui.find({ key: `fix:${REVIEWED}` })).toBeUndefined()
  expect(await ui.find({ key: `address:${READY}` })).toBeUndefined()
  expect(await ui.find({ key: `address:${BROKEN}` })).toBeUndefined()
  expect((await ui.find({ key: `fix:${BROKEN}` }))?.props.label).toBe('Fix with Claude')
  await ui.unmount()
})

test("Address with Claude reads that PR's threads on the press and drafts the open ones in the prompt box", async ($, on) => {
  const { ui, asked, fills, sent } = await fixCard($, on, { [REVIEWED]: discussed, [RUNNING]: GH[RUNNING] })
  // The poll asks whether threads are resolved, never what they say.
  expect(asked.length).toBeGreaterThan(0)
  expect(asked.some(query => query.includes('body'))).toBe(false)
  const before = asked.length
  await ui.press({ key: `address:${REVIEWED}` })

  const press = asked.slice(before)
  expect(press).toHaveLength(1)
  expect(press[0]).toContain('pullRequest(number: 41)')
  expect(press[0]).toContain('comments(first: 10) { totalCount nodes { author { login } body url } }')
  expect(press[0]?.match(/repository\(/g)).toHaveLength(1)
  expect(fills).toHaveLength(1)
  const draft = fills[0] ?? ''
  expect(draft).toContain('Review comments to address on acme/app#41: Retry uploads with exponential backoff')
  expect(draft).toContain('2 unresolved review threads, by file (1 outdated')
  expect(draft).toMatch(/1\. src\/upload\.ts \(outdated; was line 7\) · https:\/\/github\.com\/acme\/app\/pull\/41#discussion_r\d+\n   @nadav: Is this branch still needed\?/)
  expect(draft).toContain('src/upload.ts:40-42')
  expect(draft).toContain('   @nadav: Take the retry count from config.\n   @octocat (PR author): Will do.')
  expect(draft).not.toContain('Fixed already')
  expect(draft).toContain("Don't reply to or resolve any thread on GitHub")
  // A draft, not a message: nothing reaches Claude until the person presses Enter.
  expect(sent).toEqual([])
  expect(await ui.find({ text: /Drafted in the prompt box/ })).toBeDefined()
  await ui.unmount()
})

test('while the threads come the card says so, and a second press reads and drafts nothing more', async ($, on) => {
  const { ui, clock, asked, gate, fills } = await fixCard($, on, { [REVIEWED]: discussed })
  const gh = held(undefined)
  gate.hold = gh.promise
  const first = ui.press({ key: `address:${REVIEWED}` })
  const second = ui.press({ key: `address:${REVIEWED}` })
  await clock.advance(10)
  expect(await ui.find({ text: '◌ Reading review threads…' })).toBeDefined()
  gh.release()
  await Promise.all([first, second])

  expect(asked.filter(query => query.includes('body'))).toHaveLength(1)
  expect(fills).toHaveLength(1)
  expect(await ui.find({ text: /Reading review threads/ })).toBeUndefined()
  await ui.unmount()
})

test('a read left busy gives the button back once stale, and the cut-off one drafts nothing', async ($, on) => {
  const { ui, clock, gate, fills } = await fixCard($, on, { [REVIEWED]: discussed })
  const hung = held(undefined)
  gate.hold = hung.promise
  void ui.press({ key: `address:${REVIEWED}` })
  await clock.advance(10)
  expect(await ui.find({ text: '◌ Reading review threads…' })).toBeDefined()

  await clock.advance(121_000)
  gate.hold = null
  await ui.press({ key: 'refresh' })
  expect(await ui.find({ text: /Reading review threads/ })).toBeUndefined()
  await ui.press({ key: `address:${REVIEWED}` })
  expect(fills).toHaveLength(1)
  hung.release()
  await clock.advance(10)
  expect(fills).toHaveLength(1)
  await ui.unmount()
})

test('threads resolved since the last round: the card says so and drafts nothing', async ($, on) => {
  const fixtures: Record<string, unknown> = { [REVIEWED]: discussed }
  const { ui, fills } = await fixCard($, on, fixtures)
  fixtures[REVIEWED] = { ...discussed, threads: [said('src/upload.ts', 42, [['nadav', 'x']], { isResolved: true })] }
  await ui.press({ key: `address:${REVIEWED}` })

  expect(fills).toEqual([])
  expect(await ui.find({ text: '✗ No review thread is unresolved now' })).toBeUndefined()
  // The press asked GitHub again, so the card has caught up: no threads, no button.
  expect(await ui.find({ key: `address:${REVIEWED}` })).toBeUndefined()
  await ui.unmount()
})

test("where the prompt box won't take a draft, it asks first, and Confirm sends the review comments", async ($, on) => {
  const { ui, sent } = await fixCard($, on, { [REVIEWED]: discussed }, undefined, () => ({ isFilled: false }))
  await ui.press({ key: `address:${REVIEWED}` })
  expect(await ui.find({ text: 'Send review comments of #41 to Claude?' })).toBeDefined()

  await ui.press({ key: `address-confirm:${REVIEWED}` })
  expect(sent).toHaveLength(1)
  expect(sent[0]).toContain('Review comments to address on acme/app#41')
  expect(await ui.find({ text: '✓ Sent to Claude' })).toBeDefined()
  await ui.unmount()
})

test('a card with failed CI and open threads offers both, and each press keeps the other draft', async ($, on) => {
  const both = { ...broken, reviews: comments, threads: discussion }
  const { ui, box } = await fixCard($, on, { [BROKEN]: both })
  expect(await ui.find({ key: `fix:${BROKEN}` })).toBeDefined()
  expect(await ui.find({ key: `address:${BROKEN}` })).toBeDefined()

  await ui.press({ key: `fix:${BROKEN}` })
  await ui.press({ key: `address:${BROKEN}` })
  await ui.press({ key: `fix:${BROKEN}` })
  expect(box().startsWith('CI failed on acme/app#31')).toBe(true)
  expect(box().split('CI failed on acme/app#31')).toHaveLength(2)
  expect(box()).toContain('\n\nReview comments to address on acme/app#31')
  await ui.unmount()
})

// ---- GitHub having a moment -------------------------------------------------------------

const NOON = Date.parse('2026-10-07T12:00:00Z')
const BAD_GATEWAY = { stderr: 'gh: HTTP 502' }

// A session watching #13 (CI failed, nothing moving), its watch fetched at noon, where the
// first `blips` GraphQL queries fail. Rounds then run a minute apart: 12:00:10, 12:01:10, …
// `setup` sees the host before the session starts (its gh logins and their budgets).
async function blipSession(
  $: Parameters<TestBody>[0],
  on: On,
  blips: Blip[] = [],
  files = new Map<string, { text: string; mtimeMs: number }>(),
  { fixtures = GH, setup = () => undefined }: { fixtures?: Record<string, unknown>; setup?: (host: ReturnType<typeof fakeHost>) => void } = {},
) {
  const clock = mock.clock(on, { now: NOON })
  quietUi(on)
  const host = fakeHost(on, clock, fixtures, files)
  host.blips.push(...blips)
  setup(host)
  on('command.register', () => ({ value: { command: 'pr-watch' } }))
  on('tool.register', () => ({ value: { tool: 'mcp__pr-watch__watch' } }))
  on('session.start', () => ({ cwd: '/work' }))
  await $.session.start({ source: 'startup', cwd: '/work' } as never)
  await $.command.run({ command: 'pr-watch', args: FAILING, origin: { kind: 'composer' }, presentation: { isFullscreen: true, columns: 180 } })
  await clock.advance(100)

  return { ...host, clock, entry: () => JSON.parse(files.get(RESULTS)?.text ?? '{}').prs?.[FAILING] }
}

// What #13's card says: its error line, if any, and how old its data is.
async function blipCard($: Parameters<TestBody>[0]) {
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  // The innermost Text that says it (each one around it says it too).
  const line = async (text: RegExp) => (await ui.findAll({ type: 'Text', text })).map(t => t.text).sort((a, b) => a.length - b.length)[0] ?? null
  const said = { warning: await line(/⚠ /), checked: await line(/· checked /), isFailedShown: (await ui.find({ text: /CI FAILED/ })) !== undefined }
  await ui.unmount()

  return said
}

// The pane's gh line, a string a line, and the colour of each part that has one (the rest
// are dim, `isDim` says).
type Node = { type: string; props?: Record<string, unknown>; children?: unknown[] }
const flat = (node: unknown): string => (typeof node === 'string' ? node : ((node as Node).children ?? []).map(flat).join(''))

async function ghFooter($: Parameters<TestBody>[0]) {
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  const rows = (((await ui.find({ key: 'gh' })) as unknown as Node | undefined)?.children ?? []) as Node[]
  await ui.unmount()
  const parts = rows.flatMap(row => (row.children ?? []) as Node[])
  const lines = rows.map(flat)
  const tones = Object.fromEntries(parts.filter(part => part.props?.color !== undefined).map(part => [flat(part), part.props?.color]))
  const isDim = parts.every(part => part.props?.color !== undefined || part.props?.dimColor === true)

  return { lines, tones, isDim }
}

test("a 502 a retry gets past leaves no trace: one retry, two seconds on", async ($, on) => {
  const { clock, queries, blips, entry } = await blipSession($, on)
  queries.length = 0
  blips.push(BAD_GATEWAY)
  // The round at 12:00:10 (the lease's 150 ms, then the 502): the retry waits its two seconds.
  await clock.advance(9_900 + 150 + 1_900)
  expect(queries).toEqual([[FAILING]])
  await clock.advance(200)
  expect(queries).toEqual([[FAILING], [FAILING]])
  expect(entry()).toMatchObject({ at: NOON + 10_000 })
  expect(entry().error).toBe(undefined)
  expect(entry().failedSince).toBe(undefined)
  expect(await blipCard($)).toMatchObject({ warning: null, isFailedShown: true })
})

test('a refusal or spent points are not asked again, and spent points wait for the reset', async ($, on) => {
  const { clock, queries, blips, files } = await blipSession($, on)
  queries.length = 0
  const reset = Date.parse('2026-10-07T13:00:00Z')
  blips.push(
    { stderr: 'gh: Bad credentials (HTTP 401)', body: '{"message":"Bad credentials"}' },
    {
      stderr: 'gh: API rate limit already exceeded for user ID 1.',
      status: 200,
      headers: ['X-Ratelimit-Remaining: 0', `X-Ratelimit-Reset: ${reset / 1_000}`, 'X-Ratelimit-Resource: graphql'],
      body: '{"data":null,"errors":[{"type":"RATE_LIMITED","message":"API rate limit already exceeded for user ID 1."}]}',
    },
  )
  await clock.advance(13_000)
  expect(queries).toEqual([[FAILING]])
  await clock.advance(60_000)
  expect(queries).toEqual([[FAILING], [FAILING]])
  expect(JSON.parse(files.get(RESULTS)?.text ?? '{}').nextAt).toBe(reset)
})

const ERRORS_LOG = '/home/me/.cache/pr-watch/errors.log'
const SECONDARY_LIMIT = {
  stderr: 'gh: You have exceeded a secondary rate limit. Please wait a few minutes before you try again. (HTTP 403)',
  status: 403,
  headers: ['Retry-After: 120', 'X-Ratelimit-Remaining: 4990', 'X-Ratelimit-Used: 10', 'X-Ratelimit-Resource: graphql'],
  body: '{"message":"You have exceeded a secondary rate limit. Please wait a few minutes before you try again."}',
}

test("a secondary limit is not retried, and holds every session off GitHub for its retry-after", async ($, on) => {
  const { clock, queries, blips, files, entry } = await blipSession($, on)
  queries.length = 0
  blips.push(SECONDARY_LIMIT)
  await clock.advance(13_000)
  expect(queries).toEqual([[FAILING]])
  const held = JSON.parse(files.get(RESULTS)?.text ?? '{}')
  // Hit at 12:00:10.150, after the lease's wait.
  expect(held.heldUntil).toBe(NOON + 10_150 + 120_000)
  expect(held.nextAt).toBe(held.heldUntil)
  // The card had data: it keeps it, untouched; the gh line says why it's not fresh.
  expect(entry()).toMatchObject({ at: NOON })
  expect(entry().failedSince).toBe(undefined)
  expect(await blipCard($)).toMatchObject({ warning: null, isFailedShown: true })
  expect((await ghFooter($)).lines).toEqual(['⏸ GitHub rate limit · polling resumes in 2m'])
  const line = JSON.parse(files.get(ERRORS_LOG)?.text.trim() ?? '{}')
  expect(line).toMatchObject({ login: 'me', batch: 1, status: 403, class: 'secondary-limit', retryAfter: '120', remaining: '4990', used: '10', resource: 'graphql' })

  // No round and no Refresh asks until the wait is over.
  await clock.advance(100_000)
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  await ui.press({ key: 'refresh' })
  await ui.unmount()
  expect(queries).toEqual([[FAILING]])
  // Counted down: the Refresh at 12:01:53.1 came 17.05s before the end.
  expect((await ghFooter($)).lines).toEqual(['⏸ GitHub rate limit · polling resumes in 18s'])
  // The tick at 12:02:10 is just inside the wait; the one at 12:02:20 asks.
  await clock.advance(20_000)
  expect(queries).toEqual([[FAILING]])
  await clock.advance(10_000)
  expect(queries).toEqual([[FAILING], [FAILING]])
  expect(entry().failedSince).toBe(undefined)
  // The pause is gone with the hold, the budget back in its place.
  expect((await ghFooter($)).lines).toEqual(['gh  me 4k/5k GraphQL · 4.9k/5k REST · resets 16:00'])
})

test('a secondary limit that names no wait holds a minute; a PR never read waits under the pause line, with no error of its own', async ($, on) => {
  const noWait = { ...SECONDARY_LIMIT, headers: SECONDARY_LIMIT.headers.filter(line => !line.startsWith('Retry-After')) }
  // The watch's own fetch, at noon, is the one held.
  const { clock, queries, files, entry } = await blipSession($, on, [noWait])
  expect(queries).toEqual([[FAILING]])
  expect(JSON.parse(files.get(RESULTS)?.text ?? '{}').heldUntil).toBe(NOON + 60_000)
  expect(entry()).toBe(undefined)
  expect(await blipCard($)).toMatchObject({ warning: null, checked: null })
  expect((await ghFooter($)).lines).toEqual(['⏸ GitHub rate limit · polling resumes in 1m'])
  await clock.advance(55_000)
  expect(queries).toEqual([[FAILING]])
  expect((await ghFooter($)).lines).toEqual(['⏸ GitHub rate limit · polling resumes in 10s'])
  // The round at 12:01:00 asks, and the card fills.
  await clock.advance(10_000)
  expect(queries).toEqual([[FAILING], [FAILING]])
  expect(await blipCard($)).toMatchObject({ warning: null, isFailedShown: true })
})

test('each failed call goes to errors.log as a line, the last 200 kept', async ($, on) => {
  const files = new Map<string, { text: string; mtimeMs: number }>()
  files.set(ERRORS_LOG, { text: Array.from({ length: 200 }, (_, i) => JSON.stringify({ old: i })).join('\n') + '\n', mtimeMs: 0 })
  const { clock, blips } = await blipSession($, on, [], files)
  blips.push(BAD_GATEWAY, { stderr: 'Post "https://api.github.com/graphql": EOF' })
  await clock.advance(13_000)
  const lines = (files.get(ERRORS_LOG)?.text ?? '').trimEnd().split('\n').map(line => JSON.parse(line))
  expect(lines).toHaveLength(200)
  expect(lines[0]).toEqual({ old: 2 })
  expect(lines.slice(-2)).toMatchObject([
    { at: '2026-10-07T12:00:10.150Z', login: 'me', batch: 1, status: 502, class: 'timeout', remaining: null, error: 'gh: HTTP 502' },
    { at: '2026-10-07T12:00:12.150Z', status: null, class: 'timeout', error: 'Post "https://api.github.com/graphql": EOF' },
  ])
})

test('a 504 that comes late in the lease is not retried, so the round ends inside it', async ($, on) => {
  const { clock, queries, blips, entry } = await blipSession($, on)
  queries.length = 0
  blips.push({ stderr: 'gh: HTTP 504', afterMs: 25_000 })
  await clock.advance(40_000)
  expect(queries).toEqual([[FAILING]])
  expect(entry().failedSince).toBe(NOON + 10_000)
})

test('a round whose retry fails too keeps the last data, and the card says nothing yet', async ($, on) => {
  const { clock, queries, blips, entry } = await blipSession($, on)
  queries.length = 0
  blips.push(BAD_GATEWAY, BAD_GATEWAY)
  await clock.advance(13_000)
  expect(queries).toEqual([[FAILING], [FAILING]])
  // Its data and time are the watch's, at noon.
  expect(entry()).toMatchObject({ at: NOON, failedSince: NOON + 10_000 })
  expect(entry().pr.number).toBe(13)
  expect(entry().error).toBe(undefined)
  expect(await blipCard($)).toEqual({ warning: null, checked: ' · checked 12s ago', isFailedShown: true })
})

test('a failure that lasts two minutes shows on the card, beside how old its data is, and goes once GitHub answers', async ($, on) => {
  // Three rounds in a row fail, retries and all: 12:00:10, 12:01:10, 12:02:10.
  const { clock, blips, entry } = await blipSession($, on, [])
  blips.push(...Array.from({ length: 6 }, () => BAD_GATEWAY))
  await clock.advance(73_000)
  expect((await blipCard($)).warning).toBe(null)
  await clock.advance(60_000)
  expect(entry()).toMatchObject({ at: NOON, failedSince: NOON + 10_000, error: 'gh: HTTP 502' })
  expect(await blipCard($)).toEqual({ warning: '⚠ gh: HTTP 502', checked: ' · checked 2m ago', isFailedShown: true })

  // The next round gets through: the error goes, and the data is fresh.
  await clock.advance(60_000)
  expect(entry().failedSince).toBe(undefined)
  expect(await blipCard($)).toMatchObject({ warning: null, checked: ' · checked just now' })
})

test("a PR whose first fetch fails says so at once, and the next round asks again, with no five-minute skip", async ($, on) => {
  const { clock, queries, entry } = await blipSession($, on, [BAD_GATEWAY, BAD_GATEWAY])
  await clock.advance(2_500)
  expect(queries).toEqual([[FAILING], [FAILING]])
  expect(entry()).toEqual({ at: NOON, error: 'gh: HTTP 502' })
  expect((await blipCard($)).warning).toBe('⚠ gh: HTTP 502')
  // The round at 12:00:10 asks it again, and gets through.
  await clock.advance(10_000)
  expect(queries).toEqual([[FAILING], [FAILING], [FAILING]])
  expect(entry().pr.number).toBe(13)
  expect(await blipCard($)).toMatchObject({ warning: null, isFailedShown: true })
})

test('a PR no gh account can see is left out of rounds for five minutes, then asked again', async ($, on) => {
  const GONE = 'https://github.com/octocat/gone/pull/4'
  const { clock, queries, files } = await blipSession($, on)
  await $.tool.call({ tool: 'mcp__pr-watch__watch', urls: [GONE] } as never)
  await clock.advance(100)
  const gone = () => JSON.parse(files.get(RESULTS)?.text ?? '{}').prs?.[GONE]
  expect(gone()).toMatchObject({ error: 'Not found, or no gh account can see it', isUnreachable: true })
  queries.length = 0
  // Rounds at 12:00:10 … 12:04:10 leave it out; the one at 12:05:10 asks.
  await clock.advance(4 * 60_000 + 10_000)
  expect(queries.flat()).not.toContain(GONE)
  await clock.advance(60_000)
  expect(queries.at(-1)).toContain(GONE)
})

test("an older version's failed entry still shows, and a round on top of it is quiet", async ($, on) => {
  const files = new Map<string, { text: string; mtimeMs: number }>()
  const { clock, blips, entry } = await blipSession($, on, [], files)
  // A session on a version before this one ran a round that failed: its entry has the error
  // and the round's time, and no failedSince.
  const shared = JSON.parse(files.get(RESULTS)?.text ?? '{}')
  const at = clock.now()
  const older = { ...shared, fetchedAt: at, nextAt: at + 60_000, prs: { [FAILING]: { ...shared.prs[FAILING], at, error: 'gh: HTTP 502' } } }
  files.set(RESULTS, { text: JSON.stringify(older), mtimeMs: at })
  await clock.advance(10_000)
  expect((await blipCard($)).warning).toBe('⚠ gh: HTTP 502')

  // This version's next round fails too: counted from the older one's failure, a minute is
  // not yet two, so the card is quiet again.
  blips.push(BAD_GATEWAY, BAD_GATEWAY)
  await clock.advance(63_000)
  expect(entry()).toMatchObject({ at, failedSince: at })
  expect(entry().error).toBe(undefined)
  expect((await blipCard($)).warning).toBe(null)
})

test('a round drops the PRs no live session watches once an hour old; a watch or Refresh drops nothing', async ($, on) => {
  const files = new Map<string, { text: string; mtimeMs: number }>()
  const old = (hours: number) => ({ at: NOON - hours * 3_600_000, pr: GH[MERGED] })
  const LIVE_OLD = 'https://github.com/acme/api/pull/1'
  const DEAD_OLD = 'https://github.com/acme/api/pull/2'
  const DEAD_RECENT = 'https://github.com/acme/api/pull/3'
  const UNREACHABLE = 'https://github.com/octocat/gone/pull/4'
  // Another session, alive, watches a PR that merged three hours ago.
  files.set('/home/me/.cache/pr-watch/sessions/other.json', { text: JSON.stringify({ urls: [LIVE_OLD] }), mtimeMs: NOON })
  const prs = { [LIVE_OLD]: old(3), [DEAD_OLD]: old(2), [DEAD_RECENT]: old(0.5), [UNREACHABLE]: { at: NOON - 7_200_000, error: 'Not found, or no gh account can see it' } }
  files.set(RESULTS, { text: JSON.stringify({ fetchedAt: 0, nextAt: 0, prs, accounts: {}, rate: null }), mtimeMs: NOON })
  const { clock, queries } = await blipSession($, on, [], files)
  const kept = () => Object.keys(JSON.parse(files.get(RESULTS)?.text ?? '{}').prs).sort()

  // The watch's own fetch is no round.
  expect(kept()).toEqual([DEAD_OLD, DEAD_RECENT, FAILING, LIVE_OLD, UNREACHABLE].sort())
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  await ui.press({ key: 'refresh' })
  await ui.unmount()
  expect(kept()).toEqual([DEAD_OLD, DEAD_RECENT, FAILING, LIVE_OLD, UNREACHABLE].sort())

  queries.length = 0
  await clock.advance(10_500)
  // The merged PR is asked nothing, and kept: a live session still shows it.
  expect(queries).toEqual([[FAILING]])
  expect(kept()).toEqual([DEAD_RECENT, FAILING, LIVE_OLD].sort())
})

// ---- The gh line ------------------------------------------------------------------------

test("the gh line: the poll's points at once, REST's requests once a round asks, dim while healthy", async ($, on) => {
  const { clock, restAsks, files } = await blipSession($, on)
  // The watch's own fetch said the points, and asked nothing more.
  expect(restAsks).toEqual([])
  expect(await ghFooter($)).toEqual({ lines: ['gh  me 4k/5k GraphQL · resets 16:00'], tones: {}, isDim: true })
  await clock.advance(10_500)
  expect(restAsks).toEqual(['me'])
  expect(await ghFooter($)).toEqual({ lines: ['gh  me 4k/5k GraphQL · 4.9k/5k REST · resets 16:00'], tones: {}, isDim: true })
  const { rates, rate } = JSON.parse(files.get(RESULTS)?.text ?? '{}')
  expect(rates.me).toEqual({
    graphql: { limit: 5_000, remaining: 4_000, resetAt: POINTS_RESET, seenAt: NOON + 10_150 },
    core: { limit: 5_000, remaining: 4_900, resetAt: REQUESTS_RESET, seenAt: NOON + 10_150 },
    askedAt: NOON + 10_000,
  })
  // What an older version reads.
  expect(rate).toEqual({ limit: 5_000, remaining: 4_000, resetAt: '2026-10-07T13:00:00.000Z' })
})

test('REST is asked at most every five minutes per login, and only by a round', async ($, on) => {
  const { clock, restAsks } = await blipSession($, on)
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  await ui.press({ key: 'refresh' })
  await ui.unmount()
  expect(restAsks).toEqual([])
  // Rounds at 12:00:10, 12:01:10 … 12:04:10: the first asks.
  await clock.advance(10_500 + 4 * 60_000)
  expect(restAsks).toEqual(['me'])
  // 12:05:10: five minutes on.
  await clock.advance(60_000)
  expect(restAsks).toEqual(['me', 'me'])
})

test('below 20% of the limit a bucket is a warning, below 5% an error, and so is its reset', async ($, on) => {
  const { clock, budget } = await blipSession($, on)
  budget.points.me = 900
  await clock.advance(10_500)
  expect(await ghFooter($)).toMatchObject({
    lines: ['gh  me 900/5k GraphQL · 4.9k/5k REST · resets 16:00'],
    tones: { '900/5k GraphQL': 'warning', 'resets 16:00': 'warning' },
  })
  budget.points.me = 4_000
  budget.requests.me = 200
  await clock.advance(5 * 60_000)
  expect(await ghFooter($)).toMatchObject({
    lines: ['gh  me 4k/5k GraphQL · 200/5k REST · resets 16:30'],
    tones: { '200/5k REST': 'error', 'resets 16:30': 'error' },
  })
})

const SPENT = 'https://github.com/hubot/app/pull/5'

test("one login out of points holds back only its own PRs: the other's are polled as ever", async ($, on) => {
  const files = new Map<string, { text: string; mtimeMs: number }>()
  // hubot's owner is read as hubot; acme, by the active login.
  files.set(RESULTS, { text: JSON.stringify({ fetchedAt: 0, nextAt: 0, prs: {}, accounts: { hubot: 'hubot' }, rate: null }), mtimeMs: NOON })
  const { clock, queries, restAsks, budget } = await blipSession($, on, [], files, {
    fixtures: { ...GH, [SPENT]: pr(SPENT, {}) },
    setup: host => {
      host.budget.logins = ['octocat', 'hubot']
      host.budget.points.hubot = 0
    },
  })
  await $.tool.call({ tool: 'mcp__pr-watch__watch', urls: [SPENT] } as never)
  await clock.advance(100)
  const shared = () => JSON.parse(files.get(RESULTS)?.text ?? '{}')
  // Read once, its points spent: 0 left until 13:00.
  expect(shared().prs[SPENT].at).toBe(NOON + 100)
  queries.length = 0
  await clock.advance(10_000)
  // The round at 12:00:10 asks octocat's PR alone, and runs again a minute on, not at 13:00.
  expect(queries).toEqual([[FAILING]])
  expect(shared().nextAt).toBe(NOON + 70_000)
  expect(shared().prs[SPENT]).toMatchObject({ at: NOON + 100 })
  expect(shared().prs[SPENT].failedSince).toBe(undefined)
  // REST is asked for both logins, each with its own token.
  expect(restAsks).toEqual(['octocat', 'hubot'])
  expect(await ghFooter($)).toMatchObject({
    lines: ['gh  octocat 4k/5k GraphQL · 4.9k/5k REST', '    hubot 0/5k GraphQL · 4.9k/5k REST · resets 16:00'],
    tones: { '0/5k GraphQL': 'error', 'resets 16:00': 'error' },
  })
  await clock.advance(60_000)
  expect(queries).toEqual([[FAILING], [FAILING]])

  // Past hubot's reset, its PR is asked again.
  budget.points.hubot = 5_000
  await clock.advance(Date.parse('2026-10-07T13:00:10Z') - clock.now() + 500)
  expect(queries.at(-1)).toContain(SPENT)
})

test('every login the round needs out of points: the round waits for the first reset', async ($, on) => {
  const { clock, queries, files } = await blipSession($, on, [], new Map(), { setup: host => void (host.budget.points.me = 100) })
  queries.length = 0
  await clock.advance(10_500)
  // The watch's fetch saw 100 left: the round asks nothing, and waits for 13:00.
  expect(queries).toEqual([])
  expect(JSON.parse(files.get(RESULTS)?.text ?? '{}').nextAt).toBe(POINTS_RESET)
})

test('a round leaves a batch it has no time for to the next round, untouched, and each call takes no more than is left', async ($, on) => {
  const many = Array.from({ length: 21 }, (_, i) => `https://github.com/acme/app/pull/${200 + i}`)
  const fixtures = { ...GH, ...Object.fromEntries(many.map(url => [url, pr(url, {})])) }
  const { clock, queries, files, blips, timeouts } = await blipSession($, on, [], new Map(), { fixtures })
  await $.tool.call({ tool: 'mcp__pr-watch__watch', urls: many } as never)
  await clock.advance(100)
  queries.length = 0
  timeouts.length = 0
  // The round at 12:00:10: its first batch fails slowly, twice. Started at 12:00:10.150 it
  // may take 20s; the retry, at 12:00:30.150, the 8.85s left of the lease; by 12:00:36.150
  // the 2.85s left are too few for the second batch.
  blips.push({ ...BAD_GATEWAY, afterMs: 18_000 }, { ...BAD_GATEWAY, afterMs: 6_000 })
  await clock.advance(30_000)
  const first = queries[0] ?? []
  expect(first).toHaveLength(20)
  expect(queries).toEqual([first, first])
  expect(timeouts).toEqual([20_000, 8_850])
  const prs = JSON.parse(files.get(RESULTS)?.text ?? '{}').prs
  const late = [FAILING, ...many].filter(url => !first.includes(url))
  expect(late).toHaveLength(2)
  for (const url of late) {
    expect(prs[url].pr.number).toBe(Number(url.split('/').pop()))
    expect(prs[url].failedSince).toBe(undefined)
  }
  // The next round, at 12:01:10, asks them first.
  await clock.advance(40_000)
  expect(queries[2]?.slice(0, 2)).toEqual(late)
})

test("an older version's results.json still reads, and its low `rate` holds nothing back", async ($, on) => {
  const files = new Map<string, { text: string; mtimeMs: number }>()
  const older = { fetchedAt: NOON, nextAt: NOON + 30_000, prs: { [FAILING]: { at: NOON, pr: GH[FAILING] } }, accounts: {}, rate: { remaining: 100, resetAt: '2026-10-07T13:00:00Z' } }
  files.set(RESULTS, { text: JSON.stringify(older), mtimeMs: NOON })
  const { clock, queries } = await blipSession($, on, [], files)
  expect((await blipCard($)).isFailedShown).toBe(true)
  queries.length = 0
  // Its round, due at 12:00:30, runs then: not at the old rate's 13:00.
  await clock.advance(30_500)
  expect(queries).toEqual([[FAILING]])
  const shared = JSON.parse(files.get(RESULTS)?.text ?? '{}')
  expect(shared.rates.me.graphql.remaining).toBe(4_000)
  expect(shared.rate.remaining).toBe(4_000)
  expect((await ghFooter($)).lines).toEqual(['gh  me 4k/5k GraphQL · 4.9k/5k REST · resets 16:00'])
})

// ---- The band layout ----------------------------------------------------------------------

const BAND = {
  plugin: 'pr-watch',
  component: 'AbovePrompt' as const,
  props: { hasSurvey: false, isWorking: false, maxRows: 20, bodyColumns: 75, scroll: { offset: 0, bodyRows: 19 }, view: {} },
  viewport: { columns: 80, rows: 40, isFullscreen: true },
}
// What the engine draws in the band of its own: nothing, but a survey.
const NOTHING = { type: 'engine', ref: 0 } as const
const IN_BAND = { options: { layout: 'band' } }
const bandAt = (props: Partial<(typeof BAND)['props']>) => ({ ...BAND, props: { ...BAND.props, ...props } })

type Drawn = { type: string; props?: Record<string, unknown>; children?: unknown[] }

// The band as the terminal lays it out, line by line: a column Box stacks its children, any
// other Box joins them across (its gap in spaces), a plain Button is "o: Open pane", a Raster
// (the spinner) one '⠋'. What the engine draws itself is no line.
function lines(node: unknown): string[] {
  if (typeof node === 'string') return [node]
  const el = node as Drawn
  if (el.type === 'engine') return []
  if (el.type === 'Button') return [`${el.props?.hotkey ? `${el.props.hotkey}: ` : ''}${el.props?.label}`]
  if (el.type === 'Raster') return ['⠋']
  const kids = (el.children ?? []).map(lines).filter(kid => kid.length > 0)
  if (el.type === 'Box' && el.props?.flexDirection === 'column') return kids.flat()

  return [kids.map(kid => kid.join('')).join(el.type === 'Box' ? ' '.repeat(Number(el.props?.gap ?? 0)) : '')]
}

type BandHost = { fixtures?: Record<string, unknown>; beneath?: RenderElement; fill?: () => { isFilled: boolean } }

// A session watching `urls` (by the watch tool) on `fixtures`, over a band the engine leaves
// empty unless another plugin draws `beneath`, and a prompt box that takes drafts while `fill`
// says so; `opens` is each pane open since, with whether it asked for the keys, and `fills` the
// drafts.
async function bandSession($: Parameters<TestBody>[0], on: On, urls: readonly string[], { fixtures = GH, beneath, fill = () => ({ isFilled: true }) }: BandHost = {}) {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  const opens: { id: string; focus?: true }[] = []
  const fills: string[] = []
  const blits: { requestId: string; key: string }[] = []
  const closes: string[] = []
  on('ui.render', { component: 'AbovePrompt' }, () => beneath ?? NOTHING)
  on('prompt.read', () => ({ value: { text: '', cursor: 0 } }))
  on('prompt.fill', ($, e) => (fills.push(e.text), fill()))
  on('prompt.submit', ($, e) => ({ text: e.text }))
  on('ui.open', ($, e) => (opens.push({ id: e.id, focus: e.focus }), { value: { isPlaced: true } }))
  on('ui.close', ($, e) => (closes.push(e.id), { value: undefined }))
  on('ui.toast', () => ({ value: undefined }))
  on('ui.status', () => ({ value: undefined }))
  on('ui.blit', ($, e) => (blits.push({ requestId: e.requestId, key: e.key }), { value: {} }))
  const host = fakeHost(on, clock, fixtures)
  on('command.register', () => ({ value: { command: 'pr-watch' } }))
  on('tool.register', () => ({ value: { tool: 'mcp__pr-watch__watch' } }))
  on('session.start', () => ({ cwd: '/work' }))
  await $.session.start({ source: 'startup', cwd: '/work' } as never)
  if (urls.length > 0) await $.tool.call({ tool: 'mcp__pr-watch__watch', urls } as never)
  await clock.advance(100)

  return { ...host, clock, opens, fills, blits, closes }
}

test('the band: a line per PR, most urgent first, then its buttons', IN_BAND, async ($, on) => {
  const { blits, clock } = await bandSession($, on, [RUNNING, FAILING, MERGED])
  const ui = await $.ui.mount({ ...BAND, surface: 'terminal', requestId: 'band' })
  const drawn = lines(await ui.drawn())
  expect(drawn).toEqual([
    '✗ #13 Retry uploads with exponential backoff · 1 check failing',
    '⠋ #12 Retry uploads with exponential backoff · CI running · 1/3',
    '✓ #7 Retry uploads with exponential backoff · merged',
    'o: Open pane  r: Refresh  f: Fix #13 with Claude',
  ])
  // The same spinner as the card, blitted in the band.
  expect(await ui.find({ key: `band-spin:${RUNNING}` })).toBeDefined()
  blits.length = 0
  await clock.advance(200)
  expect(blits).toContainEqual({ requestId: 'band', key: `band-spin:${RUNNING}` })
  // No pane is drawn in the band layout, so no frame goes there.
  expect(blits.filter(blit => blit.requestId === 'pr-watch')).toEqual([])
  expect((await ui.find({ key: 'band-open' }))?.props).toMatchObject({ label: 'Open pane', hotkey: 'o', plain: true })
  expect((await ui.find({ key: 'band-refresh' }))?.props).toMatchObject({ label: 'Refresh', hotkey: 'r' })
  // No digit hotkeys: a bare digit in an empty prompt would press them.
  for (const button of await ui.findAll({ type: 'Button' })) expect(String(button.props.hotkey)).toMatch(/^[a-z]$/)
  await ui.unmount()

  // No Raster on a desktop: the card's text spinner.
  const desk = await $.ui.mount({ ...BAND, surface: 'desktop' })
  expect(lines(await desk.drawn())[1]).toBe('◌ #12 Retry uploads with exponential backoff · CI running · 1/3')
  await desk.unmount()
})

test('a narrow band cuts titles to fit and never wraps a line', IN_BAND, async ($, on) => {
  await bandSession($, on, [RUNNING, FAILING, MERGED])
  for (const bodyColumns of [30, 44, 60]) {
    const ui = await $.ui.mount({ ...bandAt({ bodyColumns }), surface: 'terminal' })
    const drawn = lines(await ui.drawn())
    for (const line of drawn.slice(0, 3)) expect([...line].length).toBeLessThanOrEqual(bodyColumns)
    expect(drawn[0]).toMatch(/^✗ #13 .*1 check failing$/)
    await ui.unmount()
  }
  const ui = await $.ui.mount({ ...bandAt({ bodyColumns: 44 }), surface: 'terminal' })
  expect(lines(await ui.drawn())[0]).toBe('✗ #13 Retry uploads with… · 1 check failing')
  await ui.unmount()
})

test('more PRs than rows: the most urgent, and "+N more" on the action line; five rows at most', IN_BAND, async ($, on) => {
  const many = Array.from({ length: 6 }, (_, i) => `https://github.com/acme/app/pull/${100 + i}`)
  const fixtures = { ...GH, ...Object.fromEntries(many.map(url => [url, pr(url, {})])) }
  await bandSession($, on, [...many, FAILING], { fixtures })

  const roomy = await $.ui.mount({ ...bandAt({ maxRows: 30 }), surface: 'terminal' })
  const drawn = lines(await roomy.drawn())
  expect(drawn).toHaveLength(5)
  expect(drawn[0]).toMatch(/^✗ #13 /)
  expect(drawn[1]).toMatch(/^◷ #100 .* · waiting for review$/)
  expect(drawn[4]).toBe('+3 more  o: Open pane  r: Refresh  f: Fix #13 with Claude')
  await roomy.unmount()

  // What the bottom slot spares, when less.
  const tight = await $.ui.mount({ ...bandAt({ maxRows: 3 }), surface: 'terminal' })
  expect(lines(await tight.drawn())).toHaveLength(3)
  expect(lines(await tight.drawn())[2]).toMatch(/^\+5 more {2}o: Open pane/)
  await tight.unmount()
})

test('nothing watched, a survey up, or the pane layout: the band is left alone', async ($, on) => {
  await bandSession($, on, [])
  const empty = await $.ui.mount({ ...BAND, surface: 'terminal' })
  expect((await empty.drawn()).type).toBe('engine')
  await empty.unmount()

  // The default layout: watched PRs, and still nothing in the band.
  await $.tool.call({ tool: 'mcp__pr-watch__watch', urls: [FAILING] } as never)
  const pane = await $.ui.mount({ ...BAND, surface: 'terminal' })
  expect((await pane.drawn()).type).toBe('engine')
  await pane.unmount()
})

test('a survey holds the band: pr-watch yields', IN_BAND, async ($, on) => {
  await bandSession($, on, [FAILING])
  const ui = await $.ui.mount({ ...bandAt({ hasSurvey: true }), surface: 'terminal' })
  expect((await ui.drawn()).type).toBe('engine')
  await ui.redraw({ ...BAND.props, hasSurvey: false })
  expect(lines(await ui.drawn())[0]).toMatch(/^✗ #13 /)
  await ui.unmount()
})

test('in the band layout new PRs never open the pane; /pr-watch and Open pane do', IN_BAND, async ($, on) => {
  const { opens } = await bandSession($, on, [FAILING])
  await $.prompt.submit({ text: `look at ${RUNNING}` } as never)
  expect(opens).toEqual([])

  await $.command.run({ command: 'pr-watch', args: '', origin: { kind: 'composer' }, presentation: { isFullscreen: false, columns: 80 } })
  expect(opens).toEqual([{ id: 'pr-watch', focus: undefined }])

  const ui = await $.ui.mount({ ...BAND, surface: 'terminal' })
  await ui.press({ key: 'band-open' })
  expect(opens[1]).toEqual({ id: 'pr-watch', focus: true })
  await ui.unmount()
})

test('Refresh in the band asks GitHub now', IN_BAND, async ($, on) => {
  const { queries } = await bandSession($, on, [FAILING, RUNNING])
  queries.length = 0
  const ui = await $.ui.mount({ ...BAND, surface: 'terminal' })
  await ui.press({ key: 'band-refresh' })
  expect(queries.flat().sort()).toEqual([FAILING, RUNNING].sort())
  await ui.unmount()
})

test("beside another plugin's lines, pr-watch keeps them under its own and takes two rows", IN_BAND, async ($, on) => {
  await bandSession($, on, [RUNNING, FAILING, MERGED], { beneath: { type: 'Text', children: ['deploy: green'] } })
  const ui = await $.ui.mount({ ...BAND, surface: 'terminal' })
  expect(lines(await ui.drawn())).toEqual([
    '✗ #13 Retry uploads with exponential backoff · 1 check failing',
    '+2 more  o: Open pane  r: Refresh  f: Fix #13 with Claude',
    'deploy: green',
  ])
  await ui.unmount()
})

test('switching the layout in /config applies at once, both ways', async ($, on) => {
  on('config.set', ($, e) => ({ value: e.value }))
  const { opens, closes } = await bandSession($, on, [FAILING])
  // The pane layout opened the pane for the new PR, and draws nothing in the band.
  expect(opens).toHaveLength(1)
  const ui = await $.ui.mount({ ...BAND, surface: 'terminal' })
  expect((await ui.drawn()).type).toBe('engine')

  await $.config.set({ key: 'pr-watch.layout', value: 'band' } as never)
  expect(lines(await ui.drawn())[0]).toMatch(/^✗ #13 /)
  // The pane closes, and a new PR doesn't open it again.
  expect(closes).toEqual(['pr-watch'])
  await $.tool.call({ tool: 'mcp__pr-watch__watch', urls: [RUNNING] } as never)
  expect(opens).toHaveLength(1)

  await $.config.set({ key: 'pr-watch.layout', value: 'pane' } as never)
  expect((await ui.drawn()).type).toBe('engine')
  // Back on the pane, it opens as a session start would open it.
  expect(opens).toHaveLength(2)
  await ui.unmount()
})

test('a newer pr-watch shows on the band\'s action line, short where the line is short', IN_BAND, async ($, on) => {
  on('ui.render', { component: 'AbovePrompt' }, () => NOTHING)
  const { clock } = await newerSession($, on, { ...GH, pw: onGitHub('0.9.0') }, [RUNNING], [])
  await clock.advance(10_500)
  const wide = await $.ui.mount({ ...BAND, surface: 'terminal' })
  expect(lines(await wide.drawn()).at(-1)).toBe('o: Open pane  r: Refresh  ↑ pr-watch 0.9.0 available')
  await wide.unmount()
  const narrow = await $.ui.mount({ ...bandAt({ bodyColumns: 40 }), surface: 'terminal' })
  expect(lines(await narrow.drawn()).at(-1)).toBe('o: Open pane  r: Refresh  ↑ 0.9.0')
  await narrow.unmount()
})

test("Fix with Claude in the band drafts the most urgent PR's failure; where it can't, the pane opens on it", IN_BAND, async ($, on) => {
  let isFilling = true
  const { fills, opens } = await bandSession($, on, [RUNNING, BROKEN], { fixtures: { [BROKEN]: broken, [RUNNING]: GH[RUNNING] }, fill: () => ({ isFilled: isFilling }) })
  const ui = await $.ui.mount({ ...BAND, surface: 'terminal' })
  expect(lines(await ui.drawn()).at(-1)).toBe('o: Open pane  r: Refresh  f: Fix #31 with Claude')
  await ui.press({ key: 'band-handoff' })
  expect(fills).toHaveLength(1)
  expect(fills[0]).toContain('CI failed on acme/app#31')
  expect(opens).toEqual([])

  // No prompt box to take the draft: the question is the card's, so the pane opens with the keys.
  isFilling = false
  await ui.press({ key: 'band-handoff' })
  expect(opens).toEqual([{ id: 'pr-watch', focus: true }])
  await ui.unmount()
})

test('the band says the gh budget only once it runs low, short where the line is short', IN_BAND, async ($, on) => {
  const { budget, clock } = await bandSession($, on, [FAILING])
  const last = async (bodyColumns = 75) => {
    const ui = await $.ui.mount({ ...bandAt({ bodyColumns }), surface: 'terminal' })
    const line = lines(await ui.drawn()).at(-1)
    await ui.unmount()

    return line
  }
  expect(await last()).toBe('o: Open pane  r: Refresh  f: Fix #13 with Claude')
  budget.points.me = 900
  await clock.advance(10_500)
  expect(await last()).toBe('o: Open pane  r: Refresh  f: Fix #13 with Claude  gh 18% · resets 16:00')
  expect(await last(60)).toBe('o: Open pane  r: Refresh  f: Fix #13 with Claude  gh 18%')
  expect(await last(50)).toBe('o: Open pane  r: Refresh  f: Fix #13 with Claude')
})

test('a secondary limit: the band says the pause, counted down, and the line goes with the hold', IN_BAND, async ($, on) => {
  const { blips, clock, queries } = await bandSession($, on, [FAILING])
  const drawn = async () => {
    const ui = await $.ui.mount({ ...BAND, surface: 'terminal' })
    const said = lines(await ui.drawn())
    await ui.unmount()

    return said
  }
  // The round at 12:00:10 hits it, at 12:00:10.150: held until 12:02:10.150.
  blips.push(SECONDARY_LIMIT)
  await clock.advance(10_500)
  // The pause before the hand-off, where both won't fit.
  expect(await drawn()).toEqual(['✗ #13 Retry uploads with exponential backoff · 1 check failing', 'o: Open pane  r: Refresh  ⏸ GitHub rate limit · polling resumes in 2m'])
  await clock.advance(30_000)
  expect((await drawn()).at(-1)).toBe('o: Open pane  r: Refresh  ⏸ GitHub rate limit · polling resumes in 1m 31s')
  // Gone the moment the hold ends, before any tick; the round at 12:02:20 then asks.
  queries.length = 0
  await clock.advance(130_300 - 40_600)
  expect((await drawn()).at(-1)).toBe('o: Open pane  r: Refresh  f: Fix #13 with Claude')
  expect(queries).toEqual([])
  await clock.advance(10_000)
  expect(queries).toEqual([[FAILING]])
})

// ---- Themes -------------------------------------------------------------------------------

// Claude Code's own `theme` row and pr-watch's, as /config lists them; `sets` records each
// change a plugin makes through $.config.set.
function configRows(on: On, rows: { theme?: string; own?: string } = {}) {
  const sets: { key: string; value: unknown }[] = []
  const row = (key: string, value: string, plugin: string) => ({ key, label: key, kind: 'choice', value, provider: { plugin, tier: 'user' }, isLocked: false })
  on('config.list', () => ({
    value: [
      ...(rows.theme === undefined ? [] : [row('theme', rows.theme, 'engine')]),
      ...(rows.own === undefined ? [] : [row('pr-watch.theme', rows.own, 'pr-watch')]),
    ] as never,
  }))
  on('config.set', ($, e) => (sets.push({ key: e.key, value: e.value }), { value: e.value }))

  return sets
}

// A session watching `urls`, the pane drawn on the terminal.
async function themedPane($: Parameters<TestBody>[0], on: On, urls: readonly string[]) {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  quietUi(on)
  fakeHost(on, clock, GH)
  on('command.register', () => ({ value: { command: 'pr-watch' } }))
  on('tool.register', () => ({ value: { tool: 'mcp__pr-watch__watch' } }))
  on('session.start', () => ({ cwd: '/work' }))
  await $.session.start({ source: 'startup', cwd: '/work' } as never)
  await $.command.run({ command: 'pr-watch', args: urls.join(' '), origin: { kind: 'composer' }, presentation: { isFullscreen: true, columns: 180 } })
  await clock.advance(100)

  return { clock, ui: await $.ui.mount({ ...PANE, surface: 'terminal' }) }
}

// The hover styles of the element keyed `key` in a drawn tree (find leaves them out).
function hoverOf(node: unknown, key: string): Record<string, unknown> | undefined {
  if (node === null || typeof node !== 'object') return undefined
  const el = node as { props?: { key?: unknown }; hover?: Record<string, unknown>; children?: unknown[] }
  if (el.props?.key === key) return el.hover

  return (el.children ?? []).map(kid => hoverOf(kid, key)).find(found => found !== undefined)
}

const theme = ($: Parameters<TestBody>[0], args: string) =>
  $.command.run({ command: 'pr-watch', args: `theme ${args}`.trim(), origin: { kind: 'composer' }, presentation: { isFullscreen: true, columns: 180 } })

test('the classic look by default: round borders in the tone, a filled pill', async ($, on) => {
  const { ui } = await themedPane($, on, [FAILING])
  expect((await ui.find({ key: `card:${FAILING}` }))?.props).toMatchObject({ borderStyle: 'round', borderColor: 'error', borderDimColor: true, paddingX: 1 })
  expect(hoverOf(await ui.drawn(), `card:${FAILING}`)).toEqual({ borderDimColor: false })
  expect((await ui.find({ type: 'Text', text: /CI FAILED/ }))?.props).toMatchObject({ backgroundColor: 'error', color: 'inverseText', bold: true })
  await ui.unmount()
})

test('solid: borderless tiles tinted by state, the same size as a bordered card', { options: { theme: 'solid' } }, async ($, on) => {
  const { ui } = await themedPane($, on, [FAILING, RUNNING, MERGED])
  const fill = async (url: string) => (await ui.find({ key: `card:${url}` }))?.props
  const failing = await fill(FAILING)
  expect(failing?.borderStyle).toBeUndefined()
  // Two cells each side and a row above and below, as a border and its padding take.
  expect(failing).toMatchObject({ paddingX: 2, paddingY: 1 })
  expect(failing?.backgroundColor).toMatch(/^#[0-9a-f]{6}$/)
  // Red failing, amber running, purple merged: three tints.
  expect(new Set([failing?.backgroundColor, (await fill(RUNNING))?.backgroundColor, (await fill(MERGED))?.backgroundColor]).size).toBe(3)
  expect((await ui.find({ type: 'Text', text: /CI FAILED/ }))?.props).toMatchObject({ backgroundColor: 'error', bold: true })
  // The bar and the spinner still draw on the tile.
  expect(await ui.find({ key: `bar:${RUNNING}` })).toBeDefined()
  expect(await ui.find({ key: `spin:${RUNNING}` })).toBeDefined()
  await ui.unmount()
})

test('minimal: no boxes, a thin rule between PRs, colour on the pill and headline only', { options: { theme: 'minimal' } }, async ($, on) => {
  const { ui } = await themedPane($, on, [FAILING, RUNNING, MERGED])
  const card = (await ui.find({ key: `card:${FAILING}` }))?.props
  expect(card?.borderStyle).toBeUndefined()
  expect(card?.backgroundColor).toBeUndefined()
  expect(card?.paddingX).toBe(2)
  // A rule before every card but the first.
  const rules = (await ui.findAll({ type: 'Box' })).filter(box => box.key?.startsWith('sep:'))
  expect(rules.map(box => box.key)).toEqual([`sep:${RUNNING}`, `sep:${MERGED}`])
  expect(rules[0]?.text).toBe('─'.repeat(60))
  const pill = (await ui.find({ type: 'Text', text: /CI FAILED/ }))?.props
  expect(pill).toMatchObject({ color: 'error', bold: true })
  expect(pill?.backgroundColor).toBeUndefined()
  expect((await ui.find({ type: 'Text', text: /^#13$/ }))?.props.color).toBeUndefined()
  await ui.unmount()
})

test('/pr-watch theme lists the looks, marks the current one, and says what names there are', async ($, on) => {
  configRows(on)
  const { ui } = await themedPane($, on, [FAILING])
  const listed = (await theme($, '')).text ?? ''
  expect(listed.split('\n').filter(line => line.startsWith('●'))).toEqual([expect.stringMatching(/^● classic .*\(current\)$/)])
  for (const name of ['classic', 'solid', 'minimal', 'neon', 'retro']) expect(listed).toContain(name)
  expect((await theme($, 'sparkly')).text).toBe('No theme "sparkly". Themes: classic, solid, minimal, neon, retro.')
  // Nothing changed.
  expect((await ui.find({ key: `card:${FAILING}` }))?.props.borderStyle).toBe('round')
  await ui.unmount()
})

test('/pr-watch theme Neon restyles the pane at once and saves it in /config', async ($, on) => {
  const sets = configRows(on, { own: 'classic' })
  const { ui } = await themedPane($, on, [FAILING])
  const said = await theme($, 'Neon')
  expect(said.text).toBe('pr-watch theme: neon. Saved in /config, so every session follows.')
  expect(sets).toEqual([{ key: 'pr-watch.theme', value: 'neon' }])
  const card = (await ui.find({ key: `card:${FAILING}` }))?.props
  expect(card).toMatchObject({ borderStyle: 'bold', borderColor: expect.stringMatching(/^#/) })
  // The glow: a brighter border while the pointer is on the card.
  const hover = hoverOf(await ui.drawn(), `card:${FAILING}`)
  expect(hover?.borderColor).toMatch(/^#/)
  expect(hover?.borderColor).not.toBe(card?.borderColor)
  expect(listedCurrent((await theme($, '')).text)).toBe('neon')
  await ui.unmount()
})

const listedCurrent = (text: string | undefined) => text?.split('\n').find(line => line.startsWith('●'))?.split(/\s+/)[1]

test('where /config has no row for it, /pr-watch theme holds for the session and says so', async ($, on) => {
  configRows(on)
  const { ui } = await themedPane($, on, [FAILING])
  expect((await theme($, 'retro')).text).toBe('pr-watch theme: retro, for this session. Pick it in /config (pr-watch, theme) to keep it for every session.')
  expect((await ui.find({ key: `card:${FAILING}` }))?.props).toMatchObject({ borderStyle: 'double', borderColor: 'ansi256(1)' })
  expect((await ui.find({ type: 'Text', text: /CI FAILED/ }))?.children).toEqual(['[', '✗ CI FAILED', ']'])
  await ui.unmount()
})

test('a theme picked in /config restyles at once, and a light Claude Code theme gets the light tints', async ($, on) => {
  configRows(on, { theme: 'dark' })
  const { ui } = await themedPane($, on, [FAILING])
  await $.config.set({ key: 'pr-watch.theme', value: 'solid' } as never)
  const dark = (await ui.find({ key: `card:${FAILING}` }))?.props.backgroundColor
  expect(dark).toMatch(/^#/)
  await $.config.set({ key: 'theme', value: 'light' } as never)
  const light = (await ui.find({ key: `card:${FAILING}` }))?.props.backgroundColor
  expect(light).toMatch(/^#/)
  expect(light).not.toBe(dark)
  await $.config.set({ key: 'pr-watch.theme', value: 'classic' } as never)
  expect((await ui.find({ key: `card:${FAILING}` }))?.props.borderStyle).toBe('round')
  await ui.unmount()
})

test("the band takes the theme's colours, and keeps its lines", { options: { layout: 'band', theme: 'retro' } }, async ($, on) => {
  await bandSession($, on, [FAILING])
  const ui = await $.ui.mount({ ...BAND, surface: 'terminal' })
  expect(lines(await ui.drawn())[0]).toBe('✗ #13 Retry uploads with exponential backoff · 1 check failing')
  expect((await ui.find({ type: 'Text', text: /^#13$/ }))?.props.color).toBe('ansi256(1)')
  expect((await ui.find({ type: 'Text', text: /^1 check failing$/ }))?.props.color).toBe('ansi256(1)')
  await ui.unmount()
})
