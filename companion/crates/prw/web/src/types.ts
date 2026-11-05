// What `GET /api/state` answers: prw-core's `web::State`, field for field (camelCase).
// Kept by hand; `web::state_of` is the source of truth.

export type Poll = 'mod' | 'prw' | 'nobody' | 'stale' | 'waiting'

export type Bucket = 'you' | 'failing' | 'ready' | 'running' | 'waiting' | 'done'

export type Pill =
  | 'merged'
  | 'closed'
  | 'draft'
  | 'conflicts'
  | 'ciFailing'
  | 'ciFailed'
  | 'ciRunning'
  | 'changes'
  | 'comments'
  | 'inReview'
  | 'behind'
  | 'blocked'
  | 'checking'
  | 'autoMerging'
  | 'ready'
  | 'loading'

export type CheckState = 'fail' | 'run' | 'queue' | 'skip' | 'pass'

export type Stop = 'done' | 'now' | 'blocked' | 'todo' | 'merged'

export type Icon =
  | 'reading'
  | 'merged'
  | 'closed'
  | 'conflict'
  | 'alert'
  | 'thread'
  | 'eye'
  | 'failed'
  | 'running'
  | 'ready'
  | 'waiting'
  | 'draft'

export interface State {
  version: string
  /** The model's clock (ms): ages count from it. */
  now: number
  poll: PollView
  logins: string[]
  lists: List[]
  toast: Toast | null
  newer: string | null
}

export interface PollView {
  by: Poll
  words: string
  fetchedAt: number | null
  live: number
  isReadFailing: boolean
  error: string | null
  problems: string[]
}

export type ListKind = 'focus' | 'inbox' | 'mine' | 'search' | 'later'

export interface List {
  /** `focus`, `inbox`, `mine`, `later`, or `search:<name>`. */
  id: string
  kind: ListKind
  label: string
  query: string | null
  total: number | null
  error: string | null
  rows: Row[]
}

export interface Row {
  url: string
  number: number
  repo: string
  repoName: string
  /** The repo's chip colour: `--repo-0` and on. */
  hue: number
  title: string
  author: string
  isYours: boolean
  agent: string | null
  watchers: string[]
  login: string | null
  branch: string
  base: string
  additions: number
  deletions: number
  openedAt: number | null
  updatedAt: number | null
  checkedAt: number | null
  pill: Pill
  pillText: string
  headline: string
  isMoving: boolean
  error: string | null
  bucket: Bucket
  checks: Check[]
  reviewers: Reviewer[]
  unresolved: number
  stations: Stations | null
  next: Next
  isFocus: boolean
  later: Later | null
}

export interface Check {
  name: string
  state: CheckState
}

export interface Reviewer {
  login: string
  verdict: 'approved' | 'changes' | 'commented' | 'requested'
}

export interface Station {
  stop: Stop
  text: string
}

export interface Stations {
  checks: Station
  review: Station
  merge: Station
}

export interface Next {
  icon: Icon
  title: string
  text: string
  actions: Action[]
}

export type Action =
  | { kind: 'link'; label: string; url: string; isPrimary: boolean }
  | { kind: 'unpin'; label: string }

export interface Later {
  url: string
  at: number
  from: 'focus' | 'inbox'
  pill: Pill
  updatedAt?: number
}

export interface Toast {
  text: string
  tone: 'good' | 'bad' | 'info'
  until: number
}

/** What `POST /api/lists` takes: one PR, and where it should be. */
export type Change = { url: string; focus: boolean } | { url: string; later: boolean }
