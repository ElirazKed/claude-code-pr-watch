import type { Checks, Stage, Tone, TrackedPr } from '../types'

const PR_URL = /https:\/\/github\.com\/([\w.-]+\/[\w.-]+)\/pull\/(\d+)/g

export type PrRef = { url: string; repo: string; number: number }

export function findPrUrls(text: string): PrRef[] {
  const found = new Map<string, PrRef>()
  for (const [, repo = '', num] of text.matchAll(PR_URL)) {
    const number = Number(num)
    const url = `https://github.com/${repo}/pull/${number}`
    found.set(url, { url, repo, number })
  }

  return [...found.values()]
}

// `gh pr view 12 -R owner/repo` style references that carry no URL.
export function refFromGhCommand(command: string): PrRef | null {
  const sub = command.match(/\bgh\s+pr\s+\w+\s+(?:[^|;&]*?\s)?#?(\d+)\b/)
  const repo = command.match(/(?:-R|--repo)[\s=]+([\w.-]+\/[\w.-]+)/)
  const name = repo?.[1]
  if (sub === null || name === undefined) return null
  const number = Number(sub[1])

  return { url: `https://github.com/${name}/pull/${number}`, repo: name, number }
}

const toolName = (tool: string) => tool.replace(/^mcp__[^_]+(?:_[^_]+)*?__/, '')

// `gh`/`git` as a command (start of the line, or after ; & | or a paren), not a word in an
// argument, a heredoc or a commit message.
const asCommand = (pattern: string) => new RegExp(String.raw`(?:^|[;&|(])\s*${pattern}`, 'm')
const GH_PR_CREATE = asCommand(String.raw`gh\s+pr\s+create\b`)
const GH_OR_PUSH = asCommand(String.raw`(?:gh\s+|git\s+push\b)`)
const GH_ONE_PR = asCommand(String.raw`gh\s+pr\s+(?:view|checks|diff|merge|ready|edit|comment|review|close|reopen|checkout|update-branch)\b`)

// PRs Claude opened (`gh pr create`, a create-pull-request MCP tool): the only ones watched
// without asking. The new PR's link is in the output.
export function created(tool: string, input: Record<string, unknown>, output: string): PrRef[] {
  const command = typeof input.command === 'string' ? input.command : ''
  const isCreate =
    tool === 'Bash' ? GH_PR_CREATE.test(command) : /create_?pull_?request|create_?prs?$/i.test(toolName(tool))

  return isCreate ? findPrUrls(output) : []
}

// PRs a tool call touched that Claude did not open, to offer: a PR link in what Claude sent
// (WebFetch, gh api, curl, MCP args), a GitHub-MCP pull request call, or a PR link in the
// output of git push / gh / a pull-request tool. Capped: a changelog full of links is not
// "dealing with" those PRs.
export function suggestable(tool: string, input: Record<string, unknown>, output: string): PrRef[] {
  const found = findPrUrls(JSON.stringify(input))
  const isPrTool = /pull|(^|_)prs?(_|$)/i.test(toolName(tool))
  if (isPrTool) {
    const repo = [input.owner, input.repo].every(v => typeof v === 'string') ? `${input.owner}/${input.repo}` : null
    const num = Number(input.pull_number ?? input.pullNumber ?? input.number ?? NaN)
    if (repo !== null && Number.isInteger(num) && num > 0) {
      found.push({ url: `https://github.com/${repo}/pull/${num}`, repo, number: num })
    }
  }
  const command = typeof input.command === 'string' ? input.command : ''
  if (isPrTool || GH_OR_PUSH.test(command)) found.push(...findPrUrls(output))
  const unique = [...new Map(found.map(ref => [ref.url, ref])).values()]

  return unique.length > 3 ? unique.slice(0, 1) : unique
}

// A `gh pr` subcommand about one PR, which gh resolves to the current branch's PR when no
// number is given (`gh pr checks`). Not list, status or create.
export function isGhOnePrCommand(command: string): boolean {
  return GH_ONE_PR.test(command)
}

// The directory a `cd dir && gh pr ...` command ran in, if it says.
export function cwdOf(command: string): string | undefined {
  return command.match(/^\s*cd\s+("[^"]+"|'[^']+'|\S+)\s*&&/)?.[1]?.replace(/^["']|["']$/g, '')
}

type CheckItem = {
  __typename?: string
  name?: string
  context?: string
  status?: string
  conclusion?: string
  state?: string
  workflow?: string
  startedAt?: string | null
  createdAt?: string | null
}

