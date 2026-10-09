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

// A card's Fix with Claude row: logs being collected, the message held for a yes where no
// prompt box can take it as a draft, what came of it, or what went wrong.
export type FixAsk = {
  busy?: Busy
  asking?: string
  done?: string
  error?: string
}

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
      // By PR url.
      fixing: Record<string, FixAsk>
    }
  }
}
