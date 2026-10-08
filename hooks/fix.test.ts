import { describe, expect, test } from 'claude-code/testing'

import { checksQuery, parseChecks } from './batch'
import { LOG_CHARS, cleanLog, failingChecks, fixPrompt, logError, logProblems, toFetch, trimLog } from './fix'

const PR = {
  repo: 'acme/app',
  number: 13,
  title: 'Retry uploads with exponential backoff',
  url: 'https://github.com/acme/app/pull/13',
  branch: 'feat/retry',
  base: 'main',
}

const job = (name: string, id: number, conclusion: string, startedAt: string) => ({
  __typename: 'CheckRun',
  name,
  status: 'COMPLETED',
  conclusion,
  startedAt,
  databaseId: id,
  detailsUrl: `https://github.com/acme/app/actions/runs/1/job/${id}`,
  workflow: 'CI',
})

// What `gh run view --job <id> --log-failed` prints: job, step, then the timestamped line.
const ghLog = (lines: readonly string[], step = 'Run tests') =>
  lines.map((line, i) => `build\t${step}\t2026-10-07T11:${String(i % 60).padStart(2, '0')}:00.1234567Z ${line}`).join('\n')

describe('picking the failing checks', () => {
  test('the latest run of each: a re-run that passed does not count', async () => {
    const failures = failingChecks(
      [
        job('build', 901, 'FAILURE', '2026-10-07T11:00:00Z'),
        job('lint', 902, 'FAILURE', '2026-10-07T11:00:00Z'),
        job('lint', 903, 'SUCCESS', '2026-10-07T11:05:00Z'),
        { __typename: 'CheckRun', name: 'e2e', status: 'IN_PROGRESS', startedAt: '2026-10-07T11:00:00Z', workflow: 'CI' },
      ],
      'acme/app',
    )
    expect(failures.map(f => f.name)).toEqual(['build'])
    expect(failures[0]?.jobId).toBe(901)
  })

  test("another CI's check run or status has no job to fetch, only its link and summary", async () => {
    const failures = failingChecks(
      [
        { __typename: 'CheckRun', name: 'ci/external', status: 'COMPLETED', conclusion: 'TIMED_OUT', databaseId: 77, title: 'Timed out' },
        { __typename: 'StatusContext', context: 'ci/jenkins', state: 'ERROR', description: 'tests failed', targetUrl: 'https://ci.example.com/job/7' },
      ],
      'acme/app',
    )
    expect(failures).toEqual([
      { name: 'ci/external', workflow: null, conclusion: 'timed out', url: 'https://github.com/acme/app/runs/77', summary: 'Timed out', jobId: null },
      { name: 'ci/jenkins', workflow: null, conclusion: 'error', url: 'https://ci.example.com/job/7', summary: 'tests failed', jobId: null },
    ])
  })

  test('logs are fetched for the first three Actions jobs only', async () => {
    const failures = failingChecks([1, 2, 3, 4].map(n => job(`job${n}`, n, 'FAILURE', '2026-10-07T11:00:00Z')), 'acme/app')
    expect(toFetch(failures).map(f => f.jobId)).toEqual([1, 2, 3])
  })

  test('the checks query is the batch alias shape, and its reply parses the same way', async () => {
    const ref = { url: PR.url, repo: 'acme/app', number: 13 }
    expect(checksQuery(ref)).toContain('p0: repository(owner: "acme", name: "app") { pullRequest(number: 13)')
    expect(checksQuery(ref)).toContain('databaseId detailsUrl')
    expect(parseChecks(JSON.stringify({ data: { p0: null } }), '', ref)).toEqual({ error: "Not found, or this gh account can't see it" })
  })
})

