// A PR's review state as a card shows it: where each reviewer stands, the review threads still
// open, and which reviews landed since the last look.
import type { Reviewer, Reviews, Verdict } from '../types'

type Actor = { __typename?: string; login?: string; slug?: string } | null

export type GhReview = { author: Actor; state: string; submittedAt?: string | null }

// The review connections the poller asks for (batch.ts), in GitHub's shape. `latestReviews` is
// each reviewer's last review of any kind, but leaves out a reviewer with a request pending (a
// re-request too); `latestOpinionatedReviews` is each one's last approval or change request,
// which still counts after a later comment or a re-request.
export type ReviewFacts = {
  latestReviews?: { totalCount?: number; nodes: GhReview[] } | null
  latestOpinionatedReviews?: { nodes: GhReview[] } | null
  reviewRequests?: { totalCount?: number; nodes: { requestedReviewer: Actor }[] } | null
  // The newest threads, enough to count; never their comments.
  reviewThreads?: { totalCount?: number; nodes: { isResolved: boolean; isOutdated: boolean }[] } | null
}

type Reviewed = Exclude<Verdict, 'requested'>

const VERDICT: Record<string, Reviewed> = { APPROVED: 'approved', CHANGES_REQUESTED: 'changes', COMMENTED: 'commented' }
const RANK: readonly Verdict[] = ['changes', 'approved', 'commented', 'requested']

// A review app's login cut to its name (copilot-pull-request-reviewer → copilot), so the line
// stays short; a person's login stays whole.
export function nameOf(actor: Actor): string {
  const login = actor?.login ?? actor?.slug ?? ''

  return actor?.__typename === 'Bot' && login.length > 12 ? login.split('-')[0] || login : login
}

const keyOf = (actor: Actor) => (actor?.__typename === 'Team' ? `team:${actor.slug ?? ''}` : (actor?.login ?? ''))

// Each reviewer once. An approval or change request stands until its reviewer changes it, as
// GitHub counts it for the decision; else a pending request; else their last comment. The PR's
// author is no reviewer (their replies in a thread are reviews too). Null when the entry has no
// review facts: an older version of this mod ran that round.
export function reviewsOf(pr: ReviewFacts & { author?: { login: string } | null }): Reviews | null {
  const { latestReviews: latest, reviewThreads: threads } = pr
  if (latest == null || threads == null) return null
  const requests = pr.reviewRequests ?? { nodes: [] }
  const seen = new Map<string, Reviewer>()
  const put = (actor: Actor, verdict: Verdict | undefined) => {
    const key = keyOf(actor)
    if (verdict === undefined || key === '' || key === pr.author?.login || seen.has(key)) return
    seen.set(key, { login: nameOf(actor), verdict })
  }
  for (const review of pr.latestOpinionatedReviews?.nodes ?? []) {
    if (review.state !== 'COMMENTED') put(review.author, VERDICT[review.state])
  }
  for (const request of requests.nodes) put(request.requestedReviewer, 'requested')
  for (const review of latest.nodes) put(review.author, VERDICT[review.state])
  const beyond = (total: number | undefined, shown: number) => Math.max(0, (total ?? shown) - shown)
  const open = threads.nodes.filter(thread => !thread.isResolved)

  return {
    reviewers: [...seen.values()].sort((a, b) => RANK.indexOf(a.verdict) - RANK.indexOf(b.verdict)),
    more: beyond(latest.totalCount, latest.nodes.length) + beyond(requests.totalCount, requests.nodes.length),
    unresolved: open.length,
    outdated: open.filter(thread => thread.isOutdated).length,
    isCapped: beyond(threads.totalCount, threads.nodes.length) > 0,
  }
}

// "3", or "8+" when there are more threads than were asked for: a floor, not a count.
const count = (reviews: Reviews) => `${reviews.unresolved}${reviews.isCapped ? '+' : ''}`
const plural = (reviews: Reviews, word: string) => `${count(reviews)} ${word}${reviews.unresolved === 1 && !reviews.isCapped ? '' : 's'}`

// "3 unresolved threads", or nothing.
export const unresolved = (reviews: Reviews) => (reviews.unresolved === 0 ? '' : plural(reviews, 'unresolved thread'))

// "3 unresolved threads (1 outdated)": outdated ones are on code that has changed since.
export function threadLine(reviews: Reviews): string {
  const text = unresolved(reviews)

  return text === '' || reviews.outdated === 0 ? text : `${text} (${reviews.outdated} outdated)`
}

// "3 comments to address", for the headline.
export const toAddress = (reviews: Reviews) => `${plural(reviews, 'comment')} to address`

