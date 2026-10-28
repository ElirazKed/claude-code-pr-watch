import { describe, expect, test } from 'claude-code/testing'

import type { TrackedPr } from '../types'
import { PILL, placeholder } from './pr'
import { approvalsHeading, approvalsOf, approvalsQuery, checkRows, gateNote, parseApprovals, parseTeams, ruledTeamIds, seenOf, stillNeeded, tabLabel, threadRows } from './tabs'
import type { GhApprovals } from './tabs'

const PR: TrackedPr = {
  ...placeholder({ url: 'https://github.com/acme/app/pull/12', repo: 'acme/app', number: 12 }),
  title: 'Retry uploads with exponential backoff',
  state: 'OPEN',
  stage: 'review',
  tone: 'warning',
  pill: PILL.inReview,
  headline: 'Waiting for review',
  checks: { passed: 3, failed: 0, pending: 0, total: 3, failing: [], running: [], queued: 0 },
  branch: 'feat/retry',
  base: 'main',
  author: 'octocat',
  reviews: { reviewers: [{ login: 'alice', verdict: 'approved' }, { login: 'bob', verdict: 'requested' }], more: 0, unresolved: 2, outdated: 0, isCapped: false },
  checkedAt: 0,
}

const run = (name: string, status: string, conclusion = '', workflow = 'CI') => ({ __typename: 'CheckRun', name, status, conclusion, workflow })

describe('the tab strip', () => {
  test('each tab counts what it holds; one with nothing to count is its name alone', async () => {
    expect(tabLabel('overview', PR)).toEqual({ name: 'Overview', count: '' })
    expect(tabLabel('checks', PR)).toEqual({ name: 'Checks', count: '3/3' })
    expect(tabLabel('checks', { ...PR, checks: { ...PR.checks, total: 0, passed: 0 } }).count).toBe('')
    expect(tabLabel('reviews', PR)).toEqual({ name: 'Reviews', count: '2' })
    expect(tabLabel('threads', PR)).toEqual({ name: 'Threads', count: '2' })
    expect(tabLabel('threads', { ...PR, reviews: { ...PR.reviews!, unresolved: 8, isCapped: true } }).count).toBe('8+')
    expect(tabLabel('threads', { ...PR, reviews: null }).count).toBe('')
  })

  test("once the rules are read, Reviews counts the approvals they ask for, never past them", async () => {
    const approvals = { required: 2, approved: 1, isOwnersRequired: false, gates: [] }
    expect(tabLabel('reviews', PR, approvals).count).toBe('1/2')
    expect(tabLabel('reviews', PR, { ...approvals, approved: 3 }).count).toBe('2/2')
    // Rules that ask for none: the reviewers again.
    expect(tabLabel('reviews', PR, { ...approvals, required: 0 }).count).toBe('2')
  })

  test('a tab reads again only when the round changed what it shows', async () => {
    expect(seenOf('checks', PR)).toBe(seenOf('checks', { ...PR, headline: 'Ready to merge' }))
    expect(seenOf('checks', PR)).not.toBe(seenOf('checks', { ...PR, checks: { ...PR.checks, passed: 2, pending: 1 } }))
    expect(seenOf('reviews', PR)).not.toBe(seenOf('reviews', { ...PR, reviewSeen: 5 }))
    expect(seenOf('reviews', PR)).not.toBe(seenOf('reviews', { ...PR, base: 'release' }))
    expect(seenOf('threads', PR)).toBe(seenOf('threads', { ...PR, checks: { ...PR.checks, passed: 1 } }))
    expect(seenOf('threads', PR)).not.toBe(seenOf('threads', { ...PR, reviews: { ...PR.reviews!, unresolved: 1 } }))
  })
})

describe('the Checks tab', () => {
  test('the latest run of each, failing first, then running, queued, passed and skipped', async () => {
    const rows = checkRows([
      run('lint', 'COMPLETED', 'SUCCESS'),
      run('docs', 'COMPLETED', 'SKIPPED'),
      run('e2e', 'QUEUED'),
      run('build', 'IN_PROGRESS'),
      run('unit', 'COMPLETED', 'FAILURE'),
      { __typename: 'StatusContext', context: 'ci/jenkins', state: 'PENDING' },
      { __typename: 'StatusContext', context: 'deploy', state: 'ERROR' },
    ])
    expect(rows.map(row => [row.name, row.state])).toEqual([
      ['unit', 'failed'],
      ['deploy', 'failed'],
      ['build', 'running'],
      ['ci/jenkins', 'running'],
      ['e2e', 'queued'],
      ['lint', 'passed'],
      ['docs', 'skipped'],
    ])
    expect(rows[0]?.workflow).toBe('CI')
    expect(rows[1]?.workflow).toBeNull()
  })

  test('a check run again shows its latest run alone', async () => {
    const rows = checkRows([
      { ...run('build', 'COMPLETED', 'FAILURE'), startedAt: '2026-10-07T10:00:00Z' },
      { ...run('build', 'COMPLETED', 'SUCCESS'), startedAt: '2026-10-07T11:00:00Z' },
    ])
    expect(rows).toEqual([{ name: 'build', workflow: 'CI', state: 'passed' }])
  })
})

