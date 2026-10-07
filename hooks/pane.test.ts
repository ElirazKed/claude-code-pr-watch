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

// gh's GraphQL shape for a fixture, checks nested under the head commit.
const asGql = (fixture: Record<string, unknown>) => {
  const { statusCheckRollup, ...rest } = fixture

  return { ...rest, closedAt: null, commits: { nodes: [{ commit: { statusCheckRollup: { contexts: { nodes: statusCheckRollup } } } }] } }
}

const ALIAS = /p(\d+): repository\(owner: "([^"]+)", name: "([^"]+)"\) \{ pullRequest\(number: (\d+)\)/g

// Stands for gh, $HOME and the file system: answers batched queries from `gh`, keeps files in
// memory (stamped with the mocked time, so mtimes are heartbeats), and records each gh argv.
function fakeHost(on: On, clock: Clock, gh: Record<string, unknown>, files = new Map<string, { text: string; mtimeMs: number }>()) {
  const queries: string[][] = []
  on('env.get', () => ({ value: '/home/me' }))
  on('fs.read', ($, e) => ({ value: files.get(e.path)?.text ?? '' }))
  on('fs.write', ($, e) => (files.set(e.path, { text: e.text, mtimeMs: clock.now() }), { value: undefined }))
  on('fs.list', ($, e) => ({
    value: [...files]
      .filter(([path]) => path.startsWith(`${e.path}/`) && !path.slice(e.path.length + 1).includes('/'))
      .map(([path, f]) => ({ name: path.slice(e.path.length + 1), kind: 'file' as const, size: f.text.length, mtimeMs: f.mtimeMs, isLink: false })),
  }))
  on('process.run', ($, e) => {
    const out = (exitCode: number, stdout: string) => ({
      value: { exitCode, stdout, stderr: '', isStdoutTruncated: false, isStderrTruncated: false },
    })
    if (e.argv[1] === 'auth') return out(0, '  ✓ Logged in to github.com account me (keyring)\n  - Active account: true\n')
    if (e.argv[1] !== 'api') return out(1, '')
    const query = String(e.argv[4] ?? '').slice('query='.length)
    const urls: string[] = []
    const data: Record<string, unknown> = { rateLimit: { remaining: 4000, resetAt: '2026-10-07T13:00:00Z' } }
    for (const [, i, owner, name, num] of query.matchAll(ALIAS)) {
      const url = `https://github.com/${owner}/${name}/pull/${num}`
      urls.push(url)
      const fixture = gh[url] as Record<string, unknown> | undefined
      data[`p${i}`] = fixture === undefined ? null : { pullRequest: asGql(fixture) }
    }
    queries.push(urls)

    return out(urls.every(url => url in gh) ? 0 : 1, JSON.stringify({ data }))
  })

  return { queries, files }
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

test('a merged PR a gh command touched is never added, however recently it merged', async ($, on) => {
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

  await $.tool.call({ tool: 'Bash', command: `gh pr view ${OLD}`, description: 'view' } as never)
  await clock.advance(100)

  expect(opens).toBe(0)
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  expect(await ui.find({ text: /#3/ })).toBeUndefined()
  await ui.unmount()
})