describe('trimming a log', () => {
  test('drops the job and step columns, timestamps, colour codes and group markers', async () => {
    const raw = `﻿${ghLog(['##[group]Run npm test', '\u001b[31mFAIL\u001b[0m src/upload.test.ts', '##[endgroup]'])}\n`
    expect(cleanLog(raw)).toEqual({ steps: ['Run tests'], lines: ['Run npm test', 'FAIL src/upload.test.ts'] })
  })

  test('keeps the tail, where the error is, and says how much it dropped', async () => {
    const lines = Array.from({ length: 300 }, (_, i) => `line ${i}`)
    const log = trimLog(ghLog(lines))
    expect(log.kept).toBe(120)
    expect(log.total).toBe(300)
    expect(log.text.split('\n')[0]).toBe('line 180')
    expect(log.text.endsWith('line 299')).toBe(true)
  })

  test('a character budget cuts further, from the front', async () => {
    const lines = Array.from({ length: 50 }, (_, i) => `${String(i).padStart(2, '0')} ${'x'.repeat(97)}`)
    const log = trimLog(ghLog(lines), 120, 1_000)
    expect(log.kept).toBe(9)
    expect(log.text.endsWith(`49 ${'x'.repeat(97)}`)).toBe(true)
  })

  test("gh's refusal in plain words", async () => {
    const gone = 'failed to get run log: HTTP 410: Server Error (https://api.github.com/repos/acme/app/actions/runs/1/logs)\n'
    expect(logError(gone)).toBe('GitHub no longer keeps this log')
    expect(logError('')).toBe('gh run view failed')
  })
})

describe('the message for Claude', () => {
  const build = failingChecks([job('build', 901, 'FAILURE', '2026-10-07T11:00:00Z')], 'acme/app')[0]!
  const jenkins = failingChecks(
    [{ __typename: 'StatusContext', context: 'ci/jenkins', state: 'FAILURE', description: 'tests failed', targetUrl: 'https://ci.example.com/job/7' }],
    'acme/app',
  )[0]!

  test('names the PR and the checks, fences the trimmed log, and asks for a fix on the branch', async () => {
    const lines = [...Array.from({ length: 200 }, (_, i) => `setup ${i}`), 'Error: expected 3 retries, got 1']
    const text = fixPrompt(PR, [{ failure: build, log: ghLog(lines) }, { failure: jenkins }])

    expect(text).toContain('CI failed on acme/app#13: Retry uploads with exponential backoff')
    expect(text).toContain('https://github.com/acme/app/pull/13 · feat/retry → main')
    expect(text).toContain('- CI / build · failure · https://github.com/acme/app/actions/runs/1/job/901')
    expect(text).toContain('- ci/jenkins · failure · tests failed · https://ci.example.com/job/7')
    expect(text).toContain('build log (last 120 of 201 lines, trimmed, failed step "Run tests"):\n```\n')
    expect(text).toContain('Error: expected 3 retries, got 1\n```')
    expect(text).not.toContain('setup 80\n')
    expect(text).not.toContain('2026-10-07T11')
    expect(text).toContain("Find the cause and fix it on this branch (feat/retry). If acme/app or feat/retry isn't checked out here, say so first.")
    expect(text).not.toMatch(/\bpush\b|\bmerge\b/i)
  })

  test('every log together stays inside the cap', async () => {
    const big = ghLog(Array.from({ length: 120 }, (_, i) => `${i} ${'y'.repeat(300)}`))
    const failures = failingChecks([1, 2, 3].map(n => job(`job${n}`, n, 'FAILURE', '2026-10-07T11:00:00Z')), 'acme/app')
    const text = fixPrompt(PR, failures.map(failure => ({ failure, log: big })))
    const logs = [...text.matchAll(/```\n([\s\S]*?)\n```/g)].map(m => m[1] ?? '')

    expect(logs).toHaveLength(3)
    expect(logs.reduce((n, log) => n + log.length, 0)).toBeLessThanOrEqual(LOG_CHARS)
    expect(text).toContain('trimmed')
  })

  test('a log that would not come leaves the name and link, and says why', async () => {
    const handoffs = [{ failure: build, error: 'GitHub no longer keeps this log' }]
    const text = fixPrompt(PR, handoffs)

    expect(text).toContain('- CI / build · failure · https://github.com/acme/app/actions/runs/1/job/901')
    expect(text).toContain('build: no log (GitHub no longer keeps this log).')
    expect(logProblems(handoffs)).toBe('No log for build: GitHub no longer keeps this log')
  })

  test('a log with a fence in it gets a longer fence', async () => {
    const text = fixPrompt(PR, [{ failure: build, log: ghLog(['```', 'boom']) }])
    expect(text).toContain('````\n```\nboom\n````')
  })
})