// GitHub keeps every run of a check on the head commit: each pull_request event (a label,
// an edit, a re-run) starts another, and the rollup lists them all. The PR page shows the
// latest of each, so fold to that, or a failure that a later run fixed stays red.
export function latestRuns(items: readonly CheckItem[]): CheckItem[] {
  const latest = new Map<string, { item: CheckItem; at: number; order: number }>()
  items.forEach((item, order) => {
    const isContext = item.__typename === 'StatusContext' || item.state !== undefined
    const key = isContext ? `status:${item.context ?? ''}` : `run:${item.workflow ?? ''}/${item.name ?? ''}`
    // A run not started yet (queued, waiting) is the newest of its check.
    const stamp = isContext ? item.createdAt : item.startedAt
    const at = stamp == null ? (isContext ? 0 : Number.POSITIVE_INFINITY) : Date.parse(stamp) || 0
    const held = latest.get(key)
    if (held === undefined || at > held.at || (at === held.at && order > held.order)) latest.set(key, { item, at, order })
  })

  return [...latest.values()].sort((a, b) => a.order - b.order).map(entry => entry.item)
}

const PASSED = new Set(['SUCCESS', 'NEUTRAL', 'SKIPPED'])

export function emptyChecks(): Checks {
  return { passed: 0, failed: 0, pending: 0, total: 0, failing: [], running: [], queued: 0 }
}

export function summarizeChecks(items: readonly CheckItem[] | null | undefined): Checks {
  const checks = emptyChecks()
  for (const item of latestRuns(items ?? [])) {
    checks.total += 1
    const isContext = item.__typename === 'StatusContext' || item.state !== undefined
    const verdict = isContext
      ? item.state === 'SUCCESS'
        ? 'passed'
        : item.state === 'PENDING' || item.state === 'EXPECTED'
          ? 'pending'
          : 'failed'
      : item.status !== 'COMPLETED'
        ? 'pending'
        : PASSED.has(item.conclusion ?? '')
          ? 'passed'
          : 'failed'
    checks[verdict] += 1
    const name = item.name ?? item.context ?? 'check'
    if (verdict === 'failed') checks.failing.push(name)
    if (verdict === 'pending') {
      // A status context only says PENDING once its CI has picked it up; EXPECTED is still waiting.
      const isStarted = isContext ? item.state === 'PENDING' : item.status === 'IN_PROGRESS'
      if (isStarted) checks.running.push(name)
      else checks.queued += 1
    }
  }

  return checks
}

// "Running build, e2e +1 · 2 queued": what CI is doing right now.
export function activity(checks: Checks): string {
  const names = checks.running.slice(0, 2).join(', ')
  const more = checks.running.length > 2 ? ` +${checks.running.length - 2}` : ''
  const running = checks.running.length > 0 ? `Running ${names}${more}` : ''
  const queued = checks.queued > 0 ? `${checks.queued} queued` : ''

  return [running, queued].filter(Boolean).join(' · ')
}

export type GhPr = {
  number: number
  title: string
  url: string
  state: 'OPEN' | 'MERGED' | 'CLOSED'
  isDraft: boolean
  reviewDecision: string | null
  mergeStateStatus: string | null
  // MERGEABLE, CONFLICTING or UNKNOWN: the direct signal, when DIRTY is still being computed.
  mergeable?: string | null
  autoMergeRequest: unknown
  statusCheckRollup: CheckItem[] | null
  mergedAt: string | null
  closedAt?: string | null
  additions: number
  deletions: number
  headRefName: string
  baseRefName: string
  author: { login: string } | null
}

export function metaOf(pr: GhPr) {
  return {
    branch: pr.headRefName,
    base: pr.baseRefName,
    additions: pr.additions,
    deletions: pr.deletions,
    author: pr.author?.login ?? '',
    mergedAt: pr.mergedAt,
  }
}

export function ago(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000))
  if (s < 10) return 'just now'
  if (s < 60) return `${s}s ago`
  if (s < 3600) return `${Math.floor(s / 60)}m ago`
  if (s < 86_400) return `${Math.floor(s / 3600)}h ago`

  return `${Math.floor(s / 86_400)}d ago`
}

export type Derived = {
  stage: Stage
  headline: string
  pill: string
  tone: Tone
  isMoving: boolean
  checks: Checks
}

