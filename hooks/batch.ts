// One GraphQL query for many PRs: an aliased field per PR, so a tick costs one call whatever
// the count, plus the rate limit it left. `gh pr view` fields, shaped back the way it gives them.
import type { CheckItem, GhPr, PrRef, RepoMerge } from './pr'

// GitHub caps a query's node count; 100 checks for each of 40 PRs stays far inside it. The
// repo's merge settings ride on each PR's own repository field: plain scalars, which cost
// nothing in rate-limit points or nodes.
export const BATCH_SIZE = 40

// The head commit's checks, with whatever more a caller asks of each kind.
const checksOf = (run = '', status = '') => `commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100) { nodes {
    __typename
    ... on CheckRun { name status conclusion startedAt ${run}checkSuite { workflowRun { workflow { name } } } }
    ... on StatusContext { context state createdAt${status} }
  } } } } } }`

const FRAGMENT = `fragment pr on PullRequest {
  number title url state isDraft reviewDecision mergeStateStatus mergeable mergedAt closedAt
  additions deletions headRefName baseRefName author { login } autoMergeRequest { enabledAt mergeMethod }
  viewerCanEnableAutoMerge viewerCanDisableAutoMerge
  ${checksOf()}
}
fragment repo on Repository {
  squashMergeAllowed rebaseMergeAllowed mergeCommitAllowed autoMergeAllowed viewerDefaultMergeMethod viewerPermission
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

type GqlPr = Omit<GhPr, 'statusCheckRollup'> & {
  commits: { nodes: { commit: { statusCheckRollup: { contexts: { nodes: GqlCheck[] } } | null } }[] }
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
// stay apart when runs are folded to the latest of each.
function flattenCheck(node: GqlCheck) {
  const { checkSuite, ...check } = node
  const workflow = checkSuite?.workflowRun?.workflow?.name

  return workflow === undefined ? check : { ...check, workflow }
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
    const nodes = commits.nodes[0]?.commit.statusCheckRollup?.contexts.nodes ?? []
    result.found.set(ref.url, { ...rest, repository, statusCheckRollup: nodes.map(flattenCheck) })
  })

  return result
}

export function chunks<T>(items: readonly T[], size = BATCH_SIZE): T[][] {
  const out: T[][] = []
  for (let i = 0; i < items.length; i += size) out.push(items.slice(i, i + size))

  return out
}

// One PR's checks with what handing a failure to Claude needs: a GitHub Actions run's job id
// (its databaseId) and the links and summaries the rest have. Asked only on that press, so
// the poller's query stays as lean as it was; the same alias shape keeps one parser.
export function checksQuery(ref: PrRef): string {
  const [owner = '', name = ''] = ref.repo.split('/')

  return `query {
  p0: repository(owner: ${JSON.stringify(owner)}, name: ${JSON.stringify(name)}) { pullRequest(number: ${ref.number}) {
    ${checksOf('databaseId detailsUrl title summary ', ' description targetUrl')}
  } }
}`
}

export function parseChecks(stdout: string, stderr: string, ref: PrRef): { checks: CheckItem[] } | { error: string } {
  const reply = parseReply(stdout, stderr, [ref])
  if (reply.error !== null) return { error: reply.error }
  const pr = reply.found.get(ref.url)

  return pr === undefined ? { error: "Not found, or this gh account can't see it" } : { checks: pr.statusCheckRollup ?? [] }
}
