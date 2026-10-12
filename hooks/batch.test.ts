import { describe, expect, test } from 'claude-code/testing'

import { LOG_LINES, MIN_CALL_MS, appendLog, buildQuery, callBudget, chunks, failureOf, logLine, parseHttp, parseReply, rateOf, retryAfterOf } from './batch'
import { LATEST_FIELD } from './update'

const A = { url: 'https://github.com/acme/app/pull/7', repo: 'acme/app', number: 7 }
const B = { url: 'https://github.com/secret/app/pull/1', repo: 'secret/app', number: 1 }

describe('batched query', () => {
  test('one aliased field per PR, and the rate limit', async () => {
    const query = buildQuery([A, B])
    expect(query).toContain('p0: repository(owner: "acme", name: "app") { pullRequest(number: 7) { ...pr } ...repo }')
    expect(query).toContain('p1: repository(owner: "secret", name: "app")')
    expect(query).toContain('rateLimit { limit remaining resetAt }')
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

// `gh api graphql -i` on stdout, as gh prints it: the status line ends "\n", each header
// "\r\n", then a blank line and the body.
const head = (status: string, headers: string[]) => `HTTP/2.0 ${status}\n${headers.map(h => `${h}\r\n`).join('')}\r\n`
const LIMITS = ['X-Ratelimit-Limit: 5000', 'X-Ratelimit-Remaining: 4998', 'X-Ratelimit-Reset: 1791378000', 'X-Ratelimit-Resource: graphql', 'X-Ratelimit-Used: 2']
const SECONDARY = '{"message":"You have exceeded a secondary rate limit. Please wait a few minutes before you try again.","documentation_url":"https://docs.github.com"}'
const NO_HTTP = { status: null, headers: {}, body: '' }

describe('what GitHub said about a call', () => {
  test('a 200: its status, headers by lower-case name, and the body', async () => {
    const body = JSON.stringify({ data: { p0: { pullRequest: { number: 7, commits: { nodes: [] } } } } })
    const http = parseHttp(head('200 OK', ['Content-Type: application/json; charset=utf-8', ...LIMITS]) + body)
    expect(http.status).toBe(200)
    expect(http.headers['x-ratelimit-remaining']).toBe('4998')
    expect(http.headers['content-type']).toBe('application/json; charset=utf-8')
    expect(http.body).toBe(body)
    expect(parseReply(http.body, '', [A]).found.has(A.url)).toBe(true)
    expect(rateOf(http.headers)).toEqual({ limit: 5000, remaining: 4998, resetAt: new Date(1791378000 * 1_000).toISOString() })
  })

  test("the rate headers count only where they speak of GraphQL's points", async () => {
    const reset = 'X-Ratelimit-Reset: 1791378000'
    const of = (lines: string[]) => rateOf(parseHttp(head('200 OK', lines)).headers)
    // No limit named: the caller takes GitHub's usual one.
    expect(of(['X-Ratelimit-Remaining: 12', reset])).toEqual({ remaining: 12, resetAt: new Date(1791378000 * 1_000).toISOString() })
    expect(of(['X-Ratelimit-Remaining: 12', reset, 'X-Ratelimit-Resource: core'])).toBe(null)
  })

  test("a gateway's 502: its HTML page is the body, with no rate headers", async () => {
    const http = parseHttp(head('502 Bad Gateway', ['Content-Length: 50', 'Content-Type: text/html']) + '<html><body><h1>502 Bad Gateway</h1></body></html>\n')
    expect(http.status).toBe(502)
    expect(rateOf(http.headers)).toBe(null)
    expect(parseReply(http.body, 'gh: HTTP 502\n', [A]).error).toBe('gh: HTTP 502')
  })

  test("a secondary limit's 403, and its retry-after", async () => {
    const http = parseHttp(head('403 Forbidden', ['Retry-After: 60', ...LIMITS]) + SECONDARY)
    expect(http.status).toBe(403)
    expect(retryAfterOf(http.headers)).toBe(60)
    expect(JSON.parse(http.body).message).toContain('secondary rate limit')
    expect(retryAfterOf({})).toBe(null)
    expect(retryAfterOf({ 'retry-after': 'soon' })).toBe(null)
  })

  test('no status line (no answer, or no -i): all of stdout is the body', async () => {
    expect(parseHttp('')).toEqual(NO_HTTP)
    const body = JSON.stringify({ data: { p0: null } })
    expect(parseHttp(body)).toEqual({ status: null, headers: {}, body })
    // A head with no blank line after it is no head.
    expect(parseHttp('HTTP/2.0 200 OK\nX-Ratelimit-Remaining: 1\r\n').status).toBe(null)
    // Nor headers at all: the blank line comes straight after the status.
    expect(parseHttp('HTTP/2.0 204 No Content\n\r\n')).toEqual({ status: 204, headers: {}, body: '' })
  })

  test("a gateway's 5xx, GraphQL's timeout or a cut connection is a timeout: worth one more try", async () => {
    // Each as gh says it (the first line of stderr, or the reply's own error), with or without -i's status.
    const blips: [string, number | null][] = [
      ['gh: HTTP 502', 502],
      ['gh: HTTP 503', null],
      ['gh: HTTP 504', null],
      ['gh: Server Error (HTTP 502)', null],
      ['Something went wrong while executing your query. This may be the result of a timeout, or it could be a GitHub bug.', 200],
      ['Something went wrong while executing your query. This may be the result of a timeout, or it could be a GitHub bug.', 504],
      ["We couldn't respond to your request in time. Sorry about that.", 502],
      ['Post "https://api.github.com/graphql": EOF', null],
      ['Post "https://api.github.com/graphql": unexpected EOF', null],
      ['Post "https://api.github.com/graphql": read tcp 10.0.0.2:50123->140.82.112.6:443: read: connection reset by peer', null],
      ['Post "https://api.github.com/graphql": stream error: stream ID 3; INTERNAL_ERROR; received from peer', null],
      ['Post "https://api.github.com/graphql": net/http: TLS handshake timeout', null],
    ]
    for (const [error, status] of blips) expect([error, failureOf(error, { ...NO_HTTP, status })]).toEqual([error, 'timeout'])
  })

  test('a secondary limit: by its message on a 403 or a 200, or by any retry-after', async () => {
    const message = 'You have exceeded a secondary rate limit. Please wait a few minutes before you try again.'
    expect(failureOf(`gh: ${message} (HTTP 403)`, { ...NO_HTTP, status: 403 })).toBe('secondary-limit')
    expect(failureOf(`gh: ${message} (HTTP 200)`, { ...NO_HTTP, status: 200 })).toBe('secondary-limit')
    expect(failureOf('You have triggered an abuse detection mechanism.', NO_HTTP)).toBe('secondary-limit')
    // A retry-after says it, whatever the status or the words.
    expect(failureOf('gh: HTTP 502', { status: 502, headers: { 'retry-after': '30' }, body: '' })).toBe('secondary-limit')
  })

  test('the primary limit: spent points, by the header or the message', async () => {
    expect(failureOf('API rate limit already exceeded for user ID 1.', { ...NO_HTTP, status: 200 })).toBe('primary-limit')
    expect(failureOf('gh: HTTP 403', { status: 403, headers: { 'x-ratelimit-remaining': '0' }, body: '' })).toBe('primary-limit')
  })

  test('a refusal, a missing PR and the rest are no blip', async () => {
    for (const [error, status] of [
      ['gh: Bad credentials (HTTP 401)', 401],
      ['gh: Resource not accessible by integration (HTTP 403)', 403],
      ['gh: Not Found (HTTP 404)', 404],
      ['gh: HTTP 500', 500],
      ['Not found, or no gh account can see it', null],
      ['error connecting to api.github.com', null],
      ['gh api graphql failed', null],
    ] as const) {
      expect([error, failureOf(error, { ...NO_HTTP, status })]).toEqual([error, 'other'])
    }
  })

})

describe("a round's time", () => {
  const until = Date.parse('2026-10-07T12:00:39Z')

  test('a call may take what is left of the lease, never more than its own cap', async () => {
    expect(callBudget(until, until - 29_000, 20_000)).toBe(20_000)
    expect(callBudget(until, until - 8_850, 20_000)).toBe(8_850)
  })

  test('with less than MIN_CALL_MS left, no call starts: the next round asks', async () => {
    expect(MIN_CALL_MS).toBe(5_000)
    expect(callBudget(until, until - MIN_CALL_MS, 20_000)).toBe(MIN_CALL_MS)
    expect(callBudget(until, until - MIN_CALL_MS + 1, 20_000)).toBe(null)
    expect(callBudget(until, until + 1_000, 20_000)).toBe(null)
  })
})

describe('the failure log', () => {
  test('one JSON line a call: when, who, how big, how long, and what GitHub said', async () => {
    const http = parseHttp(head('403 Forbidden', ['Retry-After: 60', ...LIMITS]) + SECONDARY)
    const error = `gh: ${JSON.parse(SECONDARY).message} (HTTP 403) ${'x'.repeat(300)}`
    const line = logLine(Date.parse('2026-10-07T12:00:00Z'), { login: 'octocat', size: 20, ms: 3_120, http, failure: 'secondary-limit', error })
    expect(line).not.toContain('\n')
    expect(JSON.parse(line)).toEqual({
      at: '2026-10-07T12:00:00.000Z',
      login: 'octocat',
      batch: 20,
      ms: 3_120,
      status: 403,
      class: 'secondary-limit',
      remaining: '4998',
      used: '2',
      resource: 'graphql',
      retryAfter: '60',
      error: error.slice(0, 200),
    })
    // A cut connection has no status or headers to give.
    const cut = JSON.parse(logLine(0, { login: 'octocat', size: 1, ms: 40, http: NO_HTTP, failure: 'timeout', error: 'Post "https://api.github.com/graphql": EOF' }))
    expect(cut).toMatchObject({ status: null, remaining: null, retryAfter: null })
  })

  test('appended last, and only the last LOG_LINES kept', async () => {
    expect(appendLog('', 'a')).toBe('a\n')
    expect(appendLog('a\nb\n', 'c')).toBe('a\nb\nc\n')
    expect(appendLog('a\nb\nc\n', 'd', 3)).toBe('b\nc\nd\n')
    const full = Array.from({ length: LOG_LINES }, (_, i) => `${i}`).join('\n')
    const next = appendLog(full, 'new').trimEnd().split('\n')
    expect(next).toHaveLength(LOG_LINES)
    expect([next[0], next.at(-1)]).toEqual(['1', 'new'])
  })
})
