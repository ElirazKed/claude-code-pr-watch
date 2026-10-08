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
  const sub = command.match(/\bgh\s+pr\s+[\w-]+\s+(?:[^|;&]*?\s)?#?(\d+)\b/)
  const repo = command.match(/(?:-R|--repo)[\s=]+([\w.-]+\/[\w.-]+)/)
  const name = repo?.[1]
  if (sub === null || name === undefined) return null
  const number = Number(sub[1])

  return { url: `https://github.com/${name}/pull/${number}`, repo: name, number }
}

const toolName = (tool: string) => tool.replace(/^mcp__[^_]+(?:_[^_]+)*?__/, '')

// A command's shell code: heredoc bodies and quoted strings blanked, so a `gh pr create` in a
// PR body or a commit message doesn't count, while `GH_TOKEN=$(…) gh pr create` and
// `git -c k="…" push` do.
export function shellCode(command: string): string {
  return command
    .replace(/<<-?\s*(['"]?)(\w+)\1([^\n]*)\n[\s\S]*?\n[ \t]*\2[ \t]*(?=\n|$)/g, (_m, _q, _tag, rest: string) => ` ${rest}`)
    .replace(/'[^']*'|"(?:\\[\s\S]|[^"\\])*"/g, '""')
}

// `gh pr` subcommands that act on a PR. `checks` counts: following a PR's CI is shepherding it.
const ACTS = new Set(['create', 'merge', 'ready', 'edit', 'comment', 'review', 'close', 'reopen', 'checkout', 'update-branch', 'checks'])
// Single-PR read subcommands gh resolves to the current branch's PR when given no number.
const READS_ONE = new Set(['view', 'diff'])
const GIT_PUSH = /\bgit\b[^;&|\n]*?\spush\b/
const GH_API_WRITE = /\bgh\s+api\b[^;&|\n]*?(?:(?:-X|--method)[\s=]*(?:POST|PATCH|PUT|DELETE)\b|\s(?:-f|-F|--field|--raw-field|--input)\s)/i
const API_PULL = /\brepos\/([\w.-]+\/[\w.-]+)\/pulls\/(\d+)/g

export type Touched = {
  // PRs Claude acted on (opened, pushed to, commented, reviewed, merged, followed CI): watched.
  acted: PrRef[]
  // PRs Claude only read: offered, if they look like the person's own work.
  read: PrRef[]
  // The command named no PR (`git push`, `gh pr checks`), so gh must say which one it meant.
  resolve: 'acted' | 'read' | null
}

// GitHub MCP tools that change a PR start with one of these (`merge_pull_request`,
// `request_copilot_review`) or end in `_write`; `get_`, `list_`, `search_`, `_read` only read.
const WRITE_VERBS = new Set(['create', 'update', 'merge', 'add', 'submit', 'request', 'push', 'enable', 'resolve', 'close', 'reopen', 'mark', 'dismiss'])

const NONE: Touched = { acted: [], read: [], resolve: null }

const uniq = (refs: PrRef[]) => [...new Map(refs.map(ref => [ref.url, ref])).values()]

// A changelog full of links is not "dealing with" those PRs.
const capped = (refs: PrRef[]) => (refs.length > 3 ? refs.slice(0, 1) : refs)

export function touched(tool: string, input: Record<string, unknown>, output: string): Touched {
  if (tool === 'Bash') return touchedByCommand(typeof input.command === 'string' ? input.command : '', output)
  const name = toolName(tool)
  const isPrTool = /pull|(^|_)prs?(_|$)/i.test(name)
  if (!isPrTool) return { ...NONE, read: capped(findPrUrls(JSON.stringify(input))) }

  const refs = findPrUrls(JSON.stringify(input))
  const repo = [input.owner, input.repo].every(v => typeof v === 'string') ? `${input.owner}/${input.repo}` : null
  const num = Number(input.pull_number ?? input.pullNumber ?? input.number ?? NaN)
  if (repo !== null && Number.isInteger(num) && num > 0) refs.push({ url: `https://github.com/${repo}/pull/${num}`, repo, number: num })
  const words = name.toLowerCase().split(/[_-]/)
  const isWrite = WRITE_VERBS.has(words[0] ?? '') || words.includes('write')
  if (!isWrite) {
    return { ...NONE, read: capped(uniq([...refs, ...findPrUrls(output)])) }
  }
  // A new PR's own link comes first in the reply; its body may quote others.
  const made = refs.length === 0 ? findPrUrls(output).slice(0, 1) : []

  return { ...NONE, acted: uniq([...refs, ...made]) }
}

function touchedByCommand(command: string, output: string): Touched {
  const code = shellCode(command)
  const subs = [...code.matchAll(/\bgh\s+pr\s+([\w-]+)/g)].map(m => m[1] ?? '')
  const apiRefs = [...code.matchAll(API_PULL)].map(([, repo = '', n]) => ({ url: `https://github.com/${repo}/pull/${Number(n)}`, repo, number: Number(n) }))
  // `gh pr checks 12 -R o/r`; not `gh pr list --limit 30 -R o/r`.
  const isOnePr = subs.some(sub => sub !== 'create' && (ACTS.has(sub) || READS_ONE.has(sub)))
  const fromFlags = isOnePr ? refFromGhCommand(code) : null
  const named = uniq([...findPrUrls(code), ...apiRefs, ...(fromFlags ? [fromFlags] : [])])
  const isPush = GIT_PUSH.test(code)
  if (subs.some(sub => ACTS.has(sub)) || isPush || GH_API_WRITE.test(code)) {
    // `gh pr create` prints the new PR; `gh pr comment` the comment's link on its PR.
    const acted = uniq([...named, ...findPrUrls(output)])
    const resolve = acted.length === 0 && (isPush || isOnePr) ? 'acted' : null

    return { acted, read: [], resolve }
  }
  if (!/\bgh\s/.test(code)) return NONE
  const read = capped(uniq([...named, ...findPrUrls(output)]))

  return { acted: [], read, resolve: read.length === 0 && subs.some(sub => READS_ONE.has(sub)) ? 'read' : null }
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
