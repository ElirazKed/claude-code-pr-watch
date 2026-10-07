// One GraphQL query for many PRs: an aliased field per PR, so a tick costs one call whatever
// the count, plus the rate limit it left. `gh pr view` fields, shaped back the way it gives them.
import type { GhPr, PrRef } from './pr'

// GitHub caps a query's node count; 100 checks for each of 40 PRs stays far inside it.
export const BATCH_SIZE = 40

const FRAGMENT = `fragment pr on PullRequest {
  number title url state isDraft reviewDecision mergeStateStatus mergeable mergedAt closedAt
  additions deletions headRefName baseRefName author { login } autoMergeRequest { enabledAt }
  commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100) { nodes {
    __typename
    ... on CheckRun { name status conclusion }
    ... on StatusContext { context state }
  } } } } } }
}`

export function buildQuery(refs: readonly PrRef[]): string {
  const fields = refs.map((ref, i) => {
    const [owner = '', name = ''] = ref.repo.split('/')

    return `p${i}: repository(owner: ${JSON.stringify(owner)}, name: ${JSON.stringify(name)}) { pullRequest(number: ${ref.number}) { ...pr } }`
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
  commits: { nodes: { commit: { statusCheckRollup: { contexts: { nodes: GhPr['statusCheckRollup'] } } | null } }[] }
}

type GqlReply = {
  data?: Record<string, unknown> | null
  errors?: { type?: string; message?: string }[]
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
    const pr = (data[`p${i}`] as { pullRequest: GqlPr | null } | null | undefined)?.pullRequest
    if (pr == null) {
      result.missing.push(ref)

      return
    }
    const { commits, ...rest } = pr
    result.found.set(ref.url, { ...rest, statusCheckRollup: commits.nodes[0]?.commit.statusCheckRollup?.contexts.nodes ?? [] })
  })

  return result
}

export function chunks<T>(items: readonly T[], size = BATCH_SIZE): T[][] {
  const out: T[][] = []
  for (let i = 0; i < items.length; i += size) out.push(items.slice(i, i + size))

  return out
}