export function derive(pr: GhPr): Derived {
  const checks = summarizeChecks(pr.statusCheckRollup)
  const ci = `${activity(checks)} · ${checks.passed}/${checks.total} passed`
  const at = (stage: Stage, tone: Tone, pill: string, headline: string, isMoving = false): Derived => ({
    stage,
    tone,
    pill,
    headline,
    isMoving,
    checks,
  })

  if (pr.state === 'MERGED') return at('merged', 'merged', '✓ MERGED', 'Merged')
  if (pr.state === 'CLOSED') return at('closed', 'subtle', '✕ CLOSED', 'Closed without merging')
  const names = checks.failing.slice(0, 2).join(', ')
  const failing = `${names}${checks.failing.length > 2 ? ` +${checks.failing.length - 2}` : ''}`
  // A conflict blocks the merge whatever CI says, and checks on a conflicted head are often
  // stale (GitHub cannot build the merge commit), so it outranks CI and review. CI stays in
  // the line, and on the stepper, as the second thing to know.
  const isConflicted = pr.mergeStateStatus === 'DIRTY' || pr.mergeable === 'CONFLICTING'
  if (pr.isDraft) {
    const draft = isConflicted ? 'Draft · merge conflicts' : 'Draft'

    return at('draft', 'subtle', '✎ DRAFT', checks.pending > 0 ? `${draft} · ${ci}` : draft, checks.pending > 0)
  }
  if (isConflicted) {
    const stage = checks.failed > 0 || checks.pending > 0 ? 'checks' : 'ready'
    const status =
      checks.failed > 0 ? ` · failing: ${failing}` : checks.pending > 0 ? ` · ${activity(checks)}` : ''

    return at(stage, 'error', '⚠ CONFLICTS', `Merge conflicts with base${status}`, checks.pending > 0)
  }
  if (checks.failed > 0) {
    // Still running checks keep the card live: the spinner, the shimmer and the fast poll.
    return checks.pending > 0
      ? at('checks', 'error', '✗ CI FAILING', `Failing: ${failing} · ${checks.pending} still running`, true)
      : at('checks', 'error', '✗ CI FAILED', `Failing: ${failing}`)
  }
  if (checks.pending > 0) return at('checks', 'warning', '● CI RUNNING', ci, true)
  if (pr.reviewDecision === 'CHANGES_REQUESTED') return at('review', 'error', '↺ CHANGES REQUESTED', 'Changes requested')
  if (pr.reviewDecision === 'REVIEW_REQUIRED') return at('review', 'warning', '◷ IN REVIEW', 'Waiting for review')

  const isAuto = Boolean(pr.autoMergeRequest)
  switch (pr.mergeStateStatus) {
    case 'BEHIND':
      return at('ready', 'warning', '↓ BEHIND', isAuto ? 'Behind base · auto-merge on' : 'Branch is behind base')
    case 'BLOCKED':
      return at('ready', 'warning', '⏸ BLOCKED', isAuto ? 'Blocked · auto-merge on' : 'Blocked by branch rules')
    case 'UNKNOWN':
      return at('ready', 'subtle', '◌ CHECKING', 'Computing mergeability…', true)
    default:
      return isAuto
        ? at('ready', 'suggestion', '⇢ AUTO-MERGING', 'Merging as soon as GitHub allows', true)
        : at('ready', 'success', '✓ READY', 'Ready to merge')
  }
}

export function placeholder(ref: PrRef): TrackedPr {
  return {
    ...ref,
    title: '',
    state: 'UNKNOWN',
    stage: 'unknown',
    headline: 'Fetching from GitHub…',
    pill: '◌ LOADING',
    tone: 'subtle',
    isMoving: true,
    checks: emptyChecks(),
    branch: '',
    base: '',
    additions: 0,
    deletions: 0,
    author: '',
    mergedAt: null,
    checkedAt: null,
    error: null,
  }
}

export function isActive(pr: TrackedPr): boolean {
  return pr.state === 'OPEN' || pr.state === 'UNKNOWN'
}

export const STEPS = ['Open', 'CI', 'Review', 'Merge'] as const

// Which step the PR is on: steps before it are done, it carries the tone.
export function stepIndex(stage: Stage): number {
  switch (stage) {
    case 'draft':
    case 'unknown':
      return 0
    case 'checks':
      return 1
    case 'review':
      return 2
    case 'ready':
    case 'closed':
      return 3
    case 'merged':
      return 4
  }
}
