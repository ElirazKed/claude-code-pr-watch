// Handing a failed CI run to Claude: which checks failed, their logs trimmed to the part that
// says why, and the message that asks for a fix. Pure, so the pane only fetches and delivers.
import type { TrackedPr } from '../types'
import { isStatusContext, latestRuns, verdictOf } from './pr'
import type { CheckItem } from './pr'

// The tail of each failed job's log that is kept, how many jobs' logs are fetched, and the
// characters every log together may take: enough to find the error, not the runner's setup.
export const LOG_LINES = 120
export const LOG_JOBS = 3
export const LOG_CHARS = 12_000
const LINE_CHARS = 400

export type Failure = {
  name: string
  workflow: string | null
  // In words: "failure", "timed out", "error".
  conclusion: string
  url: string | null
  // A check run's title and summary, or a status's description.
  summary: string | null
  // A GitHub Actions job, whose failed steps' log gh can fetch; null for any other CI.
  jobId: number | null
}

const words = (value: string | null | undefined) => (value ?? 'failed').toLowerCase().replace(/_/g, ' ')

const clip = (text: string, max: number) => (text.length > max ? `${text.slice(0, max - 1)}…` : text)

// The checks failing on the latest run of each, counted the way the card counts them.
export function failingChecks(items: readonly CheckItem[], repo: string): Failure[] {
  return latestRuns(items)
    .filter(item => verdictOf(item) === 'failed')
    .map(item => {
      const isContext = isStatusContext(item)
      const id = typeof item.databaseId === 'number' ? item.databaseId : null
      const about = isContext ? item.description : [item.title, item.summary].filter(Boolean).join(' · ')
      const fallback = id === null ? null : `https://github.com/${repo}/runs/${id}`

      return {
        name: item.name ?? item.context ?? 'check',
        workflow: item.workflow ?? null,
        conclusion: words(isContext ? item.state : item.conclusion),
        url: item.detailsUrl ?? item.targetUrl ?? fallback,
        summary: about ? clip(about.replace(/\s+/g, ' ').trim(), 300) : null,
        // Only a run in a workflow is an Actions job; another app's check run has no log here.
        jobId: !isContext && item.workflow !== undefined ? id : null,
      }
    })
}

// The jobs whose logs are worth a fetch: the first few Actions failures.
export const toFetch = (failures: readonly Failure[]) => failures.filter(f => f.jobId !== null).slice(0, LOG_JOBS)

const ANSI = /\u001b\[[0-9;?]*[A-Za-z]|\u001b\][^\u0007]*\u0007/g
const STAMP = /^﻿?\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z ?/

