export type Stage = 'draft' | 'checks' | 'review' | 'ready' | 'merged' | 'closed' | 'unknown'
export type Tone = 'success' | 'error' | 'warning' | 'merged' | 'subtle' | 'suggestion'
export type MergeMethod = 'squash' | 'rebase' | 'merge'

export type Checks = {
  passed: number
  failed: number
  pending: number
  total: number
  failing: string[]
  // Pending checks split: started on a runner vs still waiting for one.
  running: string[]
  queued: number
}

// What the viewer may do about merging, as GitHub said at the last check.
export type Mergeable = {
  // The methods the repo allows, the viewer's default first.
  methods: MergeMethod[]
  // GitHub would merge it now, and the viewer may.
  canMerge: boolean
  // It waits on checks or review, and the viewer may turn auto-merge on.
  canAuto: boolean
  isAuto: boolean
  // The method auto-merge will use, when GitHub said.
  autoMethod: MergeMethod | null
  canCancelAuto: boolean
}

// Where a reviewer stands: their approval or change request, a review asked of them, or a
// comment-only review.
export type Verdict = 'approved' | 'changes' | 'commented' | 'requested'

export type Reviewer = { login: string; verdict: Verdict }

// A PR's reviewers and open review threads, as GitHub said at the last check.
export type Reviews = {
  reviewers: Reviewer[]
  // Reviewers and requests beyond the ones asked for.
  more: number
  unresolved: number
  // Unresolved threads on code that has changed since.
  outdated: number
  // More threads than were asked for: `unresolved` counts the newest only.
  isCapped: boolean
}

export type TrackedPr = {
  url: string
  repo: string
  number: number
  title: string
  state: 'OPEN' | 'MERGED' | 'CLOSED' | 'UNKNOWN'
  stage: Stage
  headline: string
  pill: string
  tone: Tone
  isMoving: boolean
  checks: Checks
  branch: string
  base: string
  additions: number
  deletions: number
  author: string
  mergedAt: string | null
  checkedAt: number | null
  error: string | null
  merge: Mergeable
  // Null until a round with review facts lands.
  reviews: Reviews | null
  // The newest review submission looked at (ms), so only later ones toast; null before the
  // first look.
  reviewSeen: number | null
  // GitHub can't merge the head into the base cleanly (draft or not): Resolve with Claude.
  isConflicted: boolean
  // Never read, as no logged-in gh account could see it: its error names the logins asked,
  // and the card offers Try again.
  isNotFound: boolean
}

// A press's work under way, from when it started: a mark older than a run can take was left by
// a reload mid-run, and counts for nothing.
export type Busy = { at: number }

// A card's merge row between presses: the method picked, the question asked, gh running (and
// for which action), or what GitHub said no with.
export type MergeAsk = {
  method?: MergeMethod
  asking?: 'merge' | 'auto'
  busy?: Busy & { action: 'merge' | 'auto' | 'cancel-auto' }
  error?: string
}

// A card's hand-off to Claude (Fix, Address or Resolve with Claude): what it needs being
// collected, the message held for a yes where no prompt box can take it as a draft, what came
// of it, or what went wrong.
export type HandoffAsk = {
  busy?: Busy
  asking?: string
  done?: string
  error?: string
}

// A card's comment: its field open, the post under way, the comment posted (its link on
// GitHub, and the login it went as), or what GitHub said no with. The typed text is kept
// apart, so a keystroke redraws nothing.
export type CommentAsk = {
  isOpen?: boolean
  busy?: Busy
  done?: { url: string; login: string }
  error?: string
}

// A card's tabs (tabs.ts): Overview, what the card has always said, and the three it reads on
// its own when opened.
export type Tab = 'overview' | 'checks' | 'reviews' | 'threads'

// One check on the head commit, as the Checks tab lists it.
export type CheckRow = { name: string; workflow: string | null; state: 'failed' | 'running' | 'queued' | 'passed' | 'skipped' }

// One unresolved review thread, as the Threads tab lists it: where it is, who opened it, the
// first line they wrote, and how many replied.
export type ThreadRow = { spot: string; author: string; text: string; replies: number; isOutdated: boolean }

// A group whose review the branch's rules ask for: a code owner (a team, or a person) or a
// ruleset's required team. Approved by enough of its members; changes asked by one; asked and
// waiting; or required only where the PR touches its files (`maybe`), which GitHub doesn't say.
export type Gate = { name: string; state: 'approved' | 'changes' | 'waiting' | 'maybe'; by: string[]; need: number; patterns: string[] }

// What the base branch's rules require of a PR's reviews, and where it stands.
export type Approvals = {
  // Approvals the rules ask for (the strictest rule), and the ones given.
  required: number
  approved: number
  isOwnersRequired: boolean
  gates: Gate[]
}

// What a tab read: being read, what came, or why nothing did; and the card as it was read
// (tabs.ts's seenOf), so a round that changes it reads again.
export type Fetched<T> = { busy?: Busy; data?: T; error?: string; seen?: string }

export type Details = { checks?: Fetched<CheckRow[]>; reviews?: Fetched<Approvals>; threads?: Fetched<ThreadRow[]> }

// A newer pr-watch than the one running, and how this copy updates (update.ts).
export type Newer = { version: string; how: string }

// One of GitHub's hourly limits for a gh login, as last seen (rate.ts): GraphQL's points or
// REST's requests. Times in ms.
export type Bucket = { limit: number; remaining: number; resetAt: number; seenAt: number }

// A login's two buckets, and when the round last asked GitHub's /rate_limit for it.
export type LoginRates = { graphql?: Bucket; core?: Bucket; askedAt?: number }

// What the gh meter shows: each logged-in login's buckets, the active one first, and until
// when a secondary limit holds every session off GitHub (0: no hold).
export type Limits = { logins: ({ login: string } & Pick<LoginRates, 'graphql' | 'core'>)[]; heldUntil: number }

declare module 'claude-code' {
  interface PluginState {
    'pr-watch': {
      prs: TrackedPr[]
      now: number
      suggested: string[]
      dropped: string[]
      // By PR url.
      merging: Record<string, MergeAsk>
      // Methods GitHub refused this session, by repo.
      refused: Record<string, MergeMethod[]>
      // Fix with Claude, by PR url.
      fixing: Record<string, HandoffAsk>
      // Address with Claude, by PR url.
      addressing: Record<string, HandoffAsk>
      // Resolve with Claude, by PR url.
      resolving: Record<string, HandoffAsk>
      // Comment, by PR url.
      commenting: Record<string, CommentAsk>
      // Try again on a not-found card, by PR url: while every login is asked.
      retrying: Record<string, Busy>
      // Null while no newer version is known, or the notice is off.
      newer: Newer | null
      // Whether the card telling of prw shows: not once dismissed, or prw seen running, or the
      // notice off (update.ts).
      companion: boolean
      // GitHub's budget per login, and a hold, as the shared results last said.
      limits: Limits
      // Whether the gh meter's explanation is open (its `?`).
      ghHelp: boolean
      // The tab each card shows, by PR url: Overview where none was picked.
      tabs: Record<string, Tab>
      // What each card's tabs read, by PR url.
      details: Record<string, Details>
    }
  }
}
