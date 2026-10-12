// One GraphQL query for many PRs: an aliased field per PR, so a tick costs one call whatever
// the count, plus the rate limit it left. `gh pr view` fields, shaped back the way it gives them.
import type { CheckItem, GhPr, PrRef, RepoMerge } from './pr'
import { LATEST_FIELD, latestIn } from './update'

// 20 PRs a query, run one after another: a query of 40 busy PRs can outlast GitHub's own
// timeout (a 502 or 504) and lose the whole round, while two of 20 cost the same points and
// each answers in a few seconds. 100 checks for each stays far inside the node cap. The
// repo's merge settings ride on each PR's own repository field: plain scalars, which cost
// nothing in rate-limit points or nodes. (Whether the repo allows auto-merge needs no field:
// GitHub's viewerCanEnableAutoMerge is false where it doesn't.)
export const BATCH_SIZE = 20

// The head commit's checks, with whatever more a caller asks of each kind, and of the commit.
const checksOf = (run = '', status = '', commit = '') => `commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100) { nodes {
    __typename
    ... on CheckRun { name status conclusion startedAt ${run}checkSuite { workflowRun { workflow { name } } } }
    ... on StatusContext { context state createdAt${status} }
  } } }${commit} } } }`

// Who reviewed and the open threads (review.ts): small pages, logins and states, and of the
// newest threads only whether they are resolved or outdated, never a comment's text. Each
// connection costs a PR one more request in GitHub's reckoning (100 make a point), so a batch
// of 20 costs 1 point (up to 24 would).
const REVIEWS = `latestReviews(first: 10) { totalCount nodes { author { __typename login } state submittedAt } }
  latestOpinionatedReviews(first: 10) { nodes { author { __typename login } state submittedAt } }
  reviewRequests(first: 10) { totalCount nodes { requestedReviewer {
    __typename ... on User { login } ... on Bot { login } ... on Mannequin { login } ... on Team { slug }
  } } }
  reviewThreads(last: 50) { totalCount nodes { isResolved isOutdated } }`

const FRAGMENT = `fragment pr on PullRequest {
  number title url state isDraft reviewDecision mergeStateStatus mergeable mergedAt closedAt
  additions deletions headRefName baseRefName author { login } autoMergeRequest { enabledAt mergeMethod }
  viewerCanEnableAutoMerge viewerCanDisableAutoMerge
  ${REVIEWS}
  ${checksOf()}
}
fragment repo on Repository {
  squashMergeAllowed rebaseMergeAllowed mergeCommitAllowed viewerDefaultMergeMethod viewerPermission
}`

// `withLatest`: also ask for pr-watch's newest version (update.ts), which costs nothing more.
export function buildQuery(refs: readonly PrRef[], withLatest = false): string {
  const fields = refs.map((ref, i) => {
    const [owner = '', name = ''] = ref.repo.split('/')

    return `p${i}: repository(owner: ${JSON.stringify(owner)}, name: ${JSON.stringify(name)}) { pullRequest(number: ${ref.number}) { ...pr } ...repo }`
  })

  const latest = withLatest ? `\n  ${LATEST_FIELD}` : ''

  return `query {\n  rateLimit { limit remaining resetAt }${latest}\n  ${fields.join('\n  ')}\n}\n${FRAGMENT}`
}

// `limit`: the hourly points, where the reply said (an older results.json has none).
export type RateLimit = { remaining: number; resetAt: string; limit?: number }

export type BatchResult = {
  found: Map<string, GhPr>
  // Not visible to this token: another account may see them.
  missing: PrRef[]
  rate: RateLimit | null
  // pr-watch's newest version, where the query asked and GitHub could say.
  latest: string | null
  // The whole call failed (network, auth, a limit): nothing in it is known.
  error: string | null
}

// A failed check run's title and summary, asked apart from the rest (checksQuery).
type GqlAbout = { databaseId?: number | null; title?: string | null; summary?: string | null }

type GqlPr = Omit<GhPr, 'statusCheckRollup'> & {
  commits: {
    nodes: {
      commit: {
        statusCheckRollup: { contexts: { nodes: GqlCheck[] } } | null
        checkSuites?: { nodes: ({ checkRuns: { nodes: GqlAbout[] } | null } | null)[] } | null
      }
    }[]
  }
}

type GqlRepo = RepoMerge & { pullRequest: GqlPr | null }

type GqlReply = {
  data?: Record<string, unknown> | null
  errors?: { type?: string; message?: string }[]
}

type GqlCheck = NonNullable<GhPr['statusCheckRollup']>[number] & {
  checkSuite?: { workflowRun?: { workflow?: { name?: string } | null } | null } | null
}

// A check run's workflow name, lifted out of its suite, so two workflows' same-named jobs
// stay apart when runs are folded to the latest of each; and its title and summary, where
// they were asked apart.
function flattenCheck(node: GqlCheck, about: ReadonlyMap<number, GqlAbout>) {
  const { checkSuite, ...check } = node
  const workflow = checkSuite?.workflowRun?.workflow?.name
  const text = typeof check.databaseId === 'number' ? about.get(check.databaseId) : undefined
  const named = workflow === undefined ? check : { ...check, workflow }

  return text === undefined ? named : { ...named, title: text.title ?? null, summary: text.summary ?? null }
}