// `gh run view --log-failed` prints "job<TAB>step<TAB>timestamp text" a line: kept are the
// text, without colour codes or the runner's group markers, and the failed steps' names.
export function cleanLog(raw: string): { steps: string[]; lines: string[] } {
  const steps = new Set<string>()
  const lines: string[] = []
  for (const row of raw.replace(/\r/g, '').split('\n')) {
    const parts = row.split('\t')
    const step = parts.length >= 3 ? (parts[1] ?? '') : ''
    if (step !== '' && step !== 'UNKNOWN STEP') steps.add(step)
    const text = (parts.length >= 3 ? parts.slice(2).join('\t') : row).replace(STAMP, '').replace(ANSI, '')
    if (/^##\[endgroup\]/.test(text)) continue
    lines.push(clip(text.replace(/^##\[group\]/, ''), LINE_CHARS))
  }
  while (lines.length > 0 && lines[lines.length - 1]?.trim() === '') lines.pop()

  return { steps: [...steps], lines }
}

export type Trimmed = { text: string; steps: string[]; kept: number; total: number }

// The end of the log, where the error is: its last `maxLines` lines, fewer if they pass `maxChars`.
export function trimLog(raw: string, maxLines = LOG_LINES, maxChars = LOG_CHARS): Trimmed {
  const { steps, lines } = cleanLog(raw)
  const tail = lines.slice(-maxLines)
  let size = tail.reduce((n, line) => n + line.length + 1, 0)
  while (tail.length > 1 && size > maxChars) size -= (tail.shift()?.length ?? 0) + 1
  while (tail.length > 0 && tail[0]?.trim() === '') tail.shift()

  return { text: tail.join('\n'), steps, kept: tail.length, total: lines.length }
}

// What gh said when a log would not come, in plain words.
export function logError(stderr: string): string {
  const text = (stderr.trim().split('\n')[0] ?? '').replace(/^failed to get run log:\s*/i, '')
  if (/HTTP 410/.test(text)) return 'GitHub no longer keeps this log'
  if (/HTTP 404/.test(text)) return "GitHub has no log for it, or this account can't see it"
  if (/in progress/i.test(text)) return 'its run is still going, and GitHub gives the log once it ends'

  return text || 'gh run view failed'
}

// A failure and what came of fetching its log: the raw log, or why there is none.
export type Handoff = { failure: Failure; log?: string; error?: string }

// Logs that would not come, for the card: "No log for build: GitHub no longer keeps this log".
export function logProblems(handoffs: readonly Handoff[]): string | null {
  const problems = handoffs.filter(h => h.error !== undefined).map(h => `No log for ${h.failure.name}: ${h.error}`)

  return problems.length > 0 ? problems.join('; ') : null
}

// A fence longer than any run of backticks in the log, so the log can't close it.
const fence = (text: string) => '`'.repeat(Math.max(3, ...[...text.matchAll(/`+/g)].map(m => m[0].length + 1)))

type PrFacts = Pick<TrackedPr, 'repo' | 'number' | 'title' | 'url' | 'branch' | 'base'>

// The message for Claude: the PR, what failed, the logs' tails, and the ask. It never asks
// for a push or a merge; that stays the person's call.
export function fixPrompt(pr: PrFacts, handoffs: readonly Handoff[]): string {
  const logs = handoffs.filter(h => h.log !== undefined)
  const budget = Math.floor(LOG_CHARS / Math.max(1, logs.length))
  const branch = pr.branch || 'its head branch'
  const out = [
    `CI failed on ${pr.repo}#${pr.number}: ${pr.title || 'untitled'}`,
    `${pr.url} · ${branch} → ${pr.base || 'its base'} · checks: ${pr.url}/checks`,
    '',
    'Failing checks (the latest run of each):',
    ...handoffs.map(({ failure: f }) => {
      const name = f.workflow === null || f.workflow === f.name ? f.name : `${f.workflow} / ${f.name}`

      return `- ${[name, f.conclusion, f.summary, f.url].filter(Boolean).join(' · ')}`
    }),
  ]
  for (const h of handoffs) {
    const name = h.failure.name
    if (h.error !== undefined) out.push('', `${name}: no log (${h.error}).`)
    if (h.log === undefined) continue
    const log = trimLog(h.log, LOG_LINES, budget)
    if (log.kept === 0) {
      out.push('', `${name}: gh found no failed step in its log.`)
      continue
    }
    const step = log.steps.length > 0 ? `, failed step ${log.steps.map(s => `"${s}"`).join(', ')}` : ''
    const span = log.kept < log.total ? `last ${log.kept} of ${log.total} lines, trimmed` : `all ${log.total} lines`
    const bar = fence(log.text)
    out.push('', `${name} log (${span}${step}):`, bar, log.text, bar)
  }
  const unfetched = handoffs.filter(h => h.failure.jobId !== null && h.log === undefined && h.error === undefined).length
  if (unfetched > 0) out.push('', `Logs of the first ${LOG_JOBS} failing jobs only; ${unfetched} more not fetched.`)
  out.push(
    '',
    `Find the cause and fix it on this branch (${branch}). If ${pr.repo} or ${branch} isn't checked out here, say so first.`,
  )

  return out.join('\n')
}
