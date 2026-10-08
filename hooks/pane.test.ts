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

// gh's GraphQL shape for a fixture: the repository alias with its merge settings, and the PR
// in it, checks nested under the head commit.
const asGql = (fixture: Record<string, unknown>) => {
  const { statusCheckRollup, repository, ...rest } = fixture
  const commits = { nodes: [{ commit: { statusCheckRollup: { contexts: { nodes: statusCheckRollup } } } }] }

  return { ...(repository as object | undefined), pullRequest: { ...rest, closedAt: null, commits } }
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
// ends; by default it goes through, and `merges` records each one's argv. `runLog`: what
// `gh run view` prints, a failed job's log by default; `runs` records each argv.
function fakeHost(
  on: On,
  clock: Clock,
  gh: Record<string, unknown>,
  files = new Map<string, { text: string; mtimeMs: number }>(),
  branchPr?: string,
  merge: (argv: readonly string[]) => MergeReply = () => ({ exitCode: 0 }),
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
      const reply = merge(e.argv)
      if (reply.exitCode === 0) merged(gh, e.argv)

      return out(reply.exitCode, '', reply.stderr)
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
      data[`p${i}`] = fixture === undefined ? null : asGql(fixture)
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
  autoMergeAllowed: true,
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
async function mergeCard($: Parameters<TestBody>[0], on: On, fixture: Record<string, unknown>, merge?: (argv: readonly string[]) => MergeReply) {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  quietUi(on)
  const { merges } = fakeHost(on, clock, { [READY]: fixture }, undefined, undefined, merge)
  await $.command.run({ command: 'pr-watch', args: READY, origin: { kind: 'composer' }, presentation: { isFullscreen: true, columns: 180 } })
  await clock.advance(100)

  return { ui: await $.ui.mount({ ...PANE, surface: 'terminal' }), merges }
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

// Watches `fixtures` and draws the pane, with a prompt box that takes drafts unless `fill`
// says otherwise; `fills` and `sent` are what reached the box and the session.
async function fixCard(
  $: Parameters<TestBody>[0],
  on: On,
  fixtures: Record<string, unknown>,
  runLog?: (argv: readonly string[]) => RunReply | Promise<RunReply>,
  fill: () => { isFilled: boolean } = () => ({ isFilled: true }),
) {
  const clock = mock.clock(on, { now: Date.parse('2026-10-07T12:00:00Z') })
  quietUi(on)
  const fills: string[] = []
  const sent: string[] = []
  const { runs } = fakeHost(on, clock, fixtures, undefined, undefined, undefined, runLog)
  on('prompt.read', () => ({ value: { text: '', cursor: 0 } }))
  on('prompt.fill', ($, e) => (fills.push(e.text), fill()))
  on('prompt.submit', ($, e) => (sent.push(e.text), { text: e.text }))
  await $.command.run({
    command: 'pr-watch',
    args: Object.keys(fixtures).join(' '),
    origin: { kind: 'composer' },
    presentation: { isFullscreen: true, columns: 180 },
  })
  await clock.advance(100)

  return { ui: await $.ui.mount({ ...PANE, surface: 'terminal' }), clock, runs, fills, sent }
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
