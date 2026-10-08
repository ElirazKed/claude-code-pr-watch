import { describe, expect, test } from 'claude-code/testing'

import { newReviews, reviewToasts, reviewsOf, threadLine, threadsBeside, verdictLine } from './review'
import type { GhReview, ReviewFacts } from './review'

const review = (login: string, state: string, submittedAt: string, __typename = 'User'): GhReview => ({
  author: { __typename, login },
  state,
  submittedAt,
})
const thread = (isResolved: boolean, isOutdated = false) => ({ isResolved, isOutdated })

// GitHub's review connections: `latest` each reviewer's last review, `opinionated` each one's
// last approval or change request, `requested` the logins with a request pending.
const facts = (
  latest: GhReview[],
  opinionated: GhReview[] = [],
  requested: string[] = [],
  threads: { isResolved: boolean; isOutdated: boolean }[] = [],
): ReviewFacts & { author: { login: string } } => ({
  author: { login: 'octocat' },
  latestReviews: { totalCount: latest.length, nodes: latest },
  latestOpinionatedReviews: { nodes: opinionated },
  reviewRequests: { totalCount: requested.length, nodes: requested.map(login => ({ requestedReviewer: { __typename: 'User', login } })) },
  reviewThreads: { totalCount: threads.length, nodes: threads },
})

const line = (f: ReviewFacts, columns = 100) => {
  const reviews = reviewsOf(f)

  return reviews === null ? null : verdictLine(reviews, columns).map(piece => piece.text).join(' · ')
}

describe('who reviewed', () => {
  test('each verdict, changes first, then approvals, comments and requests', async () => {
    const f = facts(
      [review('nadav', 'COMMENTED', '2026-10-07T10:00:00Z'), review('alice', 'APPROVED', '2026-10-07T11:00:00Z')],
      [review('alice', 'APPROVED', '2026-10-07T11:00:00Z'), review('bob', 'CHANGES_REQUESTED', '2026-10-07T09:00:00Z')],
      ['carol'],
    )
    expect(line(f)).toBe('✗ bob requested changes · ✓ alice approved · 💬 nadav commented · ◷ carol requested')
  })

  test('an approval stands after a later comment, as GitHub counts it', async () => {
    const f = facts([review('alice', 'COMMENTED', '2026-10-07T12:00:00Z')], [review('alice', 'APPROVED', '2026-10-07T11:00:00Z')])
    expect(reviewsOf(f)?.reviewers).toEqual([{ login: 'alice', verdict: 'approved' }])
  })

  test('a change request stands while its reviewer is asked again; a comment gives way to the request', async () => {
    // GitHub's latestReviews leaves out a reviewer with a request pending.
    const f = facts([], [review('bob', 'CHANGES_REQUESTED', '2026-10-07T09:00:00Z')], ['bob', 'nadav'])
    expect(line(f)).toBe('✗ bob requested changes · ◷ nadav requested')
  })

  test("the author's own replies are no review, and a review app goes by its name", async () => {
    const f = facts([review('octocat', 'COMMENTED', '2026-10-07T10:00:00Z'), review('copilot-pull-request-reviewer', 'COMMENTED', '2026-10-07T10:05:00Z', 'Bot')])
    expect(line(f)).toBe('💬 copilot commented')
  })

  test('a team asked to review goes by its slug', async () => {
    const f = { ...facts([]), reviewRequests: { totalCount: 1, nodes: [{ requestedReviewer: { __typename: 'Team', slug: 'core' } }] } }
    expect(line(f)).toBe('◷ core requested')
  })

  test('too long for the card: names only, then as many as fit beside "+N more"', async () => {
    const f = facts(
      [review('nadav', 'COMMENTED', '2026-10-07T10:00:00Z'), review('dana', 'COMMENTED', '2026-10-07T10:00:00Z')],
      [review('alice', 'APPROVED', '2026-10-07T11:00:00Z'), review('bob', 'CHANGES_REQUESTED', '2026-10-07T09:00:00Z')],
      ['carol', 'erin'],
    )
    expect(line(f, 46)).toBe('✗ bob · ✓ alice · 💬 nadav · 💬 dana · +2 more')
    expect(line(f, 25)).toBe('✗ bob · ✓ alice · +4 more')
    expect(line(f, 5)).toBe('✗ bob · +5 more')
  })

  test('reviewers beyond the page GitHub sent count in "+N more"', async () => {
    const f = facts([review('nadav', 'COMMENTED', '2026-10-07T10:00:00Z')])
    expect(line({ ...f, latestReviews: { totalCount: 13, nodes: f.latestReviews?.nodes ?? [] } })).toBe('💬 nadav commented · +12 more')
  })

  test('no reviewers, no line', async () => {
    expect(line(facts([]))).toBe('')
  })

  test('an entry an older version wrote has no review facts', async () => {
    expect(reviewsOf({ author: { login: 'octocat' } })).toBe(null)
  })
})

