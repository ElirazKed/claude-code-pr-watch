import { describe, expect, test } from 'claude-code/testing'

import { buildQuery, chunks, parseReply } from './batch'

const A = { url: 'https://github.com/acme/app/pull/7', repo: 'acme/app', number: 7 }
const B = { url: 'https://github.com/secret/app/pull/1', repo: 'secret/app', number: 1 }

describe('batched query', () => {
  test('one aliased field per PR, and the rate limit', async () => {
    const query = buildQuery([A, B])
    expect(query).toContain('p0: repository(owner: "acme", name: "app") { pullRequest(number: 7) { ...pr } ...repo }')
    expect(query).toContain('p1: repository(owner: "secret", name: "app")')
    expect(query).toContain('rateLimit { remaining resetAt }')
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

  test('splits into batches', async () => {
    expect(chunks([1, 2, 3, 4, 5], 2)).toEqual([[1, 2], [3, 4], [5]])
  })
})
