// A card's tabs: Overview, what the card has always said; Checks, Reviews and Threads, each
// read on its own when opened, so the poll asks GitHub no more than it did. Pure: the pane
// fetches and draws.
import type { Approvals, CheckRow, Gate, Tab, ThreadRow, TrackedPr } from '../types'
import { cleanBody, openThreads, spot } from './address'
import type { GhThread } from './address'
import { clip } from './fix'
import { isStatusContext, latestRuns, verdictOf } from './pr'
import type { CheckItem, PrRef } from './pr'
import { nameOf } from './review'

export const TABS: readonly Tab[] = ['overview', 'checks', 'reviews', 'threads']

const NAME: Record<Tab, string> = { overview: 'Overview', checks: 'Checks', reviews: 'Reviews', threads: 'Threads' }

// A tab's name and the count beside it: "Checks" 2/3 passed, "Reviews" 1/2 approvals the
// rules ask for (or how many reviewers, before the rules are read), "Threads" 3 unresolved.
export function tabLabel(tab: Tab, pr: TrackedPr, approvals?: Approvals): { name: string; count: string } {
  const name = NAME[tab]
  if (tab === 'checks') return { name, count: pr.checks.total > 0 ? `${pr.checks.passed}/${pr.checks.total}` : '' }
  if (tab === 'reviews') {
    if (approvals !== undefined && approvals.required > 0) return { name, count: `${Math.min(approvals.approved, approvals.required)}/${approvals.required}` }
    const reviewers = pr.reviews?.reviewers.length ?? 0

    return { name, count: reviewers > 0 ? `${reviewers}` : '' }
  }
  if (tab === 'threads') {
    const reviews = pr.reviews

    return { name, count: reviews === null || reviews.unresolved === 0 ? '' : `${reviews.unresolved}${reviews.isCapped ? '+' : ''}` }
  }

  return { name, count: '' }
}

// What a tab's reading depends on, as the poll sees it: a round that changes it reads the tab
// again; one that doesn't, doesn't.
export function seenOf(tab: Exclude<Tab, 'overview'>, pr: TrackedPr): string {
  if (tab === 'checks') return JSON.stringify(pr.checks)
  if (tab === 'reviews') return JSON.stringify([pr.base, pr.reviews?.reviewers ?? [], pr.reviewSeen])

  return JSON.stringify([pr.reviews?.unresolved ?? 0, pr.reviews?.outdated ?? 0, pr.reviewSeen])
}

// ---- Checks -------------------------------------------------------------------------------

const ORDER: readonly CheckRow['state'][] = ['failed', 'running', 'queued', 'passed', 'skipped']

// The head commit's checks, the latest run of each, failing first, as the card counts them.
export function checkRows(items: readonly CheckItem[]): CheckRow[] {
  const rows = latestRuns(items).map((item): CheckRow => {
    const isContext = isStatusContext(item)
    const verdict = verdictOf(item)
    const isStarted = isContext ? item.state === 'PENDING' : item.status === 'IN_PROGRESS'
    const state =
      verdict === 'failed'
        ? 'failed'
        : verdict === 'pending'
          ? isStarted
            ? 'running'
            : 'queued'
          : !isContext && item.conclusion === 'SKIPPED'
            ? 'skipped'
            : 'passed'

    return { name: item.name ?? item.context ?? 'check', workflow: item.workflow ?? null, state }
  })

  return rows.map((row, i) => ({ row, i })).sort((a, b) => ORDER.indexOf(a.row.state) - ORDER.indexOf(b.row.state) || a.i - b.i).map(({ row }) => row)
}

// ---- Threads ------------------------------------------------------------------------------

const TEXT_CHARS = 200

// The threads still open, by file and line: where, who opened it, the first line they wrote.
export function threadRows(threads: readonly GhThread[]): ThreadRow[] {
  return openThreads(threads).map(thread => {
    const first = thread.comments.nodes[0]
    const line = cleanBody(first?.body ?? '').split('\n').find(text => text.trim() !== '') ?? ''
    const count = thread.comments.totalCount ?? thread.comments.nodes.length

    return {
      spot: spot(thread),
      author: first?.author?.login ?? '',
      text: clip(line.trim(), TEXT_CHARS),
      replies: Math.max(0, count - 1),
      isOutdated: thread.isOutdated,
    }
  })
}

// ---- Reviews ------------------------------------------------------------------------------

type Actor = { __typename?: string; login?: string; slug?: string } | null

type Rule = {
  type: string
  parameters?: {
    requiredApprovingReviewCount?: number
    requireCodeOwnerReview?: boolean
    requiredReviewers?: { minimumApprovals: number; reviewerId: string; filePatterns: string[] }[]
  } | null
}

