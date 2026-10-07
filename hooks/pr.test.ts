import { describe, expect, test } from 'claude-code/testing'

import { cwdOf, derive, findPrUrls, refFromGhCommand, stepIndex, suggestable } from './pr'
import type { GhPr } from './pr'

const base: GhPr = {
  number: 7,
  title: 'Add thing',
  url: 'https://github.com/acme/app/pull/7',
  state: 'OPEN',
  isDraft: false,
  reviewDecision: null,
  mergeStateStatus: 'CLEAN',
  autoMergeRequest: null,
  statusCheckRollup: [],
  mergedAt: null,
  additions: 1,
  deletions: 0,
  headRefName: 'feat',
  baseRefName: 'main',
  author: { login: 'me' },
}

const run = (name: string, status: string, conclusion = '') => ({ __typename: 'CheckRun', name, status, conclusion })

describe('detecting PRs', () => {
  test('finds and dedupes PR urls in gh output', async () => {
    const refs = findPrUrls('created https://github.com/acme/app/pull/7\nsee https://github.com/acme/app/pull/7/files')
    expect(refs).toEqual([{ url: 'https://github.com/acme/app/pull/7', repo: 'acme/app', number: 7 }])
  })

  test('ignores the "create a pull request" link git push prints', async () => {
    expect(findPrUrls('remote: https://github.com/acme/app/pull/new/my-branch')).toEqual([])
  })

  test('builds a ref from a -R flag and number', async () => {
    expect(refFromGhCommand('gh pr checks 42 -R acme/app')?.url).toBe('https://github.com/acme/app/pull/42')
    expect(refFromGhCommand('gh pr checks 42')).toBe(null)
  })

  test('reads the cd prefix', async () => {
    expect(cwdOf('cd ~/src/app && gh pr checks')).toBe('~/src/app')
    expect(cwdOf('gh pr checks')).toBe(undefined)
  })
})

