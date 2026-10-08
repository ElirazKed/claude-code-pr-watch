import { expect, mock, test } from 'claude-code/testing'
import type { TestBody } from 'claude-code/testing'

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

const ALIAS = /p(\d+): repository\(owner: "([^"]+)", name: "([^"]+)"\) \{ pullRequest\(number: (\d+)\)/g

// Stands for gh, $HOME and the file system: answers batched queries from `gh`, keeps files in
// memory (stamped with the mocked time, so mtimes are heartbeats), and records each gh argv.
// `branchPr`: what `gh pr view` answers for the current branch. `merge`: how `gh pr merge`
// ends (now, or once a promise settles); by default it goes through, and `merges` records each
// one's argv. `runLog`: what
// `gh run view` prints, a failed job's log by default; `runs` records each argv.
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
  on('env.get', () => ({ value: '/home/me' }))
  on('fs.read', ($, e) => ({ value: files.get(e.path)?.text ?? '' }))
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
    if (e.argv[1] === 'auth') return out(0, '  ✓ Logged in to github.com account me (keyring)\n  - Active account: true\n')
    if (e.argv[1] === 'pr' && e.argv[2] === 'view') return branchPr ? out(0, `${branchPr}\n`) : out(1, '')
    if (e.argv[1] !== 'api') return out(1, '')
    const query = String(e.argv[4] ?? '').slice('query='.length)
    const urls: string[] = []
    const data: Record<string, unknown> = { rateLimit: { remaining: 4000, resetAt: '2026-10-07T13:00:00Z' } }
    for (const [, i, owner, name, num] of query.matchAll(ALIAS)) {
      const url = `https://github.com/${owner}/${name}/pull/${num}`
      urls.push(url)
      const fixture = gh[url] as Record<string, unknown> | undefined
      data[`p${i}`] = fixture === undefined ? null : asGql(fixture, query)
    }
    queries.push(urls)

    return out(urls.every(url => url in gh) ? 0 : 1, JSON.stringify({ data }))
  })

  return { queries, files, merges, runs }
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
// the person's own text in it.
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
  const { runs } = fakeHost(on, clock, fixtures, undefined, undefined, undefined, runLog)
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

  return { ui: await $.ui.mount({ ...PANE, surface: 'terminal' }), clock, runs, fills, sent, box: () => box, type: (text: string) => (box = text) }
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
})