// What a card says of the threads under its headline: the whole line, or, where the headline
// counts them already ("3 comments to address", "Approved · 2 unresolved threads"), only how
// many are outdated, beside it.
export function threadsBeside(headline: string, reviews: Reviews): { line: string; note: string } {
  const isCounted = reviews.unresolved > 0 && (headline === toAddress(reviews) || headline.includes(unresolved(reviews)))
  if (!isCounted) return { line: threadLine(reviews), note: '' }

  return { line: '', note: reviews.outdated > 0 ? `${reviews.outdated} outdated` : '' }
}

const ICON: Record<Verdict, string> = { approved: '✓', changes: '✗', commented: '💬', requested: '◷' }
const SAYS: Record<Verdict, string> = { approved: 'approved', changes: 'requested changes', commented: 'commented', requested: 'requested' }

// Terminal cells: an emoji outside the basic plane (💬) takes two.
export function cells(text: string): number {
  return [...text].reduce((n, ch) => n + ((ch.codePointAt(0) ?? 0) > 0xffff ? 2 : 1), 0)
}

export type Piece = { text: string; verdict: Verdict | null }

const SEP = 3 // ' · '

// The reviewers line, to fit `columns`: "✓ alice approved · ✗ bob requested changes" while it
// fits, else just icons and names ("✓ alice · ✗ bob · 💬 nadav"), as many as fit beside a
// "+2 more" (a null verdict). Always at least one reviewer: the card truncates the rest.
export function verdictLine(reviews: Pick<Reviews, 'reviewers' | 'more'>, columns: number): Piece[] {
  const width = (pieces: readonly Piece[]) => pieces.reduce((n, p) => n + cells(p.text), 0) + SEP * Math.max(0, pieces.length - 1)
  const more = (n: number): Piece[] => (n > 0 ? [{ text: `+${n} more`, verdict: null }] : [])
  const long = reviews.reviewers.map(r => ({ text: `${ICON[r.verdict]} ${r.login} ${SAYS[r.verdict]}`, verdict: r.verdict }))
  if (width([...long, ...more(reviews.more)]) <= columns) return [...long, ...more(reviews.more)]
  const short = reviews.reviewers.map(r => ({ text: `${ICON[r.verdict]} ${r.login}`, verdict: r.verdict }))
  let shown = short.length
  while (shown > 1 && width([...short.slice(0, shown), ...more(short.length - shown + reviews.more)]) > columns) shown -= 1

  return [...short.slice(0, shown), ...more(short.length - shown + reviews.more)]
}

export type ReviewEvent = { who: string; verdict: Reviewed; at: number }

// The reviews submitted since `seen` (the newest submission time already looked at), by anyone
// but the person (any of their gh logins) or the PR's author (their thread replies are reviews
// too, and reviewsOf leaves them out). On a PR's first look (`seen` null) none is new:
// they were there before the card. Returns the new mark, which only moves forward, so a review
// that drops out of the reply and comes back (a request made and withdrawn) isn't new again.
export function newReviews(pr: ReviewFacts, seen: number | null, mine: readonly string[], author?: string): { fresh: ReviewEvent[]; seen: number | null } {
  if (pr.latestReviews == null) return { fresh: [], seen }
  const own = new Set([...mine, author ?? ''].filter(Boolean).map(login => login.toLowerCase()))
  const all = new Map<string, ReviewEvent & { login: string }>()
  for (const review of [...pr.latestReviews.nodes, ...(pr.latestOpinionatedReviews?.nodes ?? [])]) {
    const at = Date.parse(review.submittedAt ?? '')
    const verdict = VERDICT[review.state]
    const login = review.author?.login ?? ''
    if (Number.isFinite(at) && verdict !== undefined && login !== '') all.set(`${login}@${at}`, { who: nameOf(review.author), login, verdict, at })
  }
  const mark = Math.max(seen ?? 0, ...[...all.values()].map(review => review.at))
  if (seen === null) return { fresh: [], seen: mark }
  const fresh = [...all.values()]
    .filter(review => review.at > seen && !own.has(review.login.toLowerCase()))
    .sort((a, b) => a.at - b.at)
    .map(({ who, verdict, at }) => ({ who, verdict, at }))

  return { fresh, seen: mark }
}

const SAID: Record<Reviewed, (who: string, number: number) => string> = {
  approved: (who, number) => `✓ ${who} approved #${number}`,
  changes: (who, number) => `✗ ${who} requested changes on #${number}`,
  commented: (who, number) => `💬 ${who} commented on #${number}`,
}

// One toast per new review; a burst of them (a round after a long sleep) is one toast.
export function reviewToasts(fresh: readonly ReviewEvent[], number: number): string[] {
  if (fresh.length > 2) return [`💬 ${fresh.length} new reviews on #${number}`]

  return fresh.map(review => SAID[review.verdict](review.who, number))
}
