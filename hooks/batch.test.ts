import { describe, expect, test } from 'claude-code/testing'

import { buildQuery, chunks, parseReply } from './batch'
import { LATEST_FIELD } from './update'

const A = { url: 'https://github.com/acme/app/pull/7', repo: 'acme/app', number: 7 }
const B = { url: 'https://github.com/secret/app/pull/1', repo: 'secret/app', number: 1 }

describe('batched query', () => {
  test('one aliased field per PR, and the rate limit', async () => {
    const query = buildQuery([A, B])
    expect(query).toContain('p0: repository(owner: "acme", name: "app") { pullRequest(number: 7) { ...pr } ...repo }')
    expect(query).toContain('p1: repository(owner: "secret", name: "app")')
    expect(query).toContain('rateLimit { remaining resetAt }')
  })

  test('asks who reviewed and whether threads are resolved, never what anyone wrote', async () => {
    const query = buildQuery([A])
    for (const field of ['latestReviews(first: 10)', 'latestOpinionatedReviews(first: 10)', 'reviewRequests(first: 10)', 'reviewThreads(last: 50) { totalCount nodes { isResolved isOutdated } }']) {
      expect(query).toContain(field)
    }
    expect(query).not.toMatch(/\b(body|bodyText|comments)\b/)
  })

  test('the review facts come through as GitHub sent them', async () => {
    const reviewThreads = { totalCount: 1, nodes: [{ isResolved: false, isOutdated: true }] }
    const latestReviews = { totalCount: 1, nodes: [{ author: { __typename: 'User', login: 'nadav' }, state: 'COMMENTED', submittedAt: '2026-10-07T10:00:00Z' }] }
    const stdout = JSON.stringify({ data: { p0: { pullRequest: { number: 7, latestReviews, reviewThreads, commits: { nodes: [] } } } } })
    const pr = parseReply(stdout, '', [A]).found.get(A.url)
    expect(pr?.latestReviews).toEqual(latestReviews)
    expect(pr?.reviewThreads).toEqual(reviewThreads)
  })

  test('a partial reply: found PRs flattened like gh pr view, the rest missing', async () => {
    // What `gh api graphql` prints (exit 1) when one alias is NOT_FOUND.
    const stdout = JSON.stringify({
      data: {
        rateLimit: { remaining: 4961, resetAt: '2026-10-07T11:01:16Z' },
        p0: {
          squashMergeAllowed: true,
          viewerPermission: 'WRITE',
          pullRequest: {
            number: 7,
            state: 'OPEN',
            commits: { nodes: [{ commit: { statusCheckRollup: { contexts: { nodes: [{ __typename: 'CheckRun', name: 'build', status: 'IN_PROGRESS' }] } } } }] },
          },
        },
        p1: null,
      },
      errors: [{ type: 'NOT_FOUND', path: ['p1'], message: "Could not resolve to a Repository with the name 'secret/app'." }],
    })
    const reply = parseReply(stdout, 'gh: Could not resolve…', [A, B])
    expect(reply.error).toBe(null)
    expect(reply.found.get(A.url)?.statusCheckRollup).toEqual([{ __typename: 'CheckRun', name: 'build', status: 'IN_PROGRESS' }])
    expect(reply.found.get(A.url)?.repository).toEqual({ squashMergeAllowed: true, viewerPermission: 'WRITE' })
    expect(reply.missing).toEqual([B])
    expect(reply.rate?.remaining).toBe(4961)
  })

  test('a failed call knows nothing, and says why', async () => {
    const reply = parseReply('', 'error connecting to api.github.com\n', [A])
    expect(reply.error).toBe('error connecting to api.github.com')
    expect(reply.missing).toEqual([])
  })

  test("pr-watch's newest version is asked only when wanted, and once", async () => {
    expect(buildQuery([A, B])).not.toContain('pw:')
    const query = buildQuery([A, B], true)
    expect(query).toContain(LATEST_FIELD)
    expect(query.split('pw: repository(').length).toBe(2)
    expect(LATEST_FIELD).toContain('object(expression: "HEAD:.claude-plugin/plugin.json") { ... on Blob { text } }')
  })

  test('the newest version comes from the pw field', async () => {
    const pw = { object: { text: JSON.stringify({ name: 'pr-watch', version: '0.9.0' }) } }
    const stdout = JSON.stringify({ data: { pw, p0: { pullRequest: { number: 7, commits: { nodes: [] } } } } })
    const reply = parseReply(stdout, '', [A])
    expect(reply.latest).toBe('0.9.0')
    expect(reply.found.has(A.url)).toBe(true)
  })

  test("a pw GitHub can't answer leaves the PRs as they were", async () => {
    // What `gh api graphql` prints (exit 1) when the repo is renamed, gone or private.
    const stdout = JSON.stringify({
      data: { pw: null, p0: { pullRequest: { number: 7, state: 'OPEN', commits: { nodes: [] } } } },
      errors: [{ type: 'NOT_FOUND', path: ['pw'], message: "Could not resolve to a Repository with the name 'ElirazKed/claude-code-pr-watch'." }],
    })
    const reply = parseReply(stdout, 'gh: Could not resolve…', [A])
    expect(reply.error).toBe(null)
    expect(reply.latest).toBe(null)
    expect(reply.found.get(A.url)?.state).toBe('OPEN')
    expect(reply.missing).toEqual([])
    // Nor does a manifest that won't parse.
    const garbled = JSON.stringify({ data: { pw: { object: { text: '{' } }, p0: { pullRequest: { number: 7, commits: { nodes: [] } } } } })
    expect(parseReply(garbled, '', [A]).latest).toBe(null)
    expect(parseReply(garbled, '', [A]).found.has(A.url)).toBe(true)
  })

  test('splits into batches', async () => {
    expect(chunks([1, 2, 3, 4, 5], 2)).toEqual([[1, 2], [3, 4], [5]])
    // 20 a query by default: 45 PRs make three queries, none of them slow enough to time out.
    expect(chunks(Array.from({ length: 45 }, (_, i) => i)).map(part => part.length)).toEqual([20, 20, 5])
  })
})