const firstLine = (text: string) => text.trim().split('\n')[0] ?? ''

// `gh api graphql` exits 1 when any alias errs, yet still prints every alias it resolved.
export function parseReply(stdout: string, stderr: string, refs: readonly PrRef[]): BatchResult {
  const result: BatchResult = { found: new Map(), missing: [], rate: null, latest: null, error: null }
  let reply: GqlReply
  try {
    reply = JSON.parse(stdout) as GqlReply
  } catch {
    result.error = firstLine(stderr) || 'gh api graphql failed'

    return result
  }
  const data = reply.data
  if (data == null) {
    result.error = reply.errors?.[0]?.message ?? (firstLine(stderr) || 'GitHub returned no data')

    return result
  }
  result.rate = (data.rateLimit as RateLimit | undefined) ?? null
  // Null, with an error of its own, where the repo can't be read: the PRs don't mind.
  result.latest = latestIn(data.pw)
  refs.forEach((ref, i) => {
    const { pullRequest: pr, ...repository } = (data[`p${i}`] as GqlRepo | null | undefined) ?? { pullRequest: null }
    if (pr == null) {
      result.missing.push(ref)

      return
    }
    const { commits, ...rest } = pr
    const commit = commits.nodes[0]?.commit
    const nodes = commit?.statusCheckRollup?.contexts.nodes ?? []
    const runs = (commit?.checkSuites?.nodes ?? []).flatMap(suite => suite?.checkRuns?.nodes ?? [])
    const about = new Map(runs.flatMap(run => (typeof run.databaseId === 'number' ? [[run.databaseId, run] as const] : [])))
    result.found.set(ref.url, { ...rest, repository, statusCheckRollup: nodes.map(node => flattenCheck(node, about)) })
  })

  return result
}

// ---- What GitHub said about a call ----------------------------------------------------
// The poller runs `gh api graphql -i`, which prints, on stdout and whatever the status, the
// status line ("HTTP/2.0 502 Bad Gateway", ending "\n"), the headers (each ending "\r\n",
// named as Go spells them: "X-Ratelimit-Remaining"), a blank line, then the body. A call that
// got no answer (a cut connection) prints nothing there; gh's stderr says why as before.

export type Http = { status: number | null; headers: Record<string, string>; body: string }

// Headers keyed in lower case; without a status line, all of stdout is the body.
export function parseHttp(stdout: string): Http {
  const head = stdout.match(/^HTTP\/[\d.]+ (\d{3})[^\n]*\n((?:[^\r\n]+\r?\n)*)\r?\n/)
  if (head === null) return { status: null, headers: {}, body: stdout }
  const lines = (head[2] ?? '').split(/\r?\n/).filter(line => line.includes(':'))
  const headers = Object.fromEntries(lines.map(line => [line.slice(0, line.indexOf(':')).trim().toLowerCase(), line.slice(line.indexOf(':') + 1).trim()]))

  return { status: Number(head[1]), headers, body: stdout.slice(head[0].length) }
}

// The primary limit as the headers give it, failed calls included (the body's rateLimit
// field comes only with data); null where they don't, or where they speak of a limit other
// than GraphQL's.
export function rateOf(headers: Readonly<Record<string, string>>): RateLimit | null {
  const remaining = Number(headers['x-ratelimit-remaining'] ?? NaN)
  const reset = Number(headers['x-ratelimit-reset'] ?? NaN)
  const limit = Number(headers['x-ratelimit-limit'] ?? NaN)
  const resource = headers['x-ratelimit-resource']
  if (!Number.isFinite(remaining) || !Number.isFinite(reset) || (resource !== undefined && resource !== 'graphql')) return null

  return { remaining, resetAt: new Date(reset * 1_000).toISOString(), ...(Number.isFinite(limit) && { limit }) }
}

// Seconds GitHub asks to wait (`retry-after`), or null where it doesn't say.
export function retryAfterOf(headers: Readonly<Record<string, string>>): number | null {
  // An empty header is no wait named: Number('') would read it as 0.
  const said = headers['retry-after']?.trim()
  const seconds = said ? Number(said) : NaN

  return Number.isFinite(seconds) && seconds >= 0 ? seconds : null
}

