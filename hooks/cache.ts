// What results.json keeps of each PR, and what a failed fetch and a round do to it.
import type { GhPr } from './pr'

// `at`: when `pr` was fetched, or, with no `pr` (never read), when the fetch last failed.
// `failedSince`: the first failed fetch in a row, while the last data still stands; `error`:
// what the card says, once it says anything. Sessions of an older version spread an entry,
// so a field they don't know is no harm to them.
export type Entry = { at: number; pr?: GhPr; error?: string; failedSince?: number }

// How long fetches may fail before a card that has data says so. GitHub's gateway drops a
// query now and then (a 502 or 504) and the next round gets through; until then the card
// keeps its last data, and its "checked 1m ago" says how old that is. Two minutes is three
// idle rounds, or a few more busy ones, and the card then reads "checked 2m ago" beside it.
export const QUIET_MS = 120_000

// A PR's entry after a fetch at `at` failed with `error`. One never read says so at once, as
// before; one with data keeps it and its time, and shows the error once failing QUIET_MS.
// An older version's entry has an error and no failedSince: its `at` is when it failed.
export function failedEntry(entry: Entry | undefined, error: string, at: number): Entry {
  if (entry?.pr === undefined) return { at, error }
  const { error: was, failedSince, ...kept } = entry
  const since = failedSince ?? (was !== undefined ? entry.at : at)

  return at - since >= QUIET_MS ? { ...kept, failedSince: since, error } : { ...kept, failedSince: since }
}

// How long a round keeps a PR no live session watches, after its last fetch: a session
// back from a restart within the hour still finds its PRs.
export const KEEP_MS = 3_600_000

// What a round keeps: every PR a live session watches, however old (a merged one is never
// asked again, so its entry is what says it merged), and the rest until KEEP_MS old.
export function keptEntries(prs: Readonly<Record<string, Entry>>, live: ReadonlySet<string>, at: number): Record<string, Entry> {
  return Object.fromEntries(Object.entries(prs).filter(([url, entry]) => live.has(url) || at - entry.at < KEEP_MS))
}
