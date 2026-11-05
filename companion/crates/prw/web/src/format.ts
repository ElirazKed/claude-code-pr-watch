// Words and orders the page shows: everything about a PR itself comes worked out from prw.

import type { Bucket, List, Row } from './types'

/** "now", "4m", "3h", "2d": how long ago, from the model's clock. */
export function ago(now: number, at: number | null | undefined): string {
  if (at == null) return ''
  const minutes = Math.max(0, Math.floor((now - at) / 60_000))
  if (minutes < 1) return 'now'
  if (minutes < 60) return `${minutes}m`
  if (minutes < 60 * 24) return `${Math.round(minutes / 60)}h`
  return `${Math.round(minutes / (60 * 24))}d`
}

export const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`

/** Your own lists, in the rail's order (and keys 1, 2, 3). */
export const OWN_LISTS = ['focus', 'inbox', 'later'] as const

/** The groups a list shows, most pressing first. */
export const BUCKETS: { id: Bucket; label: string; hint: string; tone: Tone }[] = [
  { id: 'you', label: 'Needs you', hint: 'conflicts, changes asked, your review', tone: 'red' },
  { id: 'failing', label: 'Failing', hint: 'a check failed', tone: 'red' },
  { id: 'ready', label: 'Ready to merge', hint: 'approved and green', tone: 'green' },
  { id: 'running', label: 'In progress', hint: 'checks running or auto-merge on', tone: 'yellow' },
  { id: 'waiting', label: 'Waiting on others', hint: 'nothing for you to do yet', tone: 'blue' },
  { id: 'done', label: 'Done', hint: 'merged or closed', tone: 'lav' },
]

export type Tone = 'red' | 'green' | 'yellow' | 'blue' | 'lav' | 'dim'

/** The colour a PR's next step takes. */
export function toneOf(row: Row): Tone {
  if (row.pill === 'loading') return 'dim'
  return BUCKETS.find((b) => b.id === row.bucket)?.tone ?? 'blue'
}

/** What the rail says under a list's name, and the page above it. */
export function aboutOf(list: List): { sub: string; about: string } {
  switch (list.kind) {
    case 'focus':
      return {
        sub: "what you're on now",
        about: 'The PRs you pinned, each with its next step. p unpins one, z puts it aside.',
      }
    case 'inbox':
      return {
        sub: 'from your agents',
        about:
          "What your agents' sessions watch that you haven't sorted yet. Pin one to Focus, or put it aside in Later.",
      }
    case 'later':
      return {
        sub: 'back when it changes',
        about:
          'Put aside, out of every other list. Each comes back by itself when something changes on it: a push, a review, CI, a merge.',
      }
    case 'mine':
      return { sub: "every login's open PRs", about: 'Every open PR of each gh login.' }
    case 'search': {
      const found =
        list.total != null && list.total > list.rows.length
          ? ` ${list.rows.length} of ${list.total} shown.`
          : ''
      return { sub: list.query ?? '', about: `A saved search: ${list.query ?? ''}.${found}` }
    }
  }
}

/** What a list's rows are grouped by: what each needs (the buckets), or its repo. */
export type Grouping = 'status' | 'repo'

/** A list's rows by repo, A to Z by name and then owner (as the TUI's "by repo"), each repo's
    rows in the order they came: most urgent first. */
export function byRepo(rows: Row[]): { repo: string; rows: Row[] }[] {
  const groups = new Map<string, Row[]>()
  for (const row of rows) groups.set(row.repo, [...(groups.get(row.repo) ?? []), row])
  const key = (repo: string) => {
    const [owner = '', name = ''] = repo.toLowerCase().split('/')
    return `${name}/${owner}`
  }
  return [...groups]
    .map(([repo, rows]) => ({ repo, rows }))
    .sort((a, b) => (key(a.repo) < key(b.repo) ? -1 : key(a.repo) > key(b.repo) ? 1 : 0))
}

/** Whether a row matches the filter: its title, repo, branch, author, number or agent. */
export function matches(row: Row, query: string): boolean {
  const q = query.trim().toLowerCase()
  if (!q) return true
  return [row.title, row.repo, row.branch, row.author, `#${row.number}`, row.agent ?? '']
    .join(' ')
    .toLowerCase()
    .includes(q)
}

/** Every row once, from whichever list has it first. */
export function rowByUrl(lists: List[], url: string | null): Row | undefined {
  if (!url) return undefined
  for (const list of lists) {
    const row = list.rows.find((r) => r.url === url)
    if (row) return row
  }
  return undefined
}
