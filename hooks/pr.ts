import type { Busy, Checks, MergeMethod, Mergeable, Reviews, Stage, Tone, TrackedPr } from '../types'
import { reviewsOf, toAddress, unresolved } from './review'
import type { ReviewFacts } from './review'

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

export type CheckItem = {
  __typename?: string
  name?: string
  context?: string
  status?: string
  conclusion?: string
  state?: string
  workflow?: string
  startedAt?: string | null
  createdAt?: string | null
  // Asked for only when a failure is handed to Claude (fix.ts), not by the poller. A GitHub
  // Actions check run's databaseId is its job's id.
  databaseId?: number | null
  detailsUrl?: string | null
  title?: string | null
  summary?: string | null
  targetUrl?: string | null
  description?: string | null
}

export const isStatusContext = (item: CheckItem) => item.__typename === 'StatusContext' || item.state !== undefined

// GitHub keeps every run of a check on the head commit: each pull_request event (a label,
// an edit, a re-run) starts another, and the rollup lists them all. The PR page shows the
// latest of each, so fold to that, or a failure that a later run fixed stays red.
export function latestRuns(items: readonly CheckItem[]): CheckItem[] {
  const latest = new Map<string, { item: CheckItem; at: number; order: number }>()
  items.forEach((item, order) => {
    const isContext = isStatusContext(item)
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

export function verdictOf(item: CheckItem): 'passed' | 'pending' | 'failed' {
  if (isStatusContext(item)) {
    return item.state === 'SUCCESS' ? 'passed' : item.state === 'PENDING' || item.state === 'EXPECTED' ? 'pending' : 'failed'
  }

  return item.status !== 'COMPLETED' ? 'pending' : PASSED.has(item.conclusion ?? '') ? 'passed' : 'failed'
}

export function summarizeChecks(items: readonly CheckItem[] | null | undefined): Checks {
  const checks = emptyChecks()
  for (const item of latestRuns(items ?? [])) {
    checks.total += 1
    const isContext = isStatusContext(item)
    const verdict = verdictOf(item)
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

// Its reviews (review.ts) ride along too.
export type GhPr = ReviewFacts & {
  number: number
  title: string
  url: string
  state: 'OPEN' | 'MERGED' | 'CLOSED'
  isDraft: boolean
  reviewDecision: string | null
  mergeStateStatus: string | null
  // MERGEABLE, CONFLICTING or UNKNOWN: the direct signal, when DIRTY is still being computed.
  mergeable?: string | null
  autoMergeRequest: { enabledAt?: string | null; mergeMethod?: string | null } | null
  viewerCanEnableAutoMerge?: boolean
  viewerCanDisableAutoMerge?: boolean
  // Its repository's merge settings, asked for in the same query.
  repository?: RepoMerge
  statusCheckRollup: CheckItem[] | null
  mergedAt: string | null
  closedAt?: string | null
  additions: number
  deletions: number
  headRefName: string
  baseRefName: string
  author: { login: string } | null
}

export type RepoMerge = {
  squashMergeAllowed?: boolean
  rebaseMergeAllowed?: boolean
  mergeCommitAllowed?: boolean
  viewerDefaultMergeMethod?: string | null
  // READ, TRIAGE, WRITE, MAINTAIN or ADMIN: merging takes WRITE.
  viewerPermission?: string | null
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

const METHODS: readonly MergeMethod[] = ['squash', 'rebase', 'merge']
const ALLOWS = { squash: 'squashMergeAllowed', rebase: 'rebaseMergeAllowed', merge: 'mergeCommitAllowed' } as const
const MAY_MERGE = new Set(['WRITE', 'MAINTAIN', 'ADMIN'])
// GitHub merges these now: HAS_HOOKS and UNSTABLE (an optional check failing) only warn.
const MERGES_NOW = new Set(['CLEAN', 'HAS_HOOKS', 'UNSTABLE'])

const asMethod = (value: string | null | undefined): MergeMethod | null => METHODS.find(m => m === value?.toLowerCase()) ?? null

// The methods the repo allows, the viewer's default first while the repo still allows it.
export function methodsOf(repo: RepoMerge | undefined): MergeMethod[] {
  const allowed = METHODS.filter(m => repo?.[ALLOWS[m]] === true)
  const preferred = asMethod(repo?.viewerDefaultMergeMethod)

  return preferred !== null && allowed.includes(preferred) ? [preferred, ...allowed.filter(m => m !== preferred)] : allowed
}

const isConflicted = (pr: GhPr) => pr.mergeStateStatus === 'DIRTY' || pr.mergeable === 'CONFLICTING'

export function noMerge(): Mergeable {
  return { methods: [], canMerge: false, canAuto: false, isAuto: false, autoMethod: null, canCancelAuto: false }
}

// An entry written by an older version of this mod (one that won the poll lease) has no merge
// facts: the card keeps what it last knew of them rather than lose its merge row for a round.
// What the entry says of the PR itself still counts: no button on a draft or a conflicted
// branch, and no merge now unless GitHub would merge it now.
export function mergeOf(pr: GhPr, kept: Mergeable): Mergeable {
  if (pr.state !== 'OPEN' || pr.repository !== undefined) return mergeableOf(pr)
  if (pr.isDraft || isConflicted(pr)) return { ...kept, canMerge: false, canAuto: false }

  return { ...kept, canMerge: kept.canMerge && MERGES_NOW.has(pr.mergeStateStatus ?? '') }
}

export function mergeableOf(pr: GhPr): Mergeable {
  if (pr.state !== 'OPEN') return noMerge()
  const methods = methodsOf(pr.repository)
  const isAuto = pr.autoMergeRequest != null
  const isEligible = !pr.isDraft && !isConflicted(pr) && !isAuto && methods.length > 0
  const canMerge = isEligible && MAY_MERGE.has(pr.repository?.viewerPermission ?? '') && MERGES_NOW.has(pr.mergeStateStatus ?? '')
  // Held back by checks or review, not by a stale branch.
  const isWaiting =
    pr.mergeStateStatus === 'BLOCKED' || (pr.mergeStateStatus !== 'BEHIND' && summarizeChecks(pr.statusCheckRollup).pending > 0)

  return {
    methods,
    canMerge,
    // False already where the repo doesn't allow auto-merge.
    canAuto: isEligible && !canMerge && isWaiting && pr.viewerCanEnableAutoMerge === true,
    isAuto,
    autoMethod: asMethod(pr.autoMergeRequest?.mergeMethod),
    canCancelAuto: isAuto && pr.viewerCanDisableAutoMerge === true,
  }
}

// The method a card offers: the one the person switched to, while it is still on offer.
export function pickMethod(methods: readonly MergeMethod[], chosen: MergeMethod | undefined): MergeMethod | null {
  return chosen !== undefined && methods.includes(chosen) ? chosen : (methods[0] ?? null)
}

export function nextMethod(methods: readonly MergeMethod[], current: MergeMethod): MergeMethod {
  return methods[(methods.indexOf(current) + 1) % methods.length] ?? current
}

export const MERGE_LABEL: Record<MergeMethod, string> = { squash: 'Squash & merge', rebase: 'Rebase & merge', merge: 'Merge' }
const MERGE_VERB: Record<MergeMethod, string> = { squash: 'Squash-merge', rebase: 'Rebase-merge', merge: 'Merge' }

// "Squash-merge #12 into main?": what Confirm will do.
export function mergeQuestion(method: MergeMethod, pr: { number: number; base: string }, isAuto: boolean): string {
  const into = `${MERGE_VERB[method]} #${pr.number} into ${pr.base || 'its base'}`

  return isAuto ? `${into} once checks and review pass?` : `${into}?`
}

export type MergeRun = { action: 'merge' | 'auto'; method: MergeMethod } | { action: 'cancel-auto' }

export function mergeArgv(url: string, run: MergeRun): string[] {
  if (run.action === 'cancel-auto') return ['pr', 'merge', url, '--disable-auto']

  return ['pr', 'merge', url, ...(run.action === 'auto' ? ['--auto'] : []), `--${run.method}`]
}

// A refusal of the method itself, not of this PR's state, in GitHub's words: the repo's
// settings ("Merge method squash merging is not allowed on this repository", "Squash merges
// are not allowed on this repository.", "Merge commits are not allowed…"), a ruleset's
// ("Rebase is not an allowed merge method"), or linear history's.
const METHOD_REFUSED = new RegExp(
  [
    String.raw`\bmerge method \w+ (?:merging )?is not allowed`,
    String.raw`\b(?:squash|rebase) merg(?:es|ing) (?:are|is) not allowed`,
    String.raw`\bmerge commits are not allowed`,
    String.raw`\bis not an allowed merge method`,
    String.raw`\bmust not contain merge commits`,
  ].join('|'),
  'i',
)

// What gh said, in plain words: without its ✗, the "GraphQL:" prefix, the mutation's name
// (camelCase, `(mergePullRequest)`; a branch or check named in brackets stays), or the hints
// about gh's own flags.
export function mergeError(stderr: string): { message: string; isMethodRefused: boolean } {
  const lines = stderr
    .split('\n')
    .map(line => line.trim().replace(/^[X✗!]\s+/, '').replace(/^GraphQL:\s*/, '').replace(/\s*\([a-z]+[A-Z]\w*\)$/, ''))
    .filter(line => line !== '' && !/^To (?:have|use|merge)\b/.test(line))
  const message = lines.join(' ') || 'gh pr merge failed'

  return { message, isMethodRefused: METHOD_REFUSED.test(message) }
}

// gh runs well inside this (its own timeouts stop it sooner; a collection's longest is a token,
// the checks and a log in turn, 20 + 20 + 60 s): a busy mark older than this was left by a
// module reload mid-run, so the row shows its buttons again.
export const BUSY_MS = 120_000

export function isBusy(busy: Busy | undefined, now: number): busy is Busy {
  return typeof busy?.at === 'number' && now - busy.at < BUSY_MS
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

// `kept`: the card's last review facts, for an entry an older version of this mod wrote (none
// of its own), so the headline doesn't flip back to "Waiting for review" for a round.
export function derive(pr: GhPr, kept: Reviews | null = null): Derived {
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
  if (pr.isDraft) {
    const draft = isConflicted(pr) ? 'Draft · merge conflicts' : 'Draft'

    return at('draft', 'subtle', '✎ DRAFT', checks.pending > 0 ? `${draft} · ${ci}` : draft, checks.pending > 0)
  }
  // A conflict blocks the merge whatever CI says, and checks on a conflicted head are often
  // stale (GitHub cannot build the merge commit), so it outranks CI and review. CI stays in
  // the line, and on the stepper, as the second thing to know.
  if (isConflicted(pr)) {
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
  const reviews = reviewsOf(pr) ?? kept
  // "2 unresolved threads", or nothing: open conversations, which a repo may require resolved.
  const threads = reviews === null ? '' : unresolved(reviews)
  const also = threads === '' ? '' : ` · ${threads}`
  if (pr.reviewDecision === 'CHANGES_REQUESTED') return at('review', 'error', '↺ CHANGES REQUESTED', 'Changes requested')
  // Comments waiting for the author, where nobody has approved yet: a PR still in review, or
  // one held back (BLOCKED) in a repo that needs no approval.
  const isWaitingOnAuthor = pr.reviewDecision === 'REVIEW_REQUIRED' || (pr.reviewDecision !== 'APPROVED' && pr.mergeStateStatus === 'BLOCKED')
  if (reviews !== null && threads !== '' && isWaitingOnAuthor) return at('review', 'warning', '💬 COMMENTS', toAddress(reviews))
  if (pr.reviewDecision === 'REVIEW_REQUIRED') {
    const isLookedAt = reviews?.reviewers.some(r => r.verdict !== 'requested') ?? false

    return at('review', 'warning', '◷ IN REVIEW', isLookedAt ? 'Reviewed · waiting for approval' : 'Waiting for review')
  }

  const isAuto = Boolean(pr.autoMergeRequest)
  switch (pr.mergeStateStatus) {
    case 'BEHIND':
      return at('ready', 'warning', '↓ BEHIND', isAuto ? 'Behind base · auto-merge on' : 'Branch is behind base')
    case 'BLOCKED': {
      // Approved, held back, threads open: the likely cause where the repo requires
      // conversations resolved (GitHub says which rule only to admins).
      const why = threads !== '' ? `Blocked${also}` : isAuto ? 'Blocked' : 'Blocked by branch rules'

      return at('ready', 'warning', '⏸ BLOCKED', isAuto ? `${why} · auto-merge on` : why)
    }
    case 'UNKNOWN':
      return at('ready', 'subtle', '◌ CHECKING', 'Computing mergeability…', true)
    default: {
      if (isAuto) return at('ready', 'suggestion', '⇢ AUTO-MERGING', 'Merging as soon as GitHub allows', true)
      const ready = pr.reviewDecision === 'APPROVED' ? 'Approved' : 'Ready to merge'

      return at('ready', 'success', '✓ READY', threads === '' ? 'Ready to merge' : `${ready}${also}`)
    }
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
    merge: noMerge(),
    reviews: null,
    reviewSeen: null,
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