// What the Reviews tab asks of one PR, in GitHub's shape.
export type GhApprovals = {
  author?: { login: string } | null
  baseRef?: {
    rules?: { nodes: Rule[] } | null
    branchProtectionRule?: { requiredApprovingReviewCount?: number | null; requiresCodeOwnerReviews?: boolean } | null
  } | null
  reviewRequests?: { nodes: { asCodeOwner?: boolean; requestedReviewer: Actor }[] } | null
  latestOpinionatedReviews?: { nodes: { state: string; author: Actor; onBehalfOf?: { nodes: { slug: string }[] } | null }[] } | null
}

// The base branch's rules (rulesets, and classic branch protection where there is one), the
// code owners asked, and each approval or change request with the teams it counted for. Asked
// only when the tab opens; the poll's own review connections stay as they were.
export function approvalsQuery(ref: PrRef): string {
  const [owner = '', name = ''] = ref.repo.split('/')

  return `query {
  p0: repository(owner: ${JSON.stringify(owner)}, name: ${JSON.stringify(name)}) { pullRequest(number: ${ref.number}) {
    author { login }
    baseRef {
      rules(first: 50) { nodes { type parameters { ... on PullRequestParameters {
        requiredApprovingReviewCount requireCodeOwnerReview requiredReviewers { minimumApprovals reviewerId filePatterns }
      } } } }
      branchProtectionRule { requiredApprovingReviewCount requiresCodeOwnerReviews }
    }
    reviewRequests(first: 30) { nodes { asCodeOwner requestedReviewer {
      __typename ... on User { login } ... on Bot { login } ... on Mannequin { login } ... on Team { slug }
    } } }
    latestOpinionatedReviews(first: 30) { nodes { state author { __typename login } onBehalfOf(first: 10) { nodes { slug } } } }
  } }
}`
}

// A ruleset names its required teams by node id: their slugs, asked once the rules are read.
export const teamsQuery = (ids: readonly string[]) => `query {
  teams: nodes(ids: ${JSON.stringify(ids)}) { __typename ... on Team { id slug } }
}`

type Reply<T> = { data?: T | null; errors?: { message?: string }[] }

const firstLine = (text: string) => text.trim().split('\n')[0] ?? ''

function parse<T>(stdout: string, stderr: string): { data: T } | { error: string } {
  let reply: Reply<T>
  try {
    reply = JSON.parse(stdout) as Reply<T>
  } catch {
    return { error: firstLine(stderr) || 'gh api graphql failed' }
  }

  return reply.data == null ? { error: reply.errors?.[0]?.message ?? (firstLine(stderr) || 'GitHub returned no data') } : { data: reply.data }
}

// The PR gh printed. A field this login may not read (classic branch protection, for some)
// errs alone, and gh exits 1 with the rest still printed: what came is used.
export function parseApprovals(stdout: string, stderr: string): { pr: GhApprovals } | { error: string } {
  const reply = parse<{ p0?: { pullRequest?: GhApprovals | null } | null }>(stdout, stderr)
  if ('error' in reply) return reply
  const pr = reply.data.p0?.pullRequest

  return pr == null ? { error: "Not found, or this gh account can't see it" } : { pr }
}

// Slugs by node id; an id that isn't a team this login can see is left out.
export function parseTeams(stdout: string): Map<string, string> {
  const reply = parse<{ teams?: ({ id?: string; slug?: string } | null)[] }>(stdout, '')
  if ('error' in reply) return new Map()

  return new Map((reply.data.teams ?? []).flatMap(team => (team?.id && team.slug ? [[team.id, team.slug] as const] : [])))
}

const pullRules = (pr: GhApprovals) =>
  (pr.baseRef?.rules?.nodes ?? []).flatMap(rule => (rule.type === 'PULL_REQUEST' && rule.parameters != null ? [rule.parameters] : []))

// The teams a ruleset requires, by node id, to ask their slugs of.
export function ruledTeamIds(pr: GhApprovals): string[] {
  return [...new Set(pullRules(pr).flatMap(rule => (rule.requiredReviewers ?? []).map(reviewer => reviewer.reviewerId)))]
}

type Draft = { by: string[]; against: string[]; need: number; isAsked: boolean; isRuled: boolean; patterns: string[] }

const STATE_ORDER: readonly Gate['state'][] = ['approved', 'changes', 'waiting', 'maybe']

