export type Stage = 'draft' | 'checks' | 'review' | 'ready' | 'merged' | 'closed' | 'unknown'
export type Tone = 'success' | 'error' | 'warning' | 'merged' | 'subtle' | 'suggestion'

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
}

declare module 'claude-code' {
  interface PluginState {
    'pr-watch': { prs: TrackedPr[]; now: number; suggested: string[] }
  }
}