describe('lifecycle', () => {
  test('running CI is the CI step, warning', async () => {
    const d = derive({ ...base, statusCheckRollup: [run('build', 'IN_PROGRESS'), run('lint', 'COMPLETED', 'SUCCESS')] })
    expect(d.stage).toBe('checks')
    expect(d.headline).toBe('Running build · 1/2 passed')
    expect(d.isMoving).toBe(true)
    expect(stepIndex(d.stage)).toBe(1)
  })

  test('a failed check wins, but checks still running keep the card live', async () => {
    const d = derive({ ...base, statusCheckRollup: [run('build', 'COMPLETED', 'FAILURE'), run('e2e', 'QUEUED')] })
    expect(d.tone).toBe('error')
    expect(d.pill).toBe('✗ CI FAILING')
    expect(d.headline).toBe('Failing: build · 1 still running')
    expect(d.isMoving).toBe(true)
  })

  test('a failed check with nothing left running has settled', async () => {
    const d = derive({ ...base, statusCheckRollup: [run('build', 'COMPLETED', 'FAILURE'), run('lint', 'COMPLETED', 'SUCCESS')] })
    expect(d.pill).toBe('✗ CI FAILED')
    expect(d.headline).toBe('Failing: build')
    expect(d.isMoving).toBe(false)
  })

  test('names the running checks and counts the queued ones', async () => {
    const d = derive({
      ...base,
      statusCheckRollup: [
        run('build', 'IN_PROGRESS'),
        run('e2e', 'IN_PROGRESS'),
        run('lint', 'IN_PROGRESS'),
        run('deploy', 'QUEUED'),
        run('docs', 'WAITING'),
        run('types', 'COMPLETED', 'SUCCESS'),
      ],
    })
    expect(d.pill).toBe('● CI RUNNING')
    expect(d.headline).toBe('Running build, e2e +1 · 2 queued · 1/6 passed')
  })

  test('only queued checks say so', async () => {
    const d = derive({ ...base, statusCheckRollup: [run('build', 'QUEUED')] })
    expect(d.headline).toBe('1 queued · 0/1 passed')
  })

  test('conflicts outrank failing CI, which stays in the line', async () => {
    const d = derive({ ...base, mergeStateStatus: 'DIRTY', statusCheckRollup: [run('lint-docs', 'COMPLETED', 'FAILURE'), run('lint', 'COMPLETED', 'SUCCESS')] })
    expect(d.pill).toBe('⚠ CONFLICTS')
    expect(d.tone).toBe('error')
    expect(d.headline).toBe('Merge conflicts with base · failing: lint-docs')
    expect(d.stage).toBe('checks')
  })

  test('conflicts outrank running CI and pending review, and stay live while checks run', async () => {
    const d = derive({ ...base, mergeStateStatus: 'DIRTY', reviewDecision: 'REVIEW_REQUIRED', statusCheckRollup: [run('build', 'IN_PROGRESS')] })
    expect(d.pill).toBe('⚠ CONFLICTS')
    expect(d.headline).toBe('Merge conflicts with base · Running build')
    expect(d.isMoving).toBe(true)
  })

  test('mergeable CONFLICTING counts while mergeStateStatus is still being computed', async () => {
    const d = derive({ ...base, mergeStateStatus: 'UNKNOWN', mergeable: 'CONFLICTING', reviewDecision: 'REVIEW_REQUIRED' })
    expect(d.pill).toBe('⚠ CONFLICTS')
    expect(d.headline).toBe('Merge conflicts with base')
    expect(d.stage).toBe('ready')
  })

  test('a conflicted draft says so', async () => {
    expect(derive({ ...base, isDraft: true, mergeStateStatus: 'DIRTY' }).headline).toBe('Draft · merge conflicts')
  })

  test('a check re-run on the same commit counts by its latest run, as the PR page does', async () => {
    const at = (name: string, conclusion: string, startedAt: string) => ({ ...run(name, 'COMPLETED', conclusion), workflow: 'Docs Check', startedAt })
    const d = derive({
      ...base,
      statusCheckRollup: [
        at('lint-docs', 'FAILURE', '2026-10-07T13:55:50Z'),
        at('lint-docs', 'SUCCESS', '2026-10-07T14:07:48Z'),
        at('lint-docs', 'SUCCESS', '2026-10-07T14:00:32Z'),
        run('lint', 'COMPLETED', 'SUCCESS'),
      ],
    })
    expect(d.pill).not.toBe('✗ CI FAILED')
    expect(d.checks).toMatchObject({ passed: 2, failed: 0, total: 2 })
  })

  test('a queued re-run after a failure is what counts: CI is running again', async () => {
    const d = derive({
      ...base,
      statusCheckRollup: [
        { ...run('e2e', 'COMPLETED', 'FAILURE'), startedAt: '2026-10-07T13:00:00Z' },
        { ...run('e2e', 'QUEUED'), startedAt: null },
      ],
    })
    expect(d.pill).toBe('● CI RUNNING')
    expect(d.checks.total).toBe(1)
  })

  test("two workflows' same-named jobs are separate checks", async () => {
    const d = derive({
      ...base,
      statusCheckRollup: [
        { ...run('test', 'COMPLETED', 'SUCCESS'), workflow: 'Unit' },
        { ...run('test', 'COMPLETED', 'FAILURE'), workflow: 'E2E' },
      ],
    })
    expect(d.checks).toMatchObject({ passed: 1, failed: 1, total: 2 })
  })

  test('a status context counts by its latest report', async () => {
    const d = derive({
      ...base,
      statusCheckRollup: [
        { __typename: 'StatusContext', context: 'ci/jenkins', state: 'FAILURE', createdAt: '2026-10-07T10:00:00Z' },
        { __typename: 'StatusContext', context: 'ci/jenkins', state: 'SUCCESS', createdAt: '2026-10-07T11:00:00Z' },
      ],
    })
    expect(d.checks).toMatchObject({ passed: 1, failed: 0, total: 1 })
  })

  test('green CI waiting on approval', async () => {
    const d = derive({ ...base, reviewDecision: 'REVIEW_REQUIRED', statusCheckRollup: [run('build', 'COMPLETED', 'SUCCESS')] })
    expect(d.stage).toBe('review')
    expect(d.headline).toBe('Waiting for review')
  })

  test('approved and clean is ready', async () => {
    const d = derive({ ...base, reviewDecision: 'APPROVED' })
    expect(d.stage).toBe('ready')
    expect(d.tone).toBe('success')
  })

  test('status contexts count too', async () => {
    const d = derive({ ...base, statusCheckRollup: [{ __typename: 'StatusContext', context: 'ci/jenkins', state: 'PENDING' }] })
    expect(d.headline).toBe('Running ci/jenkins · 0/1 passed')
  })

  test('merged completes every step', async () => {
    const d = derive({ ...base, state: 'MERGED' })
    expect(d.headline).toBe('Merged')
    expect(stepIndex(d.stage)).toBe(4)
  })
})

describe('suggesting PRs Claude touched', () => {
  const url = 'https://github.com/acme/app/pull/9'

  test('a PR link Claude fetched', async () => {
    expect(suggestable('WebFetch', { url, prompt: 'status?' }, '').map(r => r.url)).toEqual([url])
  })

  test('a GitHub MCP pull request call by owner/repo/number', async () => {
    const refs = suggestable('mcp__github__get_pull_request', { owner: 'acme', repo: 'app', pull_number: 9 }, '')
    expect(refs.map(r => r.url)).toEqual([url])
  })

  test('the PR a git push updated', async () => {
    expect(suggestable('Bash', { command: 'git push' }, `remote: ${url}`).map(r => r.url)).toEqual([url])
  })

  test('not PR links that merely sit in a file Claude read', async () => {
    expect(suggestable('Read', { file_path: '/repo/CHANGELOG.md' }, `fixed in ${url}`)).toEqual([])
    expect(suggestable('Bash', { command: 'cat CHANGELOG.md' }, `fixed in ${url}`)).toEqual([])
  })
})