describe('review threads', () => {
  test('counts the unresolved ones, and which of those are outdated', async () => {
    const reviews = reviewsOf(facts([], [], [], [thread(false), thread(false, true), thread(false), thread(true), thread(true, true)]))
    expect(reviews).toMatchObject({ unresolved: 3, outdated: 1, isCapped: false })
    expect(reviews === null ? '' : threadLine(reviews)).toBe('3 unresolved threads (1 outdated)')
  })

  test('one thread, none outdated', async () => {
    const reviews = reviewsOf(facts([], [], [], [thread(false), thread(true)]))
    expect(reviews === null ? '' : threadLine(reviews)).toBe('1 unresolved thread')
  })

  test('all resolved: nothing to say', async () => {
    const reviews = reviewsOf(facts([], [], [], [thread(true)]))
    expect(reviews === null ? 'x' : threadLine(reviews)).toBe('')
  })

  test("under a headline that counts them already, the card adds only how many are outdated", async () => {
    const reviews = reviewsOf(facts([], [], [], [thread(false), thread(false, true), thread(false)]))
    if (reviews === null) throw new Error('no reviews')
    expect(threadsBeside('3 comments to address', reviews)).toEqual({ line: '', note: '1 outdated' })
    expect(threadsBeside('Approved · 3 unresolved threads', reviews)).toEqual({ line: '', note: '1 outdated' })
    expect(threadsBeside('Changes requested', reviews)).toEqual({ line: '3 unresolved threads (1 outdated)', note: '' })
  })

  test('more threads than were asked for: the count is a floor', async () => {
    const f = facts([], [], [], [thread(false), thread(true)])
    const reviews = reviewsOf({ ...f, reviewThreads: { totalCount: 80, nodes: f.reviewThreads?.nodes ?? [] } })
    expect(reviews?.isCapped).toBe(true)
    expect(reviews === null ? '' : threadLine(reviews)).toBe('1+ unresolved threads')
  })
})

describe('new reviews', () => {
  const before = facts([review('nadav', 'COMMENTED', '2026-10-07T10:00:00Z')])
  const at = Date.parse('2026-10-07T10:00:00Z')

  test("a PR's first look toasts nothing, and marks what it saw", async () => {
    expect(newReviews(before, null, ['me'])).toEqual({ fresh: [], seen: at })
    expect(newReviews(facts([]), null, ['me'])).toEqual({ fresh: [], seen: 0 })
  })

  test('a later review is new, once', async () => {
    const after = facts([review('nadav', 'COMMENTED', '2026-10-07T10:00:00Z'), review('alice', 'APPROVED', '2026-10-07T11:00:00Z')])
    const first = newReviews(after, at, ['me'])
    expect(first.fresh).toEqual([{ who: 'alice', verdict: 'approved', at: Date.parse('2026-10-07T11:00:00Z') }])
    expect(newReviews(after, first.seen, ['me']).fresh).toEqual([])
  })

  test("the person's own review is not, whichever of their accounts wrote it, but still moves the mark", async () => {
    const mine = facts([review('nadav', 'COMMENTED', '2026-10-07T10:00:00Z'), review('Me-Work', 'COMMENTED', '2026-10-07T11:00:00Z')])
    expect(newReviews(mine, at, ['me', 'me-work'])).toEqual({ fresh: [], seen: Date.parse('2026-10-07T11:00:00Z') })
  })

  test('a review that drops out and comes back (a request made and withdrawn) is not new again', async () => {
    const seen = Date.parse('2026-10-07T11:00:00Z')
    const back = facts([review('nadav', 'COMMENTED', '2026-10-07T10:00:00Z'), review('alice', 'APPROVED', '2026-10-07T11:00:00Z')])
    expect(newReviews(back, seen, ['me']).fresh).toEqual([])
  })

  test('an entry with no review facts keeps the mark', async () => {
    expect(newReviews({}, at, ['me'])).toEqual({ fresh: [], seen: at })
  })

  test('toasts: one per review, and one for a burst', async () => {
    const r = (who: string, verdict: 'approved' | 'changes' | 'commented') => ({ who, verdict, at })
    expect(reviewToasts([r('nadav', 'commented')], 532)).toEqual(['💬 nadav commented on #532'])
    expect(reviewToasts([r('alice', 'approved'), r('bob', 'changes')], 12)).toEqual(['✓ alice approved #12', '✗ bob requested changes on #12'])
    expect(reviewToasts([r('a', 'commented'), r('b', 'commented'), r('c', 'approved')], 12)).toEqual(['💬 3 new reviews on #12'])
  })
})