// Why a whole call failed, as far as asking again goes:
// - `secondary-limit`: GitHub's limits on bursts and compute (a 403, or a 200 with errors,
//   saying "secondary rate limit" or "abuse", or any retry-after). Never asked again at once:
//   GitHub may ban a client that keeps on, so no session polls until it has waited.
// - `primary-limit`: the hourly points are spent (x-ratelimit-remaining 0, or GitHub says
//   "API rate limit exceeded"). The round backs off to the reset, as on a low rate.
// - `timeout`: GitHub having a moment. Its gateway's 502, 503 or 504 (gh says "gh: HTTP 502",
//   or "gh: Server Error (HTTP 502)" where the page says why), GraphQL's own timeout ("This
//   may be the result of a timeout", "couldn't respond to your request in time"), or the
//   connection cut on the way (`Post "https://api.github.com/graphql": EOF`, a reset, an
//   HTTP/2 stream error). Asked once more.
// - `other`: a refusal (401, 403, 404) and the rest: asking again won't help.
export type CallFailure = 'timeout' | 'secondary-limit' | 'primary-limit' | 'other'

export function failureOf(error: string, http: Http): CallFailure {
  if (retryAfterOf(http.headers) !== null || /secondary rate limit|abuse/i.test(error)) return 'secondary-limit'
  if (http.headers['x-ratelimit-remaining'] === '0' || /rate limit/i.test(error)) return 'primary-limit'
  const isGateway = [502, 503, 504].includes(http.status ?? 0) || /\bHTTP 50[234]\b/.test(error)
  const isCut = /\b(timeout|EOF|connection reset|broken pipe|stream error|GOAWAY|deadline exceeded)\b|request in time/i.test(error)

  return isGateway || isCut ? 'timeout' : 'other'
}

// ---- A round's time ------------------------------------------------------------------
// A round never outlives its lease, so it never runs beside another session's. A call starts
// only with MIN_CALL_MS left before `until` (a batch has answered in 2.5 s, and a gateway that
// gives up takes 10), and may take what is left, never more than `max`. Null: no call starts;
// what it would have asked is left for the next round.
export const MIN_CALL_MS = 5_000

export function callBudget(until: number, now: number, max: number): number | null {
  const left = until - now

  return left >= MIN_CALL_MS ? Math.min(left, max) : null
}

// ---- The failure log ------------------------------------------------------------------
// ~/.cache/pr-watch/errors.log: one JSON line per failed call, the newest last, the last
// LOG_LINES kept, so the next "⚠ 502" has a record to `tail`.
export const LOG_LINES = 200

export type CallLog = { login: string; size: number; ms: number; http: Http; failure: CallFailure; error: string }

export function logLine(at: number, call: CallLog): string {
  const { headers } = call.http
  const header = (name: string) => headers[name] ?? null

  return JSON.stringify({
    at: new Date(at).toISOString(),
    login: call.login,
    batch: call.size,
    ms: call.ms,
    status: call.http.status,
    class: call.failure,
    remaining: header('x-ratelimit-remaining'),
    used: header('x-ratelimit-used'),
    resource: header('x-ratelimit-resource'),
    retryAfter: header('retry-after'),
    error: call.error.slice(0, 200),
  })
}

// The log with `line` added, cut to its last LOG_LINES lines.
export function appendLog(log: string, line: string, max = LOG_LINES): string {
  const lines = [...log.split('\n').filter(Boolean), line]

  return `${lines.slice(-max).join('\n')}\n`
}

export function chunks<T>(items: readonly T[], size = BATCH_SIZE): T[][] {
  const out: T[][] = []
  for (let i = 0; i < items.length; i += size) out.push(items.slice(i, i + size))

  return out
}

// The conclusions a check run fails with (pr.ts's verdictOf: all but SUCCESS, NEUTRAL, SKIPPED).
const FAILED = 'ACTION_REQUIRED, CANCELLED, FAILURE, STALE, STARTUP_FAILURE, TIMED_OUT'
// The failed runs' titles and summaries, by the commit's check suites: a summary may run to
// 65 KB, so a passing run's is never asked. Joined to its check by databaseId.
const FAILED_RUNS = `
    checkSuites(last: 50) { nodes { checkRuns(first: 20, filterBy: { checkType: LATEST, conclusions: [${FAILED}] }) {
      nodes { databaseId title summary }
    } } }`

// One PR's checks with what handing a failure to Claude needs: a GitHub Actions run's job id
// (its databaseId), the links and status descriptions (short: GitHub caps them at 140
// characters), and the failed runs' summaries. One call, still one point. Asked only on that
// press, so the poller's query stays as lean as it was; the same alias shape keeps one parser.
export function checksQuery(ref: PrRef): string {
  const [owner = '', name = ''] = ref.repo.split('/')

  return `query {
  p0: repository(owner: ${JSON.stringify(owner)}, name: ${JSON.stringify(name)}) { pullRequest(number: ${ref.number}) {
    ${checksOf('databaseId detailsUrl ', ' description targetUrl', FAILED_RUNS)}
  } }
}`
}

export function parseChecks(stdout: string, stderr: string, ref: PrRef): { checks: CheckItem[] } | { error: string } {
  const reply = parseReply(stdout, stderr, [ref])
  if (reply.error !== null) return { error: reply.error }
  const pr = reply.found.get(ref.url)

  return pr === undefined ? { error: "Not found, or this gh account can't see it" } : { checks: pr.statusCheckRollup ?? [] }
}
