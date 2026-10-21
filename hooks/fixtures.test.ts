// The status fixtures the companion's Rust port is held to (companion/fixtures/): each case a PR
// exactly as the poll stores it in results.json (`prs[url].pr`, parseReply's GhPr), and the
// card the mod derives from it. A change to what the mod derives fails here until the fixtures
// are regenerated (companion/fixtures/update.sh), and then the Rust tests until they match.
import { describe, expect, test } from 'claude-code/testing'

import stored from '../companion/fixtures/status'
import { parseReply } from './batch'
import { PILL, derive, mergeableOf, metaOf, stepIndex } from './pr'
import type { GhPr } from './pr'
import { reviewsOf } from './review'

const AT = '2026-10-07T11:50:00Z'

type GqlCheck =
  | { __typename: 'CheckRun'; name: string; status: string; conclusion: string | null; startedAt: string | null; checkSuite: unknown }
  | { __typename: 'StatusContext'; context: string; state: string; createdAt: string }

// A check run as GitHub answers the poll's query, its workflow inside its suite.
const run = (name: string, status: string, conclusion: string | null = null, startedAt: string | null = AT, workflow = 'CI'): GqlCheck => ({
  __typename: 'CheckRun',
  name,
  status,
  conclusion,
  startedAt,
  checkSuite: { workflowRun: { workflow: { name: workflow } } },
})
const passed = (name: string) => run(name, 'COMPLETED', 'SUCCESS')
const failed = (name: string) => run(name, 'COMPLETED', 'FAILURE')
const status = (context: string, state: string, createdAt = AT): GqlCheck => ({ __typename: 'StatusContext', context, state, createdAt })

const said = (login: string, state: string, typename = 'User') => ({ author: { __typename: typename, login }, state, submittedAt: AT })
const thread = (isResolved: boolean, isOutdated = false) => ({ isResolved, isOutdated })

const REPO = { squashMergeAllowed: true, rebaseMergeAllowed: true, mergeCommitAllowed: false, viewerDefaultMergeMethod: 'SQUASH', viewerPermission: 'WRITE' }

type Facts = Partial<Omit<GhPr, 'statusCheckRollup' | 'repository'>> & { checks?: GqlCheck[]; repository?: Record<string, unknown> }

// A PR as the poll stores it: GitHub's answer to the batched query for it, read by parseReply.
function polled(number: number, facts: Facts): GhPr {
  const url = `https://github.com/acme/app/pull/${number}`
  const { checks = [], repository = REPO, ...pr } = facts
  const node = {
    number,
    title: 'Retry uploads with exponential backoff',
    url,
    state: 'OPEN',
    isDraft: false,
    reviewDecision: null,
    mergeStateStatus: 'CLEAN',
    mergeable: 'MERGEABLE',
    mergedAt: null,
    closedAt: null,
    additions: 120,
    deletions: 14,
    headRefName: 'feat/retry',
    baseRefName: 'main',
    author: { login: 'octocat' },
    autoMergeRequest: null,
    viewerCanEnableAutoMerge: false,
    viewerCanDisableAutoMerge: false,
    latestReviews: { totalCount: 0, nodes: [] },
    latestOpinionatedReviews: { nodes: [] },
    reviewRequests: { totalCount: 0, nodes: [] },
    reviewThreads: { totalCount: 0, nodes: [] },
    ...pr,
    // GitHub says null, not an empty rollup, for a commit no check has reported on.
    commits: { nodes: [{ commit: { statusCheckRollup: checks.length === 0 ? null : { contexts: { nodes: checks } } } }] },
  }
  const data = { rateLimit: { limit: 5000, remaining: 4000, resetAt: '2026-10-07T13:00:00Z' }, p0: { ...repository, pullRequest: node } }
  const read = parseReply(JSON.stringify({ data }), '', [{ url, repo: 'acme/app', number }]).found.get(url)
  if (read === undefined) throw new Error(`#${number} did not parse`)

  return read
}