describe('the Threads tab', () => {
  const thread = (path: string, line: number | null, bodies: [string, string][], extra: Record<string, unknown> = {}) => ({
    isResolved: false,
    isOutdated: false,
    path,
    line,
    startLine: null,
    originalLine: line,
    originalStartLine: null,
    comments: { totalCount: bodies.length, nodes: bodies.map(([login, body]) => ({ author: { login }, body, url: '' })) },
    ...extra,
  })

  test('the open ones by file and line: who opened it, their first line, and how many replied', async () => {
    const rows = threadRows([
      thread('src/upload.ts', 42, [['nadav', '<!-- bot -->\n\nTake the retry count\nfrom config.'], ['octocat', 'Will do.']]),
      thread('src/api.ts', 9, [['carol', 'Fixed already.']], { isResolved: true }),
      thread('src/api.ts', null, [['carol', 'Still needed?']], { isOutdated: true, originalLine: 7 }),
    ])
    expect(rows).toEqual([
      { spot: 'src/api.ts (outdated; was line 7)', author: 'carol', text: 'Still needed?', replies: 0, isOutdated: true },
      { spot: 'src/upload.ts:42', author: 'nadav', text: 'Take the retry count', replies: 1, isOutdated: false },
    ])
  })
})

describe('the Reviews tab', () => {
  const approve = (login: string, teams: string[] = [], state = 'APPROVED') => ({
    state,
    author: { __typename: 'User', login },
    onBehalfOf: { nodes: teams.map(slug => ({ slug })) },
  })
  const ruleset = (parameters: Record<string, unknown>) => ({ type: 'PULL_REQUEST', parameters })

  test('asks for the rules, the code owners asked, and the teams each review counted for', async () => {
    const query = approvalsQuery({ url: 'https://github.com/acme/app/pull/12', repo: 'acme/app', number: 12 })
    expect(query).toContain('p0: repository(owner: "acme", name: "app") { pullRequest(number: 12)')
    for (const field of ['requiredApprovingReviewCount', 'requireCodeOwnerReview', 'requiredReviewers', 'branchProtectionRule', 'asCodeOwner', 'onBehalfOf']) {
      expect(query).toContain(field)
    }
  })

  test('code-owner teams: met by a member, waiting while asked; the approvals counted against the rule', async () => {
    const pr: GhApprovals = {
      author: { login: 'octocat' },
      baseRef: { rules: { nodes: [{ type: 'REQUIRED_STATUS_CHECKS', parameters: {} }, ruleset({ requiredApprovingReviewCount: 3, requireCodeOwnerReview: true })] } },
      reviewRequests: {
        nodes: [
          { asCodeOwner: true, requestedReviewer: { __typename: 'Team', slug: 'security' } },
          { asCodeOwner: true, requestedReviewer: { __typename: 'User', login: 'dana' } },
          // Asked by hand, not by CODEOWNERS: no gate of its own.
          { asCodeOwner: false, requestedReviewer: { __typename: 'User', login: 'erin' } },
        ],
      },
      latestOpinionatedReviews: { nodes: [approve('alice', ['backend']), approve('octocat', ['backend'])] },
    }
    const approvals = approvalsOf(pr, 'acme', new Map())
    expect(approvals).toEqual({
      required: 3,
      approved: 1,
      isOwnersRequired: true,
      gates: [
        { name: '@acme/backend', state: 'approved', by: ['alice'], need: 1, patterns: [] },
        { name: '@acme/security', state: 'waiting', by: [], need: 1, patterns: [] },
        { name: '@dana', state: 'waiting', by: [], need: 1, patterns: [] },
      ],
    })
    expect(approvalsHeading(approvals)).toBe('Required · 1 of 3 approvals · code owners')
    expect(stillNeeded(approvals)).toBe('2 more approvals')
    expect(approvals.gates.map(gateNote)).toEqual(['alice', 'waiting', 'waiting'])
  })

  test('a change request on behalf of a team holds its gate, whoever else approved', async () => {
    const pr: GhApprovals = {
      latestOpinionatedReviews: { nodes: [approve('alice', ['backend']), approve('bob', ['backend'], 'CHANGES_REQUESTED')] },
    }
    const [gate] = approvalsOf(pr, 'acme', new Map()).gates
    expect(gate?.state).toBe('changes')
    expect(gateNote(gate!)).toBe('changes requested')
  })

  test("a ruleset's teams: met at their minimum, else waiting, else only maybe, by their files", async () => {
    const pr: GhApprovals = {
      baseRef: {
        rules: {
          nodes: [
            ruleset({
              requiredApprovingReviewCount: 1,
              requiredReviewers: [
                { reviewerId: 'T_platform', minimumApprovals: 2, filePatterns: ['infra/**'] },
                { reviewerId: 'T_docs', minimumApprovals: 1, filePatterns: ['docs/**', '*.md', 'README*'] },
                { reviewerId: 'T_ui', minimumApprovals: 1, filePatterns: ['web/**'] },
                // No approval asked: nothing to wait on.
                { reviewerId: 'T_fyi', minimumApprovals: 0, filePatterns: ['**'] },
                // Not a team this login can see.
                { reviewerId: 'T_hidden', minimumApprovals: 1, filePatterns: ['**'] },
              ],
            }),
          ],
        },
        branchProtectionRule: null,
      },
      latestOpinionatedReviews: { nodes: [approve('alice', ['platform']), approve('carol', ['ui'])] },
    }
    expect(ruledTeamIds(pr)).toEqual(['T_platform', 'T_docs', 'T_ui', 'T_fyi', 'T_hidden'])
    const teams = new Map([['T_platform', 'platform'], ['T_docs', 'docs'], ['T_ui', 'ui'], ['T_fyi', 'fyi']])
    const approvals = approvalsOf(pr, 'acme', teams)
    expect(approvals.gates.map(gate => [gate.name, gate.state, gateNote(gate)])).toEqual([
      ['@acme/ui', 'approved', 'carol'],
      ['@acme/platform', 'waiting', '1 of 2 · waiting'],
      ['@acme/docs', 'maybe', 'if it changes docs/**, *.md +1'],
    ])
    expect(approvalsHeading(approvals)).toBe('Required · 1 of 1 approval')
    expect(stillNeeded(approvals)).toBe('')
  })

  test('classic branch protection counts too, the stricter of it and the rulesets', async () => {
    const pr: GhApprovals = {
      baseRef: { rules: { nodes: [ruleset({ requiredApprovingReviewCount: 1 })] }, branchProtectionRule: { requiredApprovingReviewCount: 2, requiresCodeOwnerReviews: true } },
    }
    expect(approvalsOf(pr, 'acme', new Map())).toMatchObject({ required: 2, isOwnersRequired: true })
  })

  test('rules that ask for nothing say so', async () => {
    const approvals = approvalsOf({ baseRef: { rules: { nodes: [] }, branchProtectionRule: null } }, 'acme', new Map())
    expect(approvalsHeading(approvals)).toBe('No approvals required by the branch rules')
    expect(stillNeeded(approvals)).toBe('')
  })

  test("what gh printed: the PR, even beside a field this login can't read; or why there is none", async () => {
    const partial = JSON.stringify({ data: { p0: { pullRequest: { author: { login: 'octocat' }, baseRef: { branchProtectionRule: null } } } }, errors: [{ message: 'Resource not accessible by integration' }] })
    expect(parseApprovals(partial, '')).toEqual({ pr: { author: { login: 'octocat' }, baseRef: { branchProtectionRule: null } } })
    expect(parseApprovals(JSON.stringify({ data: { p0: null } }), '')).toEqual({ error: "Not found, or this gh account can't see it" })
    expect(parseApprovals('', 'HTTP 502: Bad Gateway\nmore')).toEqual({ error: 'HTTP 502: Bad Gateway' })
    expect(parseApprovals(JSON.stringify({ errors: [{ message: 'Bad credentials' }] }), '')).toEqual({ error: 'Bad credentials' })
  })

  test("teams by node id, leaving out what isn't one", async () => {
    const stdout = JSON.stringify({ data: { teams: [{ __typename: 'Team', id: 'T_1', slug: 'backend' }, null, { __typename: 'User' }] } })
    expect([...parseTeams(stdout)]).toEqual([['T_1', 'backend']])
    expect(parseTeams('not json').size).toBe(0)
  })
})