// Where the PR stands against its base branch's rules. A team's gate is met by its members'
// approvals (the reviews GitHub says counted for it); a code owner asked is waiting until then.
// A code owner who is a person shows only while asked: once they approve, GitHub no longer
// says they were one, and the approval counts among the rest. A ruleset's team counts only for
// the files its patterns name, which GitHub doesn't match for us, so one neither asked nor
// approving may not apply: `maybe`, with its patterns. `org` names the teams (@acme/backend).
export function approvalsOf(pr: GhApprovals, org: string, teams: ReadonlyMap<string, string>): Approvals {
  const author = pr.author?.login
  const reviews = (pr.latestOpinionatedReviews?.nodes ?? []).filter(
    review => review.author?.login !== author && (review.state === 'APPROVED' || review.state === 'CHANGES_REQUESTED'),
  )
  const rules = pullRules(pr)
  const classic = pr.baseRef?.branchProtectionRule
  const gates = new Map<string, Draft>()
  const gate = (name: string) => {
    const held = gates.get(name)
    if (held !== undefined) return held
    const made: Draft = { by: [], against: [], need: 1, isAsked: false, isRuled: false, patterns: [] }
    gates.set(name, made)

    return made
  }
  const team = (slug: string) => `@${org}/${slug}`
  for (const review of reviews) {
    for (const { slug } of review.onBehalfOf?.nodes ?? []) {
      const held = gate(team(slug))
      ;(review.state === 'APPROVED' ? held.by : held.against).push(nameOf(review.author))
    }
  }
  for (const { asCodeOwner, requestedReviewer: actor } of pr.reviewRequests?.nodes ?? []) {
    if (asCodeOwner !== true || actor == null) continue
    gate(actor.__typename === 'Team' ? team(actor.slug ?? '') : `@${nameOf(actor)}`).isAsked = true
  }
  for (const rule of rules) {
    for (const reviewer of rule.requiredReviewers ?? []) {
      const slug = teams.get(reviewer.reviewerId)
      if (slug === undefined || reviewer.minimumApprovals <= 0) continue
      const held = gate(team(slug))
      held.need = held.isRuled ? Math.max(held.need, reviewer.minimumApprovals) : reviewer.minimumApprovals
      held.isRuled = true
      held.patterns.push(...reviewer.filePatterns.filter(pattern => !held.patterns.includes(pattern)))
    }
  }
  const stateOf = (held: Draft): Gate['state'] => {
    if (held.against.length > 0) return 'changes'
    if (held.by.length >= held.need) return 'approved'

    return held.isAsked || held.by.length > 0 ? 'waiting' : 'maybe'
  }

  return {
    required: Math.max(0, classic?.requiredApprovingReviewCount ?? 0, ...rules.map(rule => rule.requiredApprovingReviewCount ?? 0)),
    approved: reviews.filter(review => review.state === 'APPROVED').length,
    isOwnersRequired: classic?.requiresCodeOwnerReviews === true || rules.some(rule => rule.requireCodeOwnerReview === true),
    gates: [...gates]
      .map(([name, held]): Gate => ({ name, state: stateOf(held), by: held.by, need: held.need, patterns: held.patterns }))
      .sort((a, b) => STATE_ORDER.indexOf(a.state) - STATE_ORDER.indexOf(b.state)),
  }
}

// What a gate's row says beside its name: who approved, who asked for changes, how many of
// how many, or which files bring it in.
export function gateNote(gate: Gate): string {
  if (gate.state === 'changes') return 'changes requested'
  if (gate.state === 'approved') return gate.by.join(', ')
  if (gate.state === 'maybe') {
    const more = gate.patterns.length > 2 ? ` +${gate.patterns.length - 2}` : ''

    return `if it changes ${gate.patterns.slice(0, 2).join(', ')}${more}`
  }

  return gate.need > 1 ? `${gate.by.length} of ${gate.need} · waiting` : 'waiting'
}

// The row under the gates: approvals the rules still ask for, from anyone who may give one.
export function stillNeeded(approvals: Approvals): string {
  const left = approvals.required - approvals.approved

  return left <= 0 ? '' : `${left} more approval${left === 1 ? '' : 's'}`
}

// The heading: "Required · 1 of 2 approvals · code owners", or that the rules ask for none.
export function approvalsHeading(approvals: Approvals): string {
  const owners = approvals.isOwnersRequired ? ' · code owners' : ''
  if (approvals.required === 0) return approvals.gates.length > 0 || owners !== '' ? `Required${owners}` : 'No approvals required by the branch rules'

  return `Required · ${Math.min(approvals.approved, approvals.required)} of ${approvals.required} approval${approvals.required === 1 ? '' : 's'}${owners}`
}