const reviewed = (latest: ReturnType<typeof said>[], threads: ReturnType<typeof thread>[] = [], requested: string[] = []): Facts => ({
  latestReviews: { totalCount: latest.length, nodes: latest },
  latestOpinionatedReviews: { nodes: latest.filter(review => review.state !== 'COMMENTED') },
  reviewRequests: { totalCount: requested.length, nodes: requested.map(login => ({ requestedReviewer: { __typename: 'User', login } })) },
  reviewThreads: { totalCount: threads.length, nodes: threads },
})

const CASES: { name: string; about: string; pr: GhPr }[] = [
  { name: 'draft', about: 'A draft with CI running: the draft says so, and stays live.', pr: polled(1, { isDraft: true, mergeStateStatus: 'DRAFT', checks: [run('build', 'IN_PROGRESS')] }) },
  { name: 'draft-conflicted', about: 'A draft whose branch conflicts.', pr: polled(2, { isDraft: true, mergeStateStatus: 'DIRTY', mergeable: 'CONFLICTING' }) },
  {
    name: 'ci-running',
    about: 'Open, CI running: running checks named, queued ones counted.',
    pr: polled(3, {
      reviewDecision: 'REVIEW_REQUIRED',
      mergeStateStatus: 'BLOCKED',
      viewerCanEnableAutoMerge: true,
      checks: [passed('lint'), run('build', 'IN_PROGRESS'), run('e2e', 'IN_PROGRESS'), run('types', 'IN_PROGRESS'), run('deploy', 'QUEUED', null, null), run('docs', 'WAITING', null, null)],
    }),
  },
  {
    name: 'ci-failing-still-running',
    about: 'A check failed while others still run: failing, and live.',
    pr: polled(4, { mergeStateStatus: 'BLOCKED', checks: [failed('build'), run('e2e', 'QUEUED', null, null)] }),
  },
  { name: 'ci-failed', about: 'A check failed and nothing runs: settled.', pr: polled(5, { mergeStateStatus: 'BLOCKED', checks: [failed('build'), failed('e2e'), failed('lint'), passed('types')] }) },
  {
    name: 'ci-rerun-passed',
    about: 'A check re-run on the same commit counts by its latest run, as the PR page does.',
    pr: polled(6, {
      checks: [
        run('lint-docs', 'COMPLETED', 'FAILURE', '2026-10-07T11:00:00Z', 'Docs'),
        run('lint-docs', 'COMPLETED', 'SUCCESS', '2026-10-07T11:10:00Z', 'Docs'),
        passed('build'),
      ],
    }),
  },
  {
    name: 'ci-same-job-two-workflows',
    about: "Two workflows' same-named jobs are two checks.",
    pr: polled(7, { mergeStateStatus: 'BLOCKED', checks: [run('test', 'COMPLETED', 'SUCCESS', AT, 'Unit'), run('test', 'COMPLETED', 'FAILURE', AT, 'E2E')] }),
  },
  {
    name: 'status-contexts',
    about: 'Commit statuses count too: the latest report of each, PENDING as running, EXPECTED as queued.',
    pr: polled(8, {
      mergeStateStatus: 'BLOCKED',
      checks: [status('ci/jenkins', 'FAILURE', '2026-10-07T11:00:00Z'), status('ci/jenkins', 'PENDING', '2026-10-07T11:30:00Z'), status('ci/coverage', 'EXPECTED'), passed('lint')],
    }),
  },
  {
    name: 'optional-check-failing',
    about: 'UNSTABLE: only an optional check fails. The card says CI failed, yet GitHub would merge it now.',
    pr: polled(9, { mergeStateStatus: 'UNSTABLE', reviewDecision: 'APPROVED', checks: [passed('build'), failed('flaky-e2e')], ...reviewed([said('alice', 'APPROVED')]) }),
  },
  {
    name: 'approved-mergeable',
    about: 'Approved, CI green, clean: ready, and the viewer may merge (their default method first).',
    pr: polled(10, { reviewDecision: 'APPROVED', checks: [passed('build'), passed('lint')], ...reviewed([said('alice', 'APPROVED')]) }),
  },
  {
    name: 'approved-read-only',
    about: 'Ready, but the viewer may only read the repo: no merge.',
    pr: polled(11, { reviewDecision: 'APPROVED', checks: [passed('build')], repository: { ...REPO, viewerPermission: 'READ' }, ...reviewed([said('alice', 'APPROVED')]) }),
  },
  {
    name: 'approved-open-threads',
    about: 'Approved and clean with threads open: ready, and how many (outdated ones counted apart).',
    pr: polled(12, { reviewDecision: 'APPROVED', checks: [passed('build')], ...reviewed([said('alice', 'APPROVED')], [thread(false), thread(false, true), thread(true)]) }),
  },
  {
    name: 'approved-blocked-threads',
    about: 'Approved but held back with threads open (conversations must be resolved).',
    pr: polled(13, { reviewDecision: 'APPROVED', mergeStateStatus: 'BLOCKED', checks: [passed('build')], ...reviewed([said('alice', 'APPROVED')], [thread(false), thread(false)]) }),
  },
  {
    name: 'changes-requested',
    about: 'Changes requested: stays so whatever the threads, the change request ranked first.',
    pr: polled(14, {
      reviewDecision: 'CHANGES_REQUESTED',
      mergeStateStatus: 'BLOCKED',
      checks: [passed('build')],
      ...reviewed([said('alice', 'APPROVED'), said('bob', 'CHANGES_REQUESTED')], [thread(false)]),
    }),
  },
  {
    name: 'comments-to-address',
    about: 'Comment-only reviews with threads open, past the 50 the poll asks for: a floor, not a count. A review app is named short.',
    pr: polled(15, {
      reviewDecision: 'REVIEW_REQUIRED',
      mergeStateStatus: 'BLOCKED',
      checks: [passed('build')],
      ...reviewed([said('carol', 'COMMENTED'), said('copilot-pull-request-reviewer', 'COMMENTED', 'Bot')], [thread(false), thread(false, true), thread(true)]),
      reviewThreads: { totalCount: 60, nodes: [thread(false), thread(false, true), thread(true)] },
    }),
  },
  {
    name: 'waiting-for-review',
    about: 'Green CI, a review requested and none given yet.',
    pr: polled(16, { reviewDecision: 'REVIEW_REQUIRED', mergeStateStatus: 'BLOCKED', checks: [passed('build')], ...reviewed([], [], ['alice']) }),
  },
  {
    name: 'reviewed-waiting-for-approval',
    about: 'Looked at (comments, every thread resolved), still waiting for an approval.',
    pr: polled(17, { reviewDecision: 'REVIEW_REQUIRED', mergeStateStatus: 'BLOCKED', checks: [passed('build')], ...reviewed([said('carol', 'COMMENTED')], [thread(true)]) }),
  },
  {
    name: 'conflicts',
    about: 'Conflicts outrank failing CI, which stays in the line.',
    pr: polled(18, { mergeStateStatus: 'DIRTY', mergeable: 'CONFLICTING', reviewDecision: 'REVIEW_REQUIRED', checks: [failed('lint-docs'), passed('lint')] }),
  },
  {
    name: 'conflicts-computing',
    about: 'mergeable CONFLICTING counts while mergeStateStatus is still being computed.',
    pr: polled(19, { mergeStateStatus: 'UNKNOWN', mergeable: 'CONFLICTING', checks: [passed('build')] }),
  },
  { name: 'behind', about: 'Approved, but the branch is behind its base.', pr: polled(20, { reviewDecision: 'APPROVED', mergeStateStatus: 'BEHIND', checks: [passed('build')] }) },
  { name: 'blocked', about: 'Held back by branch rules, nothing else to say why.', pr: polled(21, { reviewDecision: 'APPROVED', mergeStateStatus: 'BLOCKED', checks: [passed('build')] }) },
  { name: 'mergeability-unknown', about: 'GitHub is still working out whether it merges.', pr: polled(22, { mergeStateStatus: 'UNKNOWN', mergeable: 'UNKNOWN', checks: [passed('build')] }) },
  {
    name: 'auto-merge-offered',
    about: 'Waiting on checks where auto-merge is allowed: offered.',
    pr: polled(23, { mergeStateStatus: 'BLOCKED', viewerCanEnableAutoMerge: true, checks: [run('build', 'IN_PROGRESS')] }),
  },
  {
    name: 'auto-merge-on',
    about: 'Auto-merge on, nothing left to wait for: merging as soon as GitHub allows.',
    pr: polled(24, { reviewDecision: 'APPROVED', autoMergeRequest: { enabledAt: AT, mergeMethod: 'REBASE' }, viewerCanDisableAutoMerge: true, checks: [passed('build')] }),
  },
  {
    name: 'auto-merge-on-blocked',
    about: 'Auto-merge on, held back by branch rules: the viewer may turn it off.',
    pr: polled(25, { mergeStateStatus: 'BLOCKED', autoMergeRequest: { enabledAt: AT, mergeMethod: 'SQUASH' }, viewerCanDisableAutoMerge: true, checks: [passed('build')] }),
  },
  { name: 'no-checks', about: 'No check has reported, no review needed: ready to merge.', pr: polled(26, {}) },
  {
    name: 'merged',
    about: 'Merged: every step done, nothing to merge.',
    pr: polled(27, { state: 'MERGED', mergeStateStatus: 'UNKNOWN', mergedAt: '2026-10-07T11:55:00Z', closedAt: '2026-10-07T11:55:00Z', reviewDecision: 'APPROVED', checks: [passed('build')] }),
  },
  { name: 'closed', about: 'Closed without merging.', pr: polled(28, { state: 'CLOSED', mergeStateStatus: 'UNKNOWN', closedAt: '2026-10-07T11:55:00Z', checks: [failed('build')] }) },
]

