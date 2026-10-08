// One GraphQL query for many PRs: an aliased field per PR, so a tick costs one call whatever
// the count, plus the rate limit it left. `gh pr view` fields, shaped back the way it gives them.
import type { CheckItem, GhPr, PrRef, RepoMerge } from './pr'

// GitHub caps a query's node count; 100 checks for each of 40 PRs stays far inside it. The
// repo's merge settings ride on each PR's own repository field: plain scalars, which cost
// nothing in rate-limit points or nodes. (Whether the repo allows auto-merge needs no field:
// GitHub's viewerCanEnableAutoMerge is false where it doesn't.)
export const BATCH_SIZE = 40

// The head commit's checks, with whatever more a caller asks of each kind, and of the commit.
const checksOf = (run = '', status = '', commit = '') => `commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100) { nodes {
    __typename
    ... on CheckRun { name status conclusion startedAt ${run}checkSuite { workflowRun { workflow { name } } } }
    ... on StatusContext { context state createdAt${status} }
  } } }${commit} } } }`

const FRAGMENT = `fragment pr on PullRequest {
  number title url state isDraft reviewDecision mergeStateStatus mergeable mergedAt closedAt
  additions deletions headRefName baseRefName author { login } autoMergeRequest { enabledAt mergeMethod }
  viewerCanEnableAutoMerge viewerCanDisableAutoMerge
  ${checksOf()}
}
fragment repo on Repository {
  squashMergeAllowed rebaseMergeAllowed mergeCommitAllowed viewerDefaultMergeMethod viewerPermission
}`

export function buildQuery(refs: readonly PrRef[]): string {
  const fields = refs.map((ref, i) => {
    const [owner = '', name = ''] = ref.repo.split('/')

    return `p${i}: repository(owner: ${JSON.stringify(owner)}, name: ${JSON.stringify(name)}) { pullRequest(number: ${ref.number}) { ...pr } ...repo }`
  })

  return `query {\n  rateLimit { remaining resetAt }\n  ${fields.join('\n  ')}\n}\n${FRAGMENT}`
}

export type RateLimit = { remaining: number; resetAt: string }

export type BatchResult = {
  found: Map<string, GhPr>
  // Not visible to this token: another account may see them.
  missing: PrRef[]
  rate: RateLimit | null
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
  const result: BatchResult = { found: new Map(), missing: [], rate: null, error: null }
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