// A pill's name in PILL (band.ts ranks by it), for a reader that words it otherwise.
const pillKind = (pill: string) => Object.entries(PILL).find(([, text]) => text === pill)?.[0] ?? null

// What a card shows of a PR, as data: no theme, no colour, no clock (the card's "checked 1m
// ago" is the reader's own). `kept` facts (an older mod's entry) are not part of it.
function cardOf(pr: GhPr) {
  const { stage, tone, pill, headline, isMoving, checks } = derive(pr)

  return {
    stage,
    step: stepIndex(stage),
    tone,
    pill,
    pillKind: pillKind(pill),
    headline,
    isMoving,
    checks,
    isConflicted: metaOf(pr).isConflicted,
    merge: mergeableOf(pr),
    reviews: reviewsOf(pr),
  }
}

// The test kit prints what a test logs; hooks have no console, so the types don't name it.
const log = (text: string) => (globalThis as { console?: { log: (text: string) => void } }).console?.log(text)

// The fixtures file, as update.sh writes it: the Rust tests read the JSON from its first `[`,
// so the comments above it hold none.
const BEGIN = '-----BEGIN companion/fixtures/status.ts-----'
const END = '-----END companion/fixtures/status.ts-----'
const fileOf = (cases: unknown) =>
  '// Generated by companion/fixtures/update.sh from hooks/fixtures.test.ts: edit the cases there.\n' +
  "// Each case: a PR as the poll stores it in results.json, and the card the mod derives from it.\n" +
  `export default ${JSON.stringify(cases, null, 2)}\n`

describe('status fixtures', () => {
  test('each fixture is what the mod derives now (else: companion/fixtures/update.sh)', async () => {
    const fresh = JSON.parse(JSON.stringify(CASES.map(({ name, about, pr }) => ({ name, about, pr, card: cardOf(pr) })))) as unknown
    const isSame = JSON.stringify(stored) === JSON.stringify(fresh)
    // update.sh takes the file from between the markers.
    if (!isSame) log(`${BEGIN}\n${fileOf(fresh)}${END}`)
    expect(stored).toEqual(fresh)
  })

  test('the cases are named apart, and each is a state of its own', async () => {
    expect(new Set(CASES.map(c => c.name)).size).toBe(CASES.length)
    expect(new Set(CASES.map(c => JSON.stringify(cardOf(c.pr)))).size).toBe(CASES.length)
  })

  test('the fixtures cover every pill the poll can lead to', async () => {
    const kinds = new Set(CASES.map(c => pillKind(derive(c.pr).pill)))
    // `loading` is the card before any data: never derived from a PR.
    expect(Object.keys(PILL).filter(kind => kind !== 'loading' && !kinds.has(kind))).toEqual([])
  })
})
